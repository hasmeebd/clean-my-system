//! Duplicate file finder. Size -> partial hash -> full hash (blake3), so only files that
//! really are byte-identical end up in a group. Removal re-verifies content right before
//! anything is touched and always keeps at least one copy per group.

use crate::quarantine::Fail;
use crate::util::*;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Serialize, Clone)]
pub struct DupFile {
    pub path: String,
    pub modified: u64,
}

#[derive(Serialize, Clone)]
pub struct DupGroup {
    pub size: u64,
    pub wasted: u64,
    pub files: Vec<DupFile>,
}

#[derive(Serialize)]
pub struct DupReport {
    pub groups: Vec<DupGroup>,
    pub total_wasted: u64,
    pub scanned_files: u64,
    pub same_size_files: u64,
    pub truncated: bool,
}

#[derive(Deserialize)]
pub struct DupSel {
    pub keep: Vec<String>,
    pub remove: Vec<String>,
}

/// Folder names that are never searched: system, app data, tool caches, and regenerable
/// folders that the Dev Junk page already handles.
const SKIP: [&str; 20] = [
    "appdata", "windows", "program files", "program files (x86)", "programdata", "$recycle.bin",
    "system volume information", ".git", "node_modules", ".cargo", ".rustup", ".gradle", ".nuget",
    ".npm", ".cache", "target", ".venv", "venv", "__pycache__", ".cdrive-quarantine",
];

fn hash_file(path: &Path, partial: bool, bytes: &AtomicU64, cancel: &AtomicBool) -> Option<String> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut h = blake3::Hasher::new();
    h.update(&len.to_le_bytes());
    let mut buf = vec![0u8; 1 << 20];
    if partial {
        const CHUNK: usize = 64 * 1024;
        let n = f.read(&mut buf[..CHUNK]).ok()?;
        h.update(&buf[..n]);
        if len > (2 * CHUNK) as u64 {
            f.seek(SeekFrom::Start(len - CHUNK as u64)).ok()?;
            let n = f.read(&mut buf[..CHUNK]).ok()?;
            h.update(&buf[..n]);
        }
        bytes.fetch_add((2 * CHUNK) as u64, Ordering::Relaxed);
    } else {
        loop {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let n = f.read(&mut buf).ok()?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            bytes.fetch_add(n as u64, Ordering::Relaxed);
        }
    }
    Some(h.finalize().to_hex().to_string())
}

pub type Found = Arc<Mutex<Vec<(PathBuf, u64, u64)>>>;

/// Builds the walker context that collects candidate files (so progress can watch it).
pub fn make_ctx(cancel: Arc<AtomicBool>, min_bytes: u64) -> (Ctx, Found) {
    let found: Found = Arc::new(Mutex::new(Vec::new()));
    let mut ctx = Ctx::new(cancel);
    let f = found.clone();
    ctx.on_file = Some(Box::new(move |p, alloc, logical, md| {
        // alloc == 0 for a non-empty file means a cloud placeholder: hashing would download it
        if logical >= min_bytes && logical > 0 && alloc > 0 {
            f.lock().unwrap().push((p.to_path_buf(), logical, mtime(md)));
        }
    }));
    ctx.skip_dir = Some(Box::new(|_, name| SKIP.contains(&name.to_lowercase().as_str())));
    (ctx, found)
}

pub fn find(roots: &[PathBuf], ctx: &Ctx, found: &Found) -> DupReport {
    for r in roots {
        walk(r, ctx);
    }
    let all = std::mem::take(&mut *found.lock().unwrap());
    let scanned = all.len() as u64;
    let mut by_size: HashMap<u64, Vec<(PathBuf, u64)>> = HashMap::new();
    for (p, s, m) in all {
        by_size.entry(s).or_default().push((p, m));
    }
    by_size.retain(|_, v| v.len() > 1);
    let same_size: u64 = by_size.values().map(|v| v.len() as u64).sum();

    // 2. partial hash
    let cancel = ctx.cancel.clone();
    let bytes = &ctx.bytes;
    let flat: Vec<(u64, PathBuf, u64)> = by_size
        .into_iter()
        .flat_map(|(s, v)| v.into_iter().map(move |(p, m)| (s, p, m)))
        .collect();
    let partial: Vec<(u64, String, PathBuf, u64)> = flat
        .into_par_iter()
        .filter_map(|(s, p, m)| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            hash_file(&p, true, bytes, &cancel).map(|h| (s, h, p, m))
        })
        .collect();
    let mut by_partial: HashMap<(u64, String), Vec<(PathBuf, u64)>> = HashMap::new();
    for (s, h, p, m) in partial {
        by_partial.entry((s, h)).or_default().push((p, m));
    }
    by_partial.retain(|_, v| v.len() > 1);

    // 3. full hash
    let flat2: Vec<(u64, PathBuf, u64)> = by_partial
        .into_iter()
        .flat_map(|((s, _), v)| v.into_iter().map(move |(p, m)| (s, p, m)))
        .collect();
    let full: Vec<(u64, String, PathBuf, u64)> = flat2
        .into_par_iter()
        .filter_map(|(s, p, m)| {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            hash_file(&p, false, bytes, &cancel).map(|h| (s, h, p, m))
        })
        .collect();
    let mut by_full: HashMap<(u64, String), Vec<(PathBuf, u64)>> = HashMap::new();
    for (s, h, p, m) in full {
        by_full.entry((s, h)).or_default().push((p, m));
    }
    let mut groups: Vec<DupGroup> = by_full
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|((s, _), mut v)| {
            v.sort_by(|a, b| a.1.cmp(&b.1)); // oldest first
            DupGroup {
                size: s,
                wasted: s * (v.len() as u64 - 1),
                files: v.into_iter().map(|(p, m)| DupFile { path: p.to_string_lossy().to_string(), modified: m }).collect(),
            }
        })
        .collect();
    groups.sort_by(|a, b| b.wasted.cmp(&a.wasted));
    let total_wasted = groups.iter().map(|g| g.wasted).sum();
    let truncated = groups.len() > 1500;
    groups.truncate(1500);
    DupReport { groups, total_wasted, scanned_files: scanned, same_size_files: same_size, truncated }
}

/// Re-checks every file marked for removal against the copy that stays. Returns the paths
/// that are verified byte-identical, plus a failure for everything else.
pub fn verify(sels: &[DupSel], cancel: &AtomicBool) -> (Vec<String>, Vec<Fail>) {
    let sink = AtomicU64::new(0);
    let results: Vec<(Vec<String>, Vec<Fail>)> = sels
        .par_iter()
        .map(|sel| {
            let mut ok = Vec::new();
            let mut bad = Vec::new();
            let keep = sel.keep.iter().find(|k| fs::metadata(k).map(|m| m.is_file()).unwrap_or(false));
            let keep = match keep {
                Some(k) => k,
                None => {
                    for r in &sel.remove {
                        bad.push(Fail { path: r.clone(), reason: "No surviving copy to keep, skipped".into() });
                    }
                    return (ok, bad);
                }
            };
            let keep_len = fs::metadata(keep).map(|m| m.len()).unwrap_or(0);
            let keep_hash = hash_file(Path::new(keep), false, &sink, cancel);
            let keep_norm = |p: &str| sel.keep.iter().any(|k| k.eq_ignore_ascii_case(p));
            for r in &sel.remove {
                if keep_norm(r) {
                    bad.push(Fail { path: r.clone(), reason: "Marked both keep and remove, skipped".into() });
                    continue;
                }
                let len = match fs::metadata(r) {
                    Ok(m) if m.is_file() => m.len(),
                    _ => {
                        bad.push(Fail { path: r.clone(), reason: "File no longer exists".into() });
                        continue;
                    }
                };
                if len != keep_len || keep_hash.is_none() || hash_file(Path::new(r), false, &sink, cancel) != keep_hash {
                    bad.push(Fail { path: r.clone(), reason: "Content differs from the kept copy now, skipped".into() });
                    continue;
                }
                ok.push(r.clone());
            }
            (ok, bad)
        })
        .collect();
    let mut ok = Vec::new();
    let mut bad = Vec::new();
    for (o, b) in results {
        ok.extend(o);
        bad.extend(b);
    }
    (ok, bad)
}
