//! Browser privacy cleaner for what Safe Clean does not touch: cookies and browsing history.
//! Files are moved to Quarantine (so it can be undone), only from a fixed allowlist of file
//! names, and only while the browser is closed. Passwords, bookmarks, autofill and extensions
//! are never listed.

use crate::util::*;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone)]
pub struct Item {
    pub browser: String,
    pub profile: String,
    pub category: String,
    pub path: String,
    pub size: u64,
}

struct B {
    label: &'static str,
    procs: &'static [&'static str],
    root: &'static str,
    chromium: bool,
}

const BROWSERS: &[B] = &[
    B { label: "Chrome", procs: &["chrome.exe"], root: r"%LOCALAPPDATA%\Google\Chrome\User Data", chromium: true },
    B { label: "Edge", procs: &["msedge.exe"], root: r"%LOCALAPPDATA%\Microsoft\Edge\User Data", chromium: true },
    B { label: "Brave", procs: &["brave.exe"], root: r"%LOCALAPPDATA%\BraveSoftware\Brave-Browser\User Data", chromium: true },
    B { label: "Firefox", procs: &["firefox.exe"], root: r"%APPDATA%\Mozilla\Firefox\Profiles", chromium: false },
];

const COOKIES: &[&str] = &["Network\\Cookies", "Network\\Cookies-journal", "Cookies", "Cookies-journal"];
const HISTORY: &[&str] = &["History", "History-journal", "Visited Links", "Top Sites", "Top Sites-journal", "Shortcuts", "Shortcuts-journal"];
const FF_COOKIES: &[&str] = &["cookies.sqlite", "cookies.sqlite-wal", "cookies.sqlite-shm"];

fn running_names() -> Vec<String> {
    crate::ps::run_capture("tasklist", &["/FO", "CSV", "/NH"], &[], 15)
        .map(|o| o.stdout.lines().filter_map(|l| l.split("\",\"").next()).map(|s| s.trim_matches('"').to_lowercase()).collect())
        .unwrap_or_default()
}

pub fn running() -> Vec<String> {
    let r = running_names();
    BROWSERS.iter().filter(|b| b.procs.iter().any(|p| r.iter().any(|x| x == p))).map(|b| b.label.to_string()).collect()
}

fn push(out: &mut Vec<Item>, b: &B, profile: &str, cat: &str, base: &Path, files: &[&str]) {
    for f in files {
        let p = base.join(f);
        if let Ok(md) = fs::metadata(&p) {
            if md.is_file() {
                out.push(Item { browser: b.label.into(), profile: profile.into(), category: cat.into(), path: p.to_string_lossy().to_string(), size: md.len() });
            }
        }
    }
}

pub fn list() -> Vec<Item> {
    let mut out = Vec::new();
    for b in BROWSERS {
        let root = PathBuf::from(fx(b.root));
        let rd = match fs::read_dir(&root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !e.path().is_dir() {
                continue;
            }
            if b.chromium {
                if name == "Default" || name.starts_with("Profile ") {
                    push(&mut out, b, &name, "cookies", &e.path(), COOKIES);
                    push(&mut out, b, &name, "history", &e.path(), HISTORY);
                }
            } else {
                push(&mut out, b, &name, "cookies", &e.path(), FF_COOKIES);
            }
        }
    }
    out
}
