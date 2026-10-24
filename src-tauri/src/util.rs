//! Shared helpers: size-on-disk, parallel walker, settings, history.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::{self, Metadata};
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Storage::FileSystem::{
    GetCompressedFileSizeW, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives,
};

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn is_dir_attr(a: u32) -> bool {
    a & 0x10 != 0
}
pub fn is_reparse(a: u32) -> bool {
    a & 0x400 != 0
}

// ---------------------------------------------------------------- paths

pub fn profile() -> PathBuf {
    std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default()
}

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\Public"));
    let d = base.join("CDriveStudio");
    let _ = fs::create_dir_all(&d);
    d
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn mtime(md: &Metadata) -> u64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Expand %VAR% placeholders.
pub fn fx(t: &str) -> String {
    let mut out = String::new();
    let mut rest = t;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        if let Some(j) = after.find('%') {
            out.push_str(&std::env::var(&after[..j]).unwrap_or_default());
            rest = &after[j + 1..];
        } else {
            out.push_str(&rest[i..]);
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    out
}

pub fn wide(p: &Path) -> Vec<u16> {
    let s = p.to_string_lossy();
    let s2 = if s.len() >= 240 && !s.starts_with(r"\\?\") && s.chars().nth(1) == Some(':') {
        format!(r"\\?\{}", s)
    } else {
        s.to_string()
    };
    std::ffi::OsStr::new(&s2).encode_wide().chain(Some(0)).collect()
}

pub fn is_admin() -> bool {
    unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
}

// ---------------------------------------------------------------- sizes

/// Real space used on disk. Sparse and compressed files report their allocated
/// size instead of the logical size, and cloud-only placeholders count as zero.
pub fn alloc_size(path: &Path, md: &Metadata) -> u64 {
    let a = md.file_attributes();
    let logical = md.len();
    if a & (0x0040_0000 | 0x0004_0000) != 0 {
        return 0; // OneDrive style placeholder, not stored locally
    }
    let needs_os = a & (0x200 | 0x800) != 0 || logical >= 64 * 1024 * 1024;
    if !needs_os {
        return logical;
    }
    unsafe {
        let w = wide(path);
        let mut high: u32 = 0;
        let low = GetCompressedFileSizeW(w.as_ptr(), &mut high);
        if low == 0xFFFF_FFFF && GetLastError() != 0 {
            return logical;
        }
        ((high as u64) << 32) | low as u64
    }
}

#[derive(Serialize, Clone)]
pub struct DriveInfo {
    pub letter: String,
    pub total: u64,
    pub free: u64,
    pub used: u64,
}

pub fn drive_info(letter: &str) -> Option<DriveInfo> {
    let l = letter.trim_end_matches('\\').trim_end_matches(':');
    let root = format!("{}:\\", l);
    let w: Vec<u16> = std::ffi::OsStr::new(&root).encode_wide().chain(Some(0)).collect();
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    let ok = unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut avail, &mut total, &mut free) };
    if ok == 0 {
        return None;
    }
    Some(DriveInfo {
        letter: format!("{}:", l.to_uppercase()),
        total,
        free: avail,
        used: total.saturating_sub(free),
    })
}

pub fn fixed_drives() -> Vec<String> {
    let mask = unsafe { GetLogicalDrives() };
    let mut v = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) != 0 {
            let l = (b'A' + i as u8) as char;
            let root: Vec<u16> = format!("{}:\\", l).encode_utf16().chain(Some(0)).collect();
            if unsafe { GetDriveTypeW(root.as_ptr()) } == 3 {
                v.push(format!("{}:", l));
            }
        }
    }
    v
}

// ---------------------------------------------------------------- walker

#[derive(Default, Clone, Copy, Serialize)]
pub struct Agg {
    pub alloc: u64,
    pub logical: u64,
    pub files: u64,
}
impl std::ops::Add for Agg {
    type Output = Agg;
    fn add(self, o: Agg) -> Agg {
        Agg {
            alloc: self.alloc + o.alloc,
            logical: self.logical + o.logical,
            files: self.files + o.files,
        }
    }
}

pub type FileCb = Box<dyn Fn(&Path, u64, u64, &Metadata) + Send + Sync>;
pub type DirFilter = Box<dyn Fn(&Path, &str) -> bool + Send + Sync>;

pub struct Ctx {
    pub files: AtomicU64,
    pub bytes: AtomicU64,
    pub cancel: Arc<AtomicBool>,
    pub on_file: Option<FileCb>,
    pub skip_dir: Option<DirFilter>,
}

impl Ctx {
    pub fn new(cancel: Arc<AtomicBool>) -> Ctx {
        Ctx {
            files: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            cancel,
            on_file: None,
            skip_dir: None,
        }
    }
}

/// Parallel recursive size walk. Never follows junctions or symlinks.
pub fn walk(path: &Path, ctx: &Ctx) -> Agg {
    if ctx.cancel.load(Ordering::Relaxed) {
        return Agg::default();
    }
    let rd = match fs::read_dir(path) {
        Ok(r) => r,
        Err(_) => return Agg::default(),
    };
    let mut agg = Agg::default();
    let mut subs: Vec<PathBuf> = Vec::new();
    for e in rd.flatten() {
        let md = match e.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let a = md.file_attributes();
        let p = e.path();
        if is_dir_attr(a) {
            if is_reparse(a) {
                continue;
            }
            if let Some(f) = &ctx.skip_dir {
                if f(&p, &e.file_name().to_string_lossy()) {
                    continue;
                }
            }
            subs.push(p);
        } else {
            let alloc = alloc_size(&p, &md);
            let logical = md.len();
            agg.alloc += alloc;
            agg.logical += logical;
            agg.files += 1;
            ctx.files.fetch_add(1, Ordering::Relaxed);
            ctx.bytes.fetch_add(alloc, Ordering::Relaxed);
            if let Some(cb) = &ctx.on_file {
                cb(&p, alloc, logical, &md);
            }
        }
    }
    let sub = subs
        .par_iter()
        .map(|d| walk(d, ctx))
        .reduce(Agg::default, |a, b| a + b);
    agg + sub
}

// ---------------------------------------------------------------- deletion helpers

fn remove_file_force(p: &Path) -> std::io::Result<()> {
    match fs::remove_file(p) {
        Ok(_) => Ok(()),
        Err(e) => {
            if let Ok(md) = fs::metadata(p) {
                let mut perm = md.permissions();
                perm.set_readonly(false);
                let _ = fs::set_permissions(p, perm);
            }
            fs::remove_file(p).map_err(|_| e)
        }
    }
}

/// Delete everything inside `dir` but keep `dir`. Returns (bytes freed, failures).
pub fn remove_contents(dir: &Path, cancel: &AtomicBool) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut failed = 0u64;
    let rd = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return (0, 0),
    };
    for e in rd.flatten() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let p = e.path();
        let md = match e.metadata() {
            Ok(m) => m,
            Err(_) => {
                failed += 1;
                continue;
            }
        };
        let a = md.file_attributes();
        if is_dir_attr(a) {
            if is_reparse(a) {
                // remove the link itself, never what it points to
                if fs::remove_dir(&p).is_err() {
                    failed += 1;
                }
                continue;
            }
            let (b, f) = remove_contents(&p, cancel);
            bytes += b;
            failed += f;
            if fs::remove_dir(&p).is_err() {
                failed += 1;
            }
        } else {
            let sz = alloc_size(&p, &md);
            if remove_file_force(&p).is_ok() {
                bytes += sz;
            } else {
                failed += 1;
            }
        }
    }
    (bytes, failed)
}

// ---------------------------------------------------------------- settings

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    pub model: String,
    pub lang: String,
    pub ollama_host: String,
    pub quarantine_days: u64,
    pub protected: Vec<String>,
    /// Per-model "thinking" switch (missing = off). Only used for models that support it.
    pub think: std::collections::HashMap<String, bool>,
    /// Folders scanned by Dev Junk.
    pub junk_roots: Vec<String>,
    /// A project counts as stale after this many days without activity.
    pub junk_stale_days: u64,
    /// Safe-clean targets the scheduled task may run.
    pub auto_clean_ids: Vec<String>,
    /// Closing the window keeps the app in the system tray.
    pub close_to_tray: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            model: "qwen3:8b".into(),
            lang: "bn".into(),
            ollama_host: "http://127.0.0.1:11434".into(),
            quarantine_days: 7,
            protected: vec![format!("{}\\Desktop\\Arian", profile().display())],
            think: std::collections::HashMap::new(),
            junk_roots: Vec::new(),
            junk_stale_days: 30,
            auto_clean_ids: vec!["user_temp".into(), "crash_dumps".into()],
            close_to_tray: false,
        }
    }
}

pub fn load_settings() -> Settings {
    fs::read_to_string(data_dir().join("settings.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_settings(s: &Settings) {
    if let Ok(t) = serde_json::to_string_pretty(s) {
        let _ = fs::write(data_dir().join("settings.json"), t);
    }
}

// ---------------------------------------------------------------- history & snapshots

pub fn log_history(action: &str, detail: &str, bytes: u64) {
    let line = json!({"ts": now(), "action": action, "detail": detail, "bytes": bytes}).to_string();
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data_dir().join("history.jsonl"))
    {
        let _ = writeln!(f, "{}", line);
    }
}

pub fn read_history(limit: usize) -> Vec<Value> {
    let txt = fs::read_to_string(data_dir().join("history.jsonl")).unwrap_or_default();
    let mut v: Vec<Value> = txt
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    v.reverse();
    v.truncate(limit);
    v
}

pub fn read_snapshots() -> Vec<Value> {
    fs::read_to_string(data_dir().join("snapshots.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn snapshot() {
    let sys = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    if let Some(d) = drive_info(&sys) {
        let mut v = read_snapshots();
        v.push(json!({"ts": now(), "free": d.free, "used": d.used}));
        if v.len() > 300 {
            let cut = v.len() - 300;
            v.drain(0..cut);
        }
        let _ = fs::write(
            data_dir().join("snapshots.json"),
            serde_json::to_string(&v).unwrap_or_default(),
        );
    }
}
