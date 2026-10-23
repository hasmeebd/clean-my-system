//! Large files, natural-language file search and AI facts.

use crate::util::*;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, Metadata};
use std::os::windows::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------- file entries

#[derive(Serialize, Clone)]
pub struct FileEntry {
    pub path: String,
    pub name: String,
    pub ext: String,
    pub size: u64,
    pub logical: u64,
    pub modified: u64,
}

impl FileEntry {
    pub fn new(p: &Path, alloc: u64, logical: u64, md: &Metadata) -> Self {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let ext = p
            .extension()
            .map(|n| format!(".{}", n.to_string_lossy().to_lowercase()))
            .unwrap_or_default();
        FileEntry {
            path: p.to_string_lossy().to_string(),
            name,
            ext,
            size: alloc,
            logical,
            modified: mtime(md),
        }
    }
}

pub fn skip_sys() -> DirFilter {
    Box::new(|p, _| {
        let s = p.to_string_lossy().to_lowercase();
        s.ends_with("\\windows\\winsxs") || s.ends_with("\\system volume information")
    })
}

pub fn large_cb(min: u64, sink: Arc<Mutex<Vec<FileEntry>>>) -> FileCb {
    Box::new(move |p, alloc, logical, md| {
        if alloc.max(logical) >= min {
            sink.lock().unwrap().push(FileEntry::new(p, alloc, logical, md));
        }
    })
}

// ---------------------------------------------------------------- natural language filter

#[derive(Serialize, Clone, Default)]
pub struct Filter {
    pub name_contains: Vec<String>,
    pub extensions: Vec<String>,
    pub min_mb: f64,
    pub older_than_days: f64,
    pub summary: String,
}

fn num(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn strs(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.trim().to_lowercase()))
            .filter(|s| !s.is_empty())
            .collect(),
        Value::String(s) if !s.trim().is_empty() => vec![s.trim().to_lowercase()],
        _ => vec![],
    }
}

impl Filter {
    pub fn from_value(v: &Value) -> Filter {
        let mut f = Filter {
            name_contains: strs(&v["name_contains"]),
            extensions: strs(&v["extensions"])
                .into_iter()
                .map(|e| if e.starts_with('.') { e } else { format!(".{}", e) })
                .collect(),
            min_mb: num(&v["min_mb"]).max(0.0),
            older_than_days: num(&v["older_than_days"]).max(0.0),
            summary: v["summary"].as_str().unwrap_or("").to_string(),
        };
        // A filter with no criteria would return the whole disk.
        if f.name_contains.is_empty() && f.extensions.is_empty() && f.min_mb == 0.0 && f.older_than_days == 0.0 {
            f.min_mb = 100.0;
        }
        f
    }
}

pub fn search_cb(f: Filter, sink: Arc<Mutex<Vec<FileEntry>>>) -> FileCb {
    let min = (f.min_mb * 1_048_576.0) as u64;
    let cutoff = if f.older_than_days > 0.0 {
        now().saturating_sub((f.older_than_days * 86_400.0) as u64)
    } else {
        u64::MAX
    };
    Box::new(move |p, alloc, logical, md| {
        if alloc < min {
            return;
        }
        let mt = mtime(md);
        if cutoff != u64::MAX && mt > cutoff {
            return;
        }
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if !f.extensions.is_empty() && !f.extensions.iter().any(|e| name.ends_with(e.as_str())) {
            return;
        }
        if !f.name_contains.is_empty() && !f.name_contains.iter().any(|n| name.contains(n.as_str())) {
            return;
        }
        let mut s = sink.lock().unwrap();
        if s.len() < 50_000 {
            s.push(FileEntry::new(p, alloc, logical, md));
        }
    })
}

// ---------------------------------------------------------------- facts for the AI

pub fn gather_facts(path: &Path, cancel: Arc<std::sync::atomic::AtomicBool>) -> Result<String, String> {
    let md = fs::symlink_metadata(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    let mut s = String::new();
    s.push_str(&format!("Path: {}\n", path.display()));
    if is_dir_attr(md.file_attributes()) {
        let stats: Arc<Mutex<HashMap<String, (u64, u64)>>> = Arc::new(Mutex::new(HashMap::new()));
        let newest = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let oldest = Arc::new(std::sync::atomic::AtomicU64::new(u64::MAX));
        let mut ctx = Ctx::new(cancel);
        {
            let st = stats.clone();
            let nw = newest.clone();
            let od = oldest.clone();
            ctx.on_file = Some(Box::new(move |p, alloc, _l, md| {
                let ext = p
                    .extension()
                    .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()))
                    .unwrap_or_else(|| "(none)".into());
                let mut m = st.lock().unwrap();
                let e = m.entry(ext).or_insert((0, 0));
                e.0 += alloc;
                e.1 += 1;
                let t = mtime(md);
                nw.fetch_max(t, Ordering::Relaxed);
                od.fetch_min(t, Ordering::Relaxed);
            }));
        }
        let a = walk(path, &ctx);
        s.push_str("Type: folder\n");
        s.push_str(&format!(
            "Size on disk: {:.2} GB (logical {:.2} GB), files: {}\n",
            a.alloc as f64 / 1e9,
            a.logical as f64 / 1e9,
            a.files
        ));
        let o = oldest.load(Ordering::Relaxed);
        if o != u64::MAX {
            s.push_str(&format!(
                "File modified range (unix seconds): {} .. {}\n",
                o,
                newest.load(Ordering::Relaxed)
            ));
        }
        let mut ex: Vec<(String, (u64, u64))> = stats.lock().unwrap().iter().map(|(k, v)| (k.clone(), *v)).collect();
        ex.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
        s.push_str("Top extensions by size:\n");
        for (k, (b, c)) in ex.iter().take(8) {
            s.push_str(&format!("  {} : {:.2} GB in {} files\n", k, *b as f64 / 1e9, c));
        }
        if let Ok(rd) = fs::read_dir(path) {
            let names: Vec<String> = rd
                .flatten()
                .take(30)
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect();
            s.push_str(&format!("Sample entries: {}\n", names.join(" | ")));
        }
    } else {
        s.push_str("Type: file\n");
        s.push_str(&format!(
            "Size on disk: {:.2} GB (logical {:.2} GB)\nModified (unix seconds): {}\n",
            alloc_size(path, &md) as f64 / 1e9,
            md.len() as f64 / 1e9,
            mtime(&md)
        ));
    }
    Ok(s)
}
