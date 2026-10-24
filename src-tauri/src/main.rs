#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai;
mod apps;
mod browsers;
mod diag;
mod devjunk;
mod dupes;
mod finder;
mod fixes;
mod health;
mod hogs;
mod procs;
mod ps;
mod quarantine;
mod recover;
mod safety;
mod schedule;
mod startup;
mod tweaks;
mod updater;
mod util;

use rayon::prelude::*;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::os::windows::fs::MetadataExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use util::*;

struct AppState {
    cancel: Arc<AtomicBool>,
    hogs: Arc<Mutex<Vec<hogs::HogInfo>>>,
    large: Arc<Mutex<Vec<finder::FileEntry>>>,
    folder: Arc<Mutex<Option<FolderView>>>,
    settings: Arc<Mutex<Settings>>,
    ai_cancels: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    fix_cancels: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

// ------------------------------------------------------------ progress events

struct ProgressGuard(Arc<AtomicBool>);
impl Drop for ProgressGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

fn progress(app: &AppHandle, label: &str, ctx: Arc<Ctx>) -> ProgressGuard {
    let done = Arc::new(AtomicBool::new(false));
    let d = done.clone();
    let app = app.clone();
    let label = label.to_string();
    thread::spawn(move || {
        while !d.load(Ordering::Relaxed) {
            let _ = app.emit(
                "scan-progress",
                json!({
                    "label": label,
                    "files": ctx.files.load(Ordering::Relaxed),
                    "bytes": ctx.bytes.load(Ordering::Relaxed)
                }),
            );
            thread::sleep(Duration::from_millis(250));
        }
    });
    ProgressGuard(done)
}

fn join_err(e: tauri::Error) -> String {
    e.to_string()
}

fn sys_drive() -> String {
    std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into())
}

// ------------------------------------------------------------ simple commands

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
fn set_settings(state: State<'_, AppState>, settings: Settings) -> Settings {
    save_settings(&settings);
    *state.settings.lock().unwrap() = settings.clone();
    settings
}

#[tauri::command]
fn drive_info_cmd(letter: String) -> Option<DriveInfo> {
    drive_info(&letter)
}

#[tauri::command]
fn list_drives() -> Vec<DriveInfo> {
    fixed_drives().iter().filter_map(|l| drive_info(l)).collect()
}

#[tauri::command]
fn quick_paths() -> Vec<Value> {
    let p = profile();
    let la = std::env::var("LOCALAPPDATA").unwrap_or_default();
    vec![
        json!({"label": "Home", "path": p.to_string_lossy()}),
        json!({"label": "Desktop", "path": p.join("Desktop").to_string_lossy()}),
        json!({"label": "Downloads", "path": p.join("Downloads").to_string_lossy()}),
        json!({"label": "Documents", "path": p.join("Documents").to_string_lossy()}),
        json!({"label": "AppData\\Local", "path": la}),
        json!({"label": "Windows", "path": format!("{}\\Windows", sys_drive())}),
    ]
}

#[tauri::command]
fn is_admin_cmd() -> bool {
    is_admin()
}

#[tauri::command]
fn relaunch_admin(app: AppHandle) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let p = exe.to_string_lossy().replace('\'', "''");
    Command::new("powershell")
        .args([
            "-NoProfile",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &format!("Start-Process -FilePath '{}' -Verb RunAs", p),
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| e.to_string())?;
    app.exit(0);
    Ok(())
}

#[tauri::command]
fn reveal_in_explorer(path: String) -> Result<(), String> {
    Command::new("explorer")
        .arg(format!("/select,{}", path))
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn cancel_scan(state: State<'_, AppState>) {
    state.cancel.store(true, Ordering::Relaxed);
}

#[tauri::command]
fn get_history() -> Vec<Value> {
    read_history(300)
}

#[tauri::command]
fn get_snapshots() -> Vec<Value> {
    read_snapshots()
}

/// Dry run of the safety rules: reports whether each path may be removed, deletes nothing.
#[tauri::command]
fn check_paths(state: State<'_, AppState>, paths: Vec<String>) -> Vec<Value> {
    let protected = state.settings.lock().unwrap().protected.clone();
    paths
        .into_iter()
        .map(|p| match safety::check_deletable(Path::new(&p), &protected) {
            Ok(()) => json!({"path": p, "allowed": true, "reason": ""}),
            Err(r) => json!({"path": p, "allowed": false, "reason": r}),
        })
        .collect()
}

// ------------------------------------------------------------ folder analysis

#[derive(Serialize, Clone)]
struct Entry {
    name: String,
    path: String,
    size: u64,
    logical: u64,
    is_dir: bool,
    link: bool,
    modified: u64,
    files: u64,
}

#[derive(Serialize, Clone)]
struct FolderView {
    path: String,
    parent: Option<String>,
    total: u64,
    logical: u64,
    entries: Vec<Entry>,
}

#[tauri::command]
async fn analyze_folder(app: AppHandle, state: State<'_, AppState>, path: String) -> Result<FolderView, String> {
    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::Relaxed);
    let store = state.folder.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<FolderView, String> {
        let mut p = path.trim().trim_matches('"').to_string();
        if p.len() == 2 && p.ends_with(':') {
            p.push('\\');
        }
        let root = PathBuf::from(&p);
        let md = fs::metadata(&root).map_err(|e| format!("{}: {}", p, e))?;
        if !md.is_dir() {
            return Err("That path is not a folder".into());
        }
        let ctx = Arc::new(Ctx::new(cancel));
        let _g = progress(&app, "Analyzing", ctx.clone());
        let rd = fs::read_dir(&root).map_err(|e| e.to_string())?;
        let mut dirs: Vec<(String, PathBuf, u64)> = Vec::new();
        let mut entries: Vec<Entry> = Vec::new();
        for e in rd.flatten() {
            let md = match e.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let a = md.file_attributes();
            let ep = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if is_dir_attr(a) {
                if is_reparse(a) {
                    entries.push(Entry {
                        name,
                        path: ep.to_string_lossy().to_string(),
                        size: 0,
                        logical: 0,
                        is_dir: true,
                        link: true,
                        modified: mtime(&md),
                        files: 0,
                    });
                } else {
                    dirs.push((name, ep, mtime(&md)));
                }
            } else {
                let alloc = alloc_size(&ep, &md);
                ctx.files.fetch_add(1, Ordering::Relaxed);
                ctx.bytes.fetch_add(alloc, Ordering::Relaxed);
                entries.push(Entry {
                    name,
                    path: ep.to_string_lossy().to_string(),
                    size: alloc,
                    logical: md.len(),
                    is_dir: false,
                    link: false,
                    modified: mtime(&md),
                    files: 1,
                });
            }
        }
        let sized: Vec<Entry> = dirs
            .par_iter()
            .map(|(n, dp, m)| {
                let a = walk(dp, &ctx);
                Entry {
                    name: n.clone(),
                    path: dp.to_string_lossy().to_string(),
                    size: a.alloc,
                    logical: a.logical,
                    is_dir: true,
                    link: false,
                    modified: *m,
                    files: a.files,
                }
            })
            .collect();
        entries.extend(sized);
        entries.sort_by(|a, b| b.size.cmp(&a.size));
        let total = entries.iter().map(|e| e.size).sum();
        let logical = entries.iter().map(|e| e.logical).sum();
        let view = FolderView {
            path: root.to_string_lossy().to_string(),
            parent: root.parent().map(|x| x.to_string_lossy().to_string()),
            total,
            logical,
            entries,
        };
        *store.lock().unwrap() = Some(view.clone());
        Ok(view)
    })
    .await
    .map_err(join_err)?
}

// ------------------------------------------------------------ safe clean

#[tauri::command]
async fn list_hogs(app: AppHandle, state: State<'_, AppState>) -> Result<Vec<hogs::HogInfo>, String> {
    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::Relaxed);
    let store = state.hogs.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let ctx = Arc::new(Ctx::new(cancel));
        let _g = progress(&app, "Scanning known junk locations", ctx.clone());
        let v = hogs::scan(&ctx);
        *store.lock().unwrap() = v.clone();
        v
    })
    .await
    .map_err(join_err)
}

#[tauri::command]
async fn clean_hogs(state: State<'_, AppState>, ids: Vec<String>) -> Result<Value, String> {
    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::Relaxed);
    tauri::async_runtime::spawn_blocking(move || {
        let sd = sys_drive();
        let before = drive_info(&sd).map(|d| d.free).unwrap_or(0);
        let items = hogs::clean(&ids, &cancel);
        let after = drive_info(&sd).map(|d| d.free).unwrap_or(0);
        let reported: u64 = items.iter().map(|i| i.freed).sum();
        log_history(
            "safe-clean",
            &items.iter().map(|i| i.name.clone()).collect::<Vec<_>>().join(", "),
            reported,
        );
        snapshot();
        json!({"items": items, "reported": reported, "freed_on_disk": after.saturating_sub(before)})
    })
    .await
    .map_err(join_err)
}

#[tauri::command]
async fn run_tool(id: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let sd = sys_drive();
        let before = drive_info(&sd).map(|d| d.free).unwrap_or(0);
        let (program, args): (&str, Vec<&str>) = match id.as_str() {
            "storage_sense" => {
                Command::new("cmd")
                    .args(["/C", "start", "", "ms-settings:storagesense"])
                    .creation_flags(CREATE_NO_WINDOW)
                    .spawn()
                    .map_err(|e| e.to_string())?;
                return Ok(json!({"output": "Opened Windows Storage settings. Choose Temporary files, tick Previous Windows installation(s), then Remove files.", "freed": 0}));
            }
            "hibernate_off" => ("powercfg", vec!["-h", "off"]),
            "hibernate_on" => ("powercfg", vec!["-h", "on"]),
            "dism_cleanup" => ("Dism.exe", vec!["/Online", "/Cleanup-Image", "/StartComponentCleanup"]),
            _ => return Err("Unknown tool".into()),
        };
        if !is_admin() {
            return Err("Administrator rights are required. Use 'Restart as Administrator'.".into());
        }
        let out = Command::new(program)
            .args(&args)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| e.to_string())?;
        let mut text = String::from_utf8_lossy(&out.stdout).to_string();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        let tail: String = text.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        let after = drive_info(&sd).map(|d| d.free).unwrap_or(0);
        let freed = after.saturating_sub(before);
        log_history(&format!("tool:{}", id), tail.trim(), freed);
        snapshot();
        Ok(json!({"output": if tail.trim().is_empty() { "Done.".to_string() } else { tail }, "freed": freed, "ok": out.status.success()}))
    })
    .await
    .map_err(join_err)?
}

// ------------------------------------------------------------ finders

#[tauri::command]
async fn find_large_files(
    app: AppHandle,
    state: State<'_, AppState>,
    root: String,
    min_mb: f64,
    limit: usize,
) -> Result<Vec<finder::FileEntry>, String> {
    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::Relaxed);
    let store = state.large.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<finder::FileEntry>, String> {
        let r = PathBuf::from(root.trim().trim_matches('"'));
        if !r.is_dir() {
            return Err("Folder not found".into());
        }
        let sink = Arc::new(Mutex::new(Vec::new()));
        let mut ctx = Ctx::new(cancel);
        ctx.on_file = Some(finder::large_cb((min_mb.max(1.0) * 1_048_576.0) as u64, sink.clone()));
        ctx.skip_dir = Some(finder::skip_sys());
        let ctx = Arc::new(ctx);
        let _g = progress(&app, "Searching large files", ctx.clone());
        walk(&r, &ctx);
        let mut v = sink.lock().unwrap().clone();
        v.sort_by(|a, b| b.size.cmp(&a.size));
        v.truncate(limit.clamp(10, 1000));
        *store.lock().unwrap() = v.clone();
        Ok(v)
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
async fn find_dev_junk(
    app: AppHandle,
    state: State<'_, AppState>,
    roots: Vec<String>,
    include_ignored: bool,
) -> Result<devjunk::Report, String> {
    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::Relaxed);
    {
        // remember the folders for next time
        let mut st = state.settings.lock().unwrap();
        st.junk_roots = roots.iter().map(|r| r.trim().trim_matches('"').to_string()).filter(|r| !r.is_empty()).collect();
        save_settings(&st);
    }
    tauri::async_runtime::spawn_blocking(move || -> Result<devjunk::Report, String> {
        let rs: Vec<PathBuf> = roots
            .iter()
            .map(|r| PathBuf::from(r.trim().trim_matches('"')))
            .filter(|r| r.is_dir())
            .collect();
        if rs.is_empty() {
            return Err("None of the folders exist. Check the paths.".into());
        }
        let ctx = Arc::new(Ctx::new(cancel));
        let _g = progress(&app, "Looking for build output, caches and dependency folders", ctx.clone());
        Ok(devjunk::find(&rs, &ctx, include_ignored))
    })
    .await
    .map_err(join_err)?
}

// ------------------------------------------------------------ delete / quarantine

#[tauri::command]
async fn delete_paths(
    state: State<'_, AppState>,
    paths: Vec<String>,
    mode: String,
    kind: Option<String>,
) -> Result<quarantine::Report, String> {
    if mode != "quarantine" && mode != "permanent" {
        return Err("Unknown mode".into());
    }
    let protected = state.settings.lock().unwrap().protected.clone();
    let mut allowed: Vec<String> = Vec::new();
    let mut rejected: Vec<quarantine::Fail> = Vec::new();
    for p in paths {
        let pb = PathBuf::from(&p);
        match safety::check_deletable(&pb, &protected) {
            Err(reason) => rejected.push(quarantine::Fail { path: p, reason }),
            Ok(()) => {
                if kind.as_deref() == Some("junk") && !devjunk::is_junk_target(&pb) {
                    rejected.push(quarantine::Fail { path: p, reason: "Not a recognised, verifiable dev-junk item".into() });
                } else {
                    allowed.push(p);
                }
            }
        }
    }
    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::Relaxed);
    let mode2 = mode.clone();
    let mut rep = tauri::async_runtime::spawn_blocking(move || {
        let r = quarantine::remove(&allowed, &mode2, cancel);
        log_history(
            if mode2 == "permanent" { "delete" } else { "quarantine" },
            &format!("{} item(s)", r.ok),
            r.bytes,
        );
        snapshot();
        r
    })
    .await
    .map_err(join_err)?;
    rep.failed.extend(rejected);
    Ok(rep)
}

#[tauri::command]
fn list_quarantine() -> Vec<quarantine::QItem> {
    quarantine::list()
}

#[tauri::command]
fn restore_quarantine(ids: Vec<String>) -> quarantine::Report {
    let r = quarantine::restore(&ids);
    log_history("restore", &format!("{} item(s)", r.ok), r.bytes);
    snapshot();
    r
}

#[tauri::command]
async fn purge_quarantine(ids: Vec<String>, all: bool) -> Result<quarantine::Report, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let r = quarantine::purge(&ids, all);
        log_history("purge", &format!("{} item(s)", r.ok), r.bytes);
        snapshot();
        r
    })
    .await
    .map_err(join_err)
}

// ------------------------------------------------------------ AI

fn build_context(state: &AppState) -> String {
    let mut s = String::new();
    if let Some(d) = drive_info(&sys_drive()) {
        s.push_str(&format!(
            "Drive {}: total {:.1} GB, used {:.1} GB, free {:.1} GB\n",
            d.letter,
            d.total as f64 / 1e9,
            d.used as f64 / 1e9,
            d.free as f64 / 1e9
        ));
    }
    let hogs = state.hogs.lock().unwrap();
    if !hogs.is_empty() {
        s.push_str("Known junk locations (scan result):\n");
        for h in hogs.iter().filter(|h| h.size > 10_000_000).take(18) {
            s.push_str(&format!(
                "- {} [{}], {:.2} GB, risk {}{}\n",
                h.name,
                h.group,
                h.size as f64 / 1e9,
                h.risk,
                if h.auto {
                    if h.admin { ", auto-clean (needs Administrator)" } else { ", auto-clean available" }
                } else {
                    ", manual review only"
                }
            ));
        }
    }
    let large = state.large.lock().unwrap();
    if !large.is_empty() {
        s.push_str("Largest files found (INFORMATION ONLY - big does not mean unneeded; many are AI models or project assets the user wants to keep):\n");
        for f in large.iter().take(12) {
            s.push_str(&format!("- {} ({:.2} GB on disk)\n", f.path, f.size as f64 / 1e9));
        }
    }
    if let Some(fv) = state.folder.lock().unwrap().as_ref() {
        s.push_str(&format!("Folder currently open in Explorer: {} ({:.2} GB)\n", fv.path, fv.total as f64 / 1e9));
        for e in fv.entries.iter().take(12) {
            s.push_str(&format!("- {} {} {:.2} GB\n", if e.is_dir { "[dir]" } else { "[file]" }, e.name, e.size as f64 / 1e9));
        }
    }
    if s.trim().is_empty() {
        s.push_str("(No scan has been run yet. Suggest running Safe Clean scan or Explorer analysis first.)\n");
    }
    s
}

#[tauri::command]
fn ollama_status(state: State<'_, AppState>) -> Value {
    let host = state.settings.lock().unwrap().ollama_host.clone();
    ai::status(&host)
}

#[tauri::command]
fn ai_chat(
    app: AppHandle,
    state: State<'_, AppState>,
    req_id: String,
    model: String,
    messages: Vec<Value>,
    lang: String,
) -> Result<(), String> {
    let (host, think) = {
        let st = state.settings.lock().unwrap();
        (st.ollama_host.clone(), st.think.get(&model).copied().unwrap_or(false))
    };
    let system = ai::chat_system(&lang, &build_context(&state));
    let cancel = Arc::new(AtomicBool::new(false));
    state.ai_cancels.lock().unwrap().insert(req_id.clone(), cancel.clone());
    let mut msgs: Vec<Value> = messages
        .into_iter()
        .filter(|m| m["role"] == "user" || m["role"] == "assistant")
        .map(|m| json!({"role": m["role"], "content": m["content"]}))
        .collect();
    // Small models often ignore the language rule in the system prompt, so repeat it
    // right next to the question.
    if let Some(last) = msgs.last_mut() {
        if last["role"] == "user" {
            let c = last["content"].as_str().unwrap_or("").to_string();
            let note = if lang == "en" {
                "\n\n(Reply in English.)"
            } else {
                "\n\n(উত্তর বাংলায় দিন; technical term, path ও unit ইংরেজিতে রাখুন।)"
            };
            last["content"] = json!(format!("{}{}", c, note));
        }
    }
    ai::stream_chat(app, host, model, system, msgs, req_id, cancel, think);
    Ok(())
}

#[tauri::command]
fn ai_cancel(state: State<'_, AppState>, req_id: String) {
    if let Some(c) = state.ai_cancels.lock().unwrap().get(&req_id) {
        c.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
async fn ai_explain(state: State<'_, AppState>, path: String, model: String, lang: String) -> Result<Value, String> {
    let st = state.settings.lock().unwrap().clone();
    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::Relaxed);
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let facts = finder::gather_facts(Path::new(&path), cancel)?;
        let think = st.think.get(&model).copied().unwrap_or(false);
        let mut v = ai::explain(&st.ollama_host, &model, &lang, &facts, think)?;
        // Hard rule: the AI cannot downgrade a protected location.
        if let Some(reason) = safety::denied_reason(Path::new(&path), &st.protected) {
            v["risk"] = json!("keep");
            v["system_note"] = json!(reason);
        }
        v["facts"] = json!(facts);
        Ok(v)
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
async fn ai_search(
    app: AppHandle,
    state: State<'_, AppState>,
    query: String,
    model: String,
    root: String,
) -> Result<Value, String> {
    let (host, think) = {
        let st = state.settings.lock().unwrap();
        (st.ollama_host.clone(), st.think.get(&model).copied().unwrap_or(false))
    };
    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::Relaxed);
    tauri::async_runtime::spawn_blocking(move || -> Result<Value, String> {
        let raw = ai::nl_to_filter(&host, &model, &query, think)?;
        let filter = finder::Filter::from_value(&raw);
        let r = if root.trim().is_empty() { profile() } else { PathBuf::from(root.trim()) };
        if !r.is_dir() {
            return Err("Search folder not found".into());
        }
        let sink = Arc::new(Mutex::new(Vec::new()));
        let mut ctx = Ctx::new(cancel);
        ctx.on_file = Some(finder::search_cb(filter.clone(), sink.clone()));
        ctx.skip_dir = Some(finder::skip_sys());
        let ctx = Arc::new(ctx);
        let _g = progress(&app, "Searching", ctx.clone());
        walk(&r, &ctx);
        let mut v = sink.lock().unwrap().clone();
        v.sort_by(|a, b| b.size.cmp(&a.size));
        v.truncate(300);
        Ok(json!({"filter": filter, "results": v}))
    })
    .await
    .map_err(join_err)?
}

// ------------------------------------------------------------ duplicates

#[tauri::command]
async fn find_duplicates(app: AppHandle, state: State<'_, AppState>, roots: Vec<String>, min_mb: f64) -> Result<dupes::DupReport, String> {
    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::Relaxed);
    tauri::async_runtime::spawn_blocking(move || -> Result<dupes::DupReport, String> {
        let rs: Vec<PathBuf> = roots
            .iter()
            .map(|r| PathBuf::from(r.trim().trim_matches('"')))
            .filter(|r| r.is_dir())
            .collect();
        if rs.is_empty() {
            return Err("None of the folders exist. Check the paths.".into());
        }
        let min = (min_mb.max(0.1) * 1_048_576.0) as u64;
        let (ctx, found) = dupes::make_ctx(cancel, min);
        let ctx = Arc::new(ctx);
        let _g = progress(&app, "Comparing files (size, then content hash)", ctx.clone());
        Ok(dupes::find(&rs, &ctx, &found))
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
async fn delete_duplicates(state: State<'_, AppState>, groups: Vec<dupes::DupSel>, mode: String) -> Result<quarantine::Report, String> {
    if mode != "quarantine" && mode != "permanent" {
        return Err("Unknown mode".into());
    }
    let protected = state.settings.lock().unwrap().protected.clone();
    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::Relaxed);
    tauri::async_runtime::spawn_blocking(move || {
        // content is compared again right now, so a file edited since the scan is never removed
        let (verified, mut failed) = dupes::verify(&groups, &cancel);
        let mut allowed = Vec::new();
        for p in verified {
            match safety::check_deletable(Path::new(&p), &protected) {
                Ok(()) => allowed.push(p),
                Err(reason) => failed.push(quarantine::Fail { path: p, reason }),
            }
        }
        let mut rep = quarantine::remove(&allowed, &mode, cancel);
        log_history(if mode == "permanent" { "delete" } else { "quarantine" }, &format!("{} duplicate file(s)", rep.ok), rep.bytes);
        snapshot();
        rep.failed.extend(failed);
        rep
    })
    .await
    .map_err(join_err)
}

// ------------------------------------------------------------ startup

#[tauri::command]
async fn list_startup() -> Vec<startup::StartItem> {
    tauri::async_runtime::spawn_blocking(startup::list).await.unwrap_or_default()
}

#[tauri::command]
async fn set_startup(id: String, enable: bool) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let name = startup::set(&id, enable, is_admin())?;
        log_history("startup", &format!("{} {}", if enable { "enabled" } else { "disabled" }, name), 0);
        Ok(name)
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
async fn ai_explain_startup(state: State<'_, AppState>, name: String, command: String, kind: String, model: String, lang: String) -> Result<Value, String> {
    let (host, think) = {
        let st = state.settings.lock().unwrap();
        (st.ollama_host.clone(), st.think.get(&model).copied().unwrap_or(false))
    };
    tauri::async_runtime::spawn_blocking(move || ai::explain_startup(&host, &model, &lang, &name, &command, &kind, think))
        .await
        .map_err(join_err)?
}

// ------------------------------------------------------------ health

#[tauri::command]
async fn health_check() -> Result<health::Report, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let d = drive_info(&sys_drive());
        health::check(d.as_ref().map(|x| x.free).unwrap_or(0), d.as_ref().map(|x| x.total).unwrap_or(0))
    })
    .await
    .map_err(join_err)
}

// ------------------------------------------------------------ troubleshoot

#[tauri::command]
fn list_fixes() -> Vec<fixes::FixDef> {
    fixes::defs()
}

#[tauri::command]
fn run_fix(app: AppHandle, state: State<'_, AppState>, id: String, req_id: String) -> Result<(), String> {
    let cancel = Arc::new(AtomicBool::new(false));
    state.fix_cancels.lock().unwrap().insert(req_id.clone(), cancel.clone());
    fixes::run(app, id, req_id, is_admin(), cancel)
}

#[tauri::command]
fn cancel_fix(state: State<'_, AppState>, req_id: String) {
    if let Some(c) = state.fix_cancels.lock().unwrap().get(&req_id) {
        c.store(true, Ordering::Relaxed);
    }
}


// ------------------------------------------------------------ phase 2: optimize, apps, updates

#[tauri::command]
async fn list_tweaks() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(tweaks::list).await.map_err(join_err)
}

#[tauri::command]
async fn set_tweak(id: String, on: bool) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || tweaks::set(&id, on, is_admin())).await.map_err(join_err)?
}

#[tauri::command]
async fn list_power_plans() -> Result<Vec<Value>, String> {
    tauri::async_runtime::spawn_blocking(tweaks::power_plans).await.map_err(join_err)?
}

#[tauri::command]
async fn set_power_plan(guid: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || tweaks::set_power_plan(&guid)).await.map_err(join_err)?
}

#[tauri::command]
async fn list_installed() -> Result<Vec<apps::App>, String> {
    tauri::async_runtime::spawn_blocking(apps::list_installed).await.map_err(join_err)
}

#[tauri::command]
fn uninstall_app(app: AppHandle, id: String, quiet: bool) -> Result<String, String> {
    apps::uninstall(app, &id, quiet)
}

#[tauri::command]
async fn list_bloat() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(apps::list_bloat).await.map_err(join_err)?
}

#[tauri::command]
async fn list_updates() -> Result<updater::Listing, String> {
    tauri::async_runtime::spawn_blocking(updater::list).await.map_err(join_err)?
}


// ------------------------------------------------------------ phase 3: monitor, privacy, schedule, diagnostics

#[tauri::command]
async fn list_processes() -> Result<Vec<procs::Proc>, String> {
    tauri::async_runtime::spawn_blocking(procs::list).await.map_err(join_err)?
}

#[tauri::command]
async fn end_process(pid: u32, name: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || procs::end(pid, &name)).await.map_err(join_err)?
}

#[tauri::command]
async fn list_browser_data() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(|| json!({"items": browsers::list(), "running": browsers::running()})).await.map_err(join_err)
}

#[tauri::command]
async fn clean_browser(state: State<'_, AppState>, paths: Vec<String>) -> Result<quarantine::Report, String> {
    let protected = state.settings.lock().unwrap().protected.clone();
    let cancel = state.cancel.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let running = browsers::running();
        let cands = browsers::list();
        let mut allowed: Vec<String> = Vec::new();
        let mut rep_fail: Vec<quarantine::Fail> = Vec::new();
        for p in paths {
            match cands.iter().find(|c| c.path.eq_ignore_ascii_case(&p)) {
                None => rep_fail.push(quarantine::Fail { path: p, reason: "Not a recognised browser privacy file".into() }),
                Some(c) if running.contains(&c.browser) => rep_fail.push(quarantine::Fail { path: p, reason: format!("{} is open. Close it first.", c.browser) }),
                Some(_) => match safety::check_deletable(Path::new(&p), &protected) {
                    Ok(()) => allowed.push(p),
                    Err(r) => rep_fail.push(quarantine::Fail { path: p, reason: r }),
                },
            }
        }
        let mut rep = quarantine::remove(&allowed, "quarantine", cancel);
        log_history("browser-privacy", &format!("{} file(s) moved to Quarantine", rep.ok), rep.bytes);
        rep.failed.extend(rep_fail);
        rep
    })
    .await
    .map_err(join_err)
}

#[tauri::command]
async fn list_event_groups(days: u32) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || diag::groups(days)).await.map_err(join_err)?
}

#[tauri::command]
async fn schedule_status() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(schedule::status).await.map_err(join_err)?
}

#[tauri::command]
fn schedule_choices() -> Vec<Value> {
    schedule::choices()
}

#[tauri::command]
async fn schedule_set(freq: String, day: String, time: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || schedule::set(&freq, &day, &time)).await.map_err(join_err)?
}

#[tauri::command]
async fn schedule_remove() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(schedule::remove).await.map_err(join_err)?
}

#[tauri::command]
async fn schedule_run_now() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(schedule::auto_clean).await.map_err(join_err)
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

// ------------------------------------------------------------ recover

#[tauri::command]
async fn recycle_list() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(recover::bin_list).await.map_err(join_err)?
}

#[tauri::command]
async fn recycle_restore(paths: Vec<String>) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let r = recover::bin_restore(paths)?;
        log_history("restore", &format!("{} item(s) from the Recycle Bin", r["restored"]), 0);
        Ok(r)
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
async fn shadow_list() -> Result<Vec<recover::Shadow>, String> {
    tauri::async_runtime::spawn_blocking(|| recover::shadows(is_admin())).await.map_err(join_err)?
}

#[tauri::command]
async fn shadow_browse(device: String, path: String) -> Result<Vec<recover::ShadowEntry>, String> {
    tauri::async_runtime::spawn_blocking(move || recover::shadow_browse(&device, &path, is_admin()))
        .await
        .map_err(join_err)?
}

#[tauri::command]
async fn shadow_restore(device: String, paths: Vec<String>, dest: String) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || recover::shadow_restore(&device, &paths, &dest, is_admin()))
        .await
        .map_err(join_err)?
}

// ------------------------------------------------------------ main

fn main() {
    if std::env::args().any(|a| a == "--auto-clean") {
        schedule::auto_clean();
        return;
    }
    let settings = load_settings();
    let state = AppState {
        cancel: Arc::new(AtomicBool::new(false)),
        hogs: Arc::new(Mutex::new(Vec::new())),
        large: Arc::new(Mutex::new(Vec::new())),
        folder: Arc::new(Mutex::new(None)),
        settings: Arc::new(Mutex::new(settings.clone())),
        ai_cancels: Arc::new(Mutex::new(HashMap::new())),
        fix_cancels: Arc::new(Mutex::new(HashMap::new())),
    };
    let days = settings.quarantine_days;
    thread::spawn(move || {
        snapshot();
        if days > 0 {
            let r = quarantine::purge_expired(days);
            if r.ok > 0 {
                log_history("auto-purge", &format!("{} expired quarantine item(s)", r.ok), r.bytes);
            }
        }
    });

    tauri::Builder::default()
        .manage(state)
        .setup(|app| {
            use tauri::menu::{Menu, MenuItem};
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
            let open = MenuItem::with_id(app, "open", "Open System Care Suite", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            let mut b = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("System Care Suite")
                .on_menu_event(|app, e| match e.id.as_ref() {
                    "open" => show_main(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                        show_main(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                b = b.icon(icon.clone());
            }
            b.build(app)?;
            Ok(())
        })
        .on_window_event(|w, ev| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = ev {
                let keep = w.state::<AppState>().settings.lock().unwrap().close_to_tray;
                if keep {
                    api.prevent_close();
                    let _ = w.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            set_settings,
            drive_info_cmd,
            list_drives,
            quick_paths,
            is_admin_cmd,
            relaunch_admin,
            reveal_in_explorer,
            cancel_scan,
            get_history,
            get_snapshots,
            check_paths,
            analyze_folder,
            list_hogs,
            clean_hogs,
            run_tool,
            find_large_files,
            find_dev_junk,
            delete_paths,
            list_quarantine,
            restore_quarantine,
            purge_quarantine,
            ollama_status,
            ai_chat,
            ai_cancel,
            ai_explain,
            ai_search,
            find_duplicates,
            delete_duplicates,
            list_startup,
            set_startup,
            ai_explain_startup,
            health_check,
            list_fixes,
            run_fix,
            cancel_fix,
            recycle_list,
            recycle_restore,
            shadow_list,
            shadow_browse,
            shadow_restore,
            list_tweaks,
            set_tweak,
            list_power_plans,
            set_power_plan,
            list_installed,
            uninstall_app,
            list_bloat,
            list_updates,
            list_processes,
            end_process,
            list_browser_data,
            clean_browser,
            list_event_groups,
            schedule_status,
            schedule_choices,
            schedule_set,
            schedule_remove,
            schedule_run_now,
        ])
        .run(tauri::generate_context!())
        .expect("error while running System Care Suite");
}
