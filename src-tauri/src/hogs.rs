//! Allowlist of known junk locations. The UI can only pick items by id; the
//! paths below are the only ones the cleaner will ever touch.

use crate::util::*;
use rayon::prelude::*;
use serde::Serialize;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use windows_sys::Win32::UI::Shell::{SHEmptyRecycleBinW, SHQueryRecycleBinW, SHQUERYRBINFO};

#[derive(Clone)]
pub struct HogDef {
    pub id: &'static str,
    pub name: &'static str,
    pub group: &'static str,
    pub paths: Vec<String>,
    pub risk: &'static str,
    pub auto: bool,
    pub admin: bool,
    pub services: Vec<&'static str>,
    pub tip: &'static str,
}

#[allow(clippy::too_many_arguments)]
fn d(
    id: &'static str,
    name: &'static str,
    group: &'static str,
    paths: &[&str],
    risk: &'static str,
    auto: bool,
    admin: bool,
    services: &[&'static str],
    tip: &'static str,
) -> HogDef {
    HogDef {
        id,
        name,
        group,
        paths: paths.iter().map(|s| s.to_string()).collect(),
        risk,
        auto,
        admin,
        services: services.to_vec(),
        tip,
    }
}

pub fn defs() -> Vec<HogDef> {
    vec![
        // ---- Windows
        d("user_temp", "User Temp files", "Windows", &["%LOCALAPPDATA%\\Temp"], "safe", true, false, &[], "Temporary files left by apps and installers."),
        d("win_temp", "Windows Temp", "Windows", &["%SystemRoot%\\Temp"], "safe", true, true, &[], "System-wide temporary files."),
        d("wu_download", "Windows Update downloads", "Windows", &["%SystemRoot%\\SoftwareDistribution\\Download"], "safe", true, true, &["wuauserv", "bits"], "Downloaded update packages. Windows re-downloads if needed."),
        d("delivery_opt", "Delivery Optimization cache", "Windows", &["%SystemRoot%\\ServiceProfiles\\NetworkService\\AppData\\Local\\Microsoft\\Windows\\DeliveryOptimization\\Cache"], "safe", true, true, &["dosvc"], "Peer-to-peer update cache."),
        d("wer", "Windows Error Reports", "Windows", &["%ProgramData%\\Microsoft\\Windows\\WER"], "safe", true, true, &[], "Crash report archives."),
        d("crash_dumps", "Crash dumps (user)", "Windows", &["%LOCALAPPDATA%\\CrashDumps"], "safe", true, false, &[], "Application crash dumps."),
        d("minidump", "Minidumps", "Windows", &["%SystemRoot%\\Minidump", "%SystemRoot%\\LiveKernelReports"], "safe", true, true, &[], "Blue-screen and kernel dump files."),
        d("windows_old", "Windows.old (previous Windows)", "Windows", &["%SystemDrive%\\Windows.old"], "review", false, true, &[], "Remove it with Tools > Storage Sense. Cannot be restored afterwards."),
        // ---- Browsers (cache only: never cookies, history or passwords)
        d("chrome_cache", "Chrome cache", "Browsers", &["%LOCALAPPDATA%\\Google\\Chrome\\User Data\\*\\Cache", "%LOCALAPPDATA%\\Google\\Chrome\\User Data\\*\\Code Cache", "%LOCALAPPDATA%\\Google\\Chrome\\User Data\\*\\GPUCache", "%LOCALAPPDATA%\\Google\\Chrome\\User Data\\ShaderCache"], "safe", true, false, &[], "Close Chrome first. Sign-ins and history are not touched."),
        d("edge_cache", "Edge cache", "Browsers", &["%LOCALAPPDATA%\\Microsoft\\Edge\\User Data\\*\\Cache", "%LOCALAPPDATA%\\Microsoft\\Edge\\User Data\\*\\Code Cache", "%LOCALAPPDATA%\\Microsoft\\Edge\\User Data\\*\\GPUCache", "%LOCALAPPDATA%\\Microsoft\\Edge\\User Data\\ShaderCache"], "safe", true, false, &[], "Close Edge first. Sign-ins and history are not touched."),
        d("firefox_cache", "Firefox cache", "Browsers", &["%LOCALAPPDATA%\\Mozilla\\Firefox\\Profiles\\*\\cache2", "%LOCALAPPDATA%\\Mozilla\\Firefox\\Profiles\\*\\startupCache"], "safe", true, false, &[], "Close Firefox first. Sign-ins and history are not touched."),
        // ---- GPU / graphics
        d("d3d_cache", "DirectX shader cache", "Graphics", &["%LOCALAPPDATA%\\D3DSCache"], "safe", true, false, &[], "Rebuilds automatically (games may stutter once)."),
        d("nvidia_dx", "NVIDIA DX cache", "Graphics", &["%LOCALAPPDATA%\\NVIDIA\\DXCache"], "safe", true, false, &[], "Rebuilds automatically."),
        d("nvidia_gl", "NVIDIA GL cache", "Graphics", &["%LOCALAPPDATA%\\NVIDIA\\GLCache", "%ProgramData%\\NVIDIA Corporation\\NV_Cache"], "safe", true, false, &[], "Rebuilds automatically."),
        d("amd_dx", "AMD DX cache", "Graphics", &["%LOCALAPPDATA%\\AMD\\DxCache"], "safe", true, false, &[], "Rebuilds automatically."),
        // ---- Developer
        d("pip_cache", "pip cache", "Developer", &["%LOCALAPPDATA%\\pip\\Cache"], "safe", true, false, &[], "Downloaded Python wheels. Re-downloaded on demand."),
        d("uv_cache", "uv cache", "Developer", &["%LOCALAPPDATA%\\uv\\cache"], "safe", true, false, &[], "Python package cache for uv."),
        d("npm_cache", "npm cache", "Developer", &["%LOCALAPPDATA%\\npm-cache", "%APPDATA%\\npm-cache"], "safe", true, false, &[], "Downloaded npm tarballs."),
        d("nuget_cache", "NuGet cache", "Developer", &["%LOCALAPPDATA%\\NuGet\\v3-cache", "%LOCALAPPDATA%\\NuGet\\Cache"], "safe", true, false, &[], "Downloaded NuGet packages."),
        d("yarn_cache", "Yarn cache", "Developer", &["%LOCALAPPDATA%\\Yarn\\Cache"], "safe", true, false, &[], "Downloaded Yarn packages."),
        d("cargo_registry", "Cargo registry (downloaded crates)", "Developer", &["%USERPROFILE%\\.cargo\\registry\\cache", "%USERPROFILE%\\.cargo\\registry\\src"], "review", true, false, &[], "Next build re-downloads crates (needs internet)."),
        d("gradle_cache", "Gradle caches", "Developer", &["%USERPROFILE%\\.gradle\\caches"], "review", true, false, &[], "Next Android/Gradle build is slower and re-downloads."),
        d("unity_cache", "Unity asset cache", "Developer", &["%LOCALAPPDATA%\\Unity\\cache"], "review", true, false, &[], "Close Unity first."),
        d("docker", "Docker Desktop data", "Developer", &["%LOCALAPPDATA%\\Docker"], "info", false, false, &[], "Use 'docker system prune' or Docker Desktop settings."),
        d("wsl_packages", "Store apps / WSL distros", "Developer", &["%LOCALAPPDATA%\\Packages"], "info", false, false, &[], "WSL .vhdx files live here. Compact or remove distros from WSL, not by hand."),
        // ---- AI & models (info only)
        d("hf_cache", "Hugging Face cache", "AI & Models", &["%USERPROFILE%\\.cache\\huggingface"], "review", false, false, &[], "Downloaded models. Open in Explorer to pick what to remove."),
        d("ollama_models", "Ollama models", "AI & Models", &["%USERPROFILE%\\.ollama"], "info", false, false, &[], "Use 'ollama rm <model>' to remove a model properly."),
        // ---- Other
        d("downloads", "Downloads folder", "Other", &["%USERPROFILE%\\Downloads"], "review", false, false, &[], "Your own files. Review in Explorer or Large Files."),
    ]
}

pub fn expand(t: &str) -> Vec<PathBuf> {
    let s = fx(t);
    let mut comps = s.split('\\').filter(|c| !c.is_empty());
    let first = match comps.next() {
        Some(c) if c.contains(':') => c,
        _ => return vec![],
    };
    let mut cur: Vec<PathBuf> = vec![PathBuf::from(format!("{}\\", first))];
    for c in comps {
        if c == "*" {
            let mut next = Vec::new();
            for p in &cur {
                if let Ok(rd) = std::fs::read_dir(p) {
                    for e in rd.flatten() {
                        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                            next.push(e.path());
                        }
                    }
                }
            }
            cur = next;
        } else {
            for p in cur.iter_mut() {
                p.push(c);
            }
        }
    }
    cur.into_iter().filter(|p| p.exists()).collect()
}

#[derive(Serialize, Clone)]
pub struct HogInfo {
    pub id: String,
    pub name: String,
    pub group: String,
    pub risk: String,
    pub auto: bool,
    pub admin: bool,
    pub tip: String,
    pub size: u64,
    pub files: u64,
    pub paths: Vec<String>,
}

fn recycle_info() -> (u64, u64) {
    unsafe {
        let mut info: SHQUERYRBINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHQUERYRBINFO>() as u32;
        let hr = SHQueryRecycleBinW(std::ptr::null(), &mut info);
        if hr < 0 {
            return (0, 0);
        }
        (info.i64Size as u64, info.i64NumItems as u64)
    }
}

pub fn scan(ctx: &Ctx) -> Vec<HogInfo> {
    let mut v: Vec<HogInfo> = defs()
        .par_iter()
        .map(|d| {
            let mut size = 0u64;
            let mut files = 0u64;
            let mut paths = Vec::new();
            for t in &d.paths {
                for p in expand(t) {
                    let a = walk(&p, ctx);
                    size += a.alloc;
                    files += a.files;
                    paths.push(p.to_string_lossy().to_string());
                }
            }
            HogInfo {
                id: d.id.into(),
                name: d.name.into(),
                group: d.group.into(),
                risk: d.risk.into(),
                auto: d.auto,
                admin: d.admin,
                tip: d.tip.into(),
                size,
                files,
                paths,
            }
        })
        .filter(|h| !h.paths.is_empty())
        .collect();

    let (rb_size, rb_items) = recycle_info();
    v.push(HogInfo {
        id: "recycle_bin".into(),
        name: "Recycle Bin".into(),
        group: "Windows".into(),
        risk: "safe".into(),
        auto: true,
        admin: false,
        tip: "Files you already deleted. Emptying is permanent.".into(),
        size: rb_size,
        files: rb_items,
        paths: vec!["Recycle Bin".into()],
    });
    v.sort_by(|a, b| b.size.cmp(&a.size));
    v
}

#[derive(Serialize)]
pub struct CleanItem {
    pub id: String,
    pub name: String,
    pub freed: u64,
    pub failed: u64,
    pub note: String,
}

fn svc(action: &str, name: &str) {
    let _ = Command::new("net")
        .args([action, name])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
}

pub fn clean(ids: &[String], cancel: &AtomicBool) -> Vec<CleanItem> {
    let admin = is_admin();
    let table = defs();
    let mut out = Vec::new();
    for id in ids {
        if id == "recycle_bin" {
            let (before, _) = recycle_info();
            let hr = unsafe { SHEmptyRecycleBinW(std::ptr::null_mut(), std::ptr::null(), 7) };
            out.push(CleanItem {
                id: id.clone(),
                name: "Recycle Bin".into(),
                freed: if hr >= 0 { before } else { 0 },
                failed: if hr >= 0 { 0 } else { 1 },
                note: if hr >= 0 { "Emptied".into() } else { "Could not empty (already empty?)".into() },
            });
            continue;
        }
        let def = match table.iter().find(|x| x.id == id.as_str()) {
            Some(x) => x,
            None => {
                out.push(CleanItem { id: id.clone(), name: id.clone(), freed: 0, failed: 0, note: "Unknown id, ignored".into() });
                continue;
            }
        };
        if !def.auto {
            out.push(CleanItem { id: id.clone(), name: def.name.into(), freed: 0, failed: 0, note: "Not an automatic target, ignored".into() });
            continue;
        }
        if def.admin && !admin {
            out.push(CleanItem { id: id.clone(), name: def.name.into(), freed: 0, failed: 0, note: "Needs Administrator rights, skipped".into() });
            continue;
        }
        for s in &def.services {
            svc("stop", s);
        }
        let mut freed = 0u64;
        let mut failed = 0u64;
        for t in &def.paths {
            for p in expand(t) {
                let (b, f) = remove_contents(&p, cancel);
                freed += b;
                failed += f;
            }
        }
        for s in &def.services {
            svc("start", s);
        }
        let note = if failed > 0 {
            format!("{} item(s) were in use and skipped", failed)
        } else {
            "Done".into()
        };
        out.push(CleanItem { id: id.clone(), name: def.name.into(), freed, failed, note });
    }
    out
}
