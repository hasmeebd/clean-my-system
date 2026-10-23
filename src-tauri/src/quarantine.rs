//! Quarantine: move instead of delete, with restore and timed purge.

use crate::util::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

static LOCK: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize, Clone)]
pub struct QItem {
    pub id: String,
    pub orig: String,
    pub qpath: String,
    pub size: u64,
    pub is_dir: bool,
    pub ts: u64,
}

#[derive(Serialize, Clone)]
pub struct Fail {
    pub path: String,
    pub reason: String,
}

#[derive(Serialize, Default)]
pub struct Report {
    pub ok: u64,
    pub bytes: u64,
    pub failed: Vec<Fail>,
    pub mode: String,
}

fn manifest_path() -> PathBuf {
    data_dir().join("quarantine.json")
}

fn load() -> Vec<QItem> {
    fs::read_to_string(manifest_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(items: &[QItem]) {
    if let Ok(t) = serde_json::to_string_pretty(items) {
        let _ = fs::write(manifest_path(), t);
    }
}

/// System drive uses the app data folder; other drives get a hidden folder at
/// the drive root so a move never has to copy across volumes.
fn qroot_for(p: &Path) -> PathBuf {
    let sys = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()).to_uppercase();
    let s = p.to_string_lossy();
    let drive: String = s.chars().take(2).collect::<String>().to_uppercase();
    if drive == sys || !drive.ends_with(':') {
        data_dir().join("quarantine")
    } else {
        PathBuf::from(format!("{}\\.cdrive-quarantine", drive))
    }
}

fn unique_id(i: usize) -> String {
    let n = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{}-{}", n, i)
}

fn rm_any(p: &Path, is_dir: bool) -> std::io::Result<()> {
    if is_dir {
        fs::remove_dir_all(p)
    } else {
        fs::remove_file(p)
    }
}

pub fn remove(paths: &[String], mode: &str, cancel: Arc<AtomicBool>) -> Report {
    let _g = LOCK.lock().unwrap();
    let mut items = load();
    let mut rep = Report { mode: mode.into(), ..Default::default() };
    for (i, ps) in paths.iter().enumerate() {
        let p = PathBuf::from(ps);
        let md = match fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(e) => {
                rep.failed.push(Fail { path: ps.clone(), reason: e.to_string() });
                continue;
            }
        };
        let is_dir = is_dir_attr(md.file_attributes());
        let size = if is_dir {
            walk(&p, &Ctx::new(cancel.clone())).alloc
        } else {
            alloc_size(&p, &md)
        };

        if mode == "permanent" {
            match rm_any(&p, is_dir) {
                Ok(_) => {
                    rep.ok += 1;
                    rep.bytes += size;
                }
                Err(e) => rep.failed.push(Fail { path: ps.clone(), reason: e.to_string() }),
            }
            continue;
        }

        let id = unique_id(i);
        let dest_dir = qroot_for(&p).join(&id);
        if let Err(e) = fs::create_dir_all(&dest_dir) {
            rep.failed.push(Fail { path: ps.clone(), reason: format!("Quarantine folder: {}", e) });
            continue;
        }
        let name = p.file_name().map(|n| n.to_os_string()).unwrap_or_default();
        let dest = dest_dir.join(name);
        match fs::rename(&p, &dest) {
            Ok(_) => {
                rep.ok += 1;
                rep.bytes += size;
                items.push(QItem {
                    id,
                    orig: ps.clone(),
                    qpath: dest.to_string_lossy().to_string(),
                    size,
                    is_dir,
                    ts: now(),
                });
            }
            Err(e) => {
                let _ = fs::remove_dir(&dest_dir);
                rep.failed.push(Fail { path: ps.clone(), reason: format!("In use or no permission: {}", e) });
            }
        }
    }
    save(&items);
    rep
}

pub fn list() -> Vec<QItem> {
    let _g = LOCK.lock().unwrap();
    let items = load();
    let kept: Vec<QItem> = items
        .iter()
        .filter(|i| fs::symlink_metadata(&i.qpath).is_ok())
        .cloned()
        .collect();
    if kept.len() != items.len() {
        save(&kept);
    }
    let mut kept = kept;
    kept.sort_by(|a, b| b.ts.cmp(&a.ts));
    kept
}

pub fn restore(ids: &[String]) -> Report {
    let _g = LOCK.lock().unwrap();
    let items = load();
    let mut keep = Vec::new();
    let mut rep = Report { mode: "restore".into(), ..Default::default() };
    for it in items {
        if !ids.contains(&it.id) {
            keep.push(it);
            continue;
        }
        let orig = PathBuf::from(&it.orig);
        if orig.exists() {
            rep.failed.push(Fail { path: it.orig.clone(), reason: "Something already exists at the original location".into() });
            keep.push(it);
            continue;
        }
        if let Some(parent) = orig.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match fs::rename(&it.qpath, &orig) {
            Ok(_) => {
                rep.ok += 1;
                rep.bytes += it.size;
                if let Some(c) = Path::new(&it.qpath).parent() {
                    let _ = fs::remove_dir(c);
                }
            }
            Err(e) => {
                rep.failed.push(Fail { path: it.orig.clone(), reason: e.to_string() });
                keep.push(it);
            }
        }
    }
    save(&keep);
    rep
}

fn purge_filter(pred: &dyn Fn(&QItem) -> bool) -> Report {
    let _g = LOCK.lock().unwrap();
    let items = load();
    let mut keep = Vec::new();
    let mut rep = Report { mode: "purge".into(), ..Default::default() };
    for it in items {
        if !pred(&it) {
            keep.push(it);
            continue;
        }
        let q = PathBuf::from(&it.qpath);
        let res = if fs::symlink_metadata(&q).is_ok() { rm_any(&q, it.is_dir) } else { Ok(()) };
        match res {
            Ok(_) => {
                rep.ok += 1;
                rep.bytes += it.size;
                if let Some(c) = q.parent() {
                    let _ = fs::remove_dir(c);
                }
            }
            Err(e) => {
                rep.failed.push(Fail { path: it.orig.clone(), reason: e.to_string() });
                keep.push(it);
            }
        }
    }
    save(&keep);
    rep
}

pub fn purge(ids: &[String], all: bool) -> Report {
    if all {
        purge_filter(&|_| true)
    } else {
        let ids = ids.to_vec();
        purge_filter(&move |i| ids.contains(&i.id))
    }
}

pub fn purge_expired(days: u64) -> Report {
    let cutoff = now().saturating_sub(days * 86_400);
    purge_filter(&move |i| i.ts < cutoff)
}
