//! Installed programs: list from the Uninstall registry keys (what "Apps & features" reads),
//! launch the program's own uninstaller, and a hard-coded allowlist of removable Microsoft
//! Store apps ("debloat"). Nothing here takes command text from the UI: the UI sends an id
//! that is looked up again in a fresh listing.

use crate::ps;
use crate::util::*;
use serde::Serialize;
use serde_json::{json, Value};
use std::os::windows::process::CommandExt;
use std::process::Command;
use tauri::{AppHandle, Emitter};
use winreg::enums::*;
use winreg::RegKey;

#[derive(Serialize, Clone)]
pub struct App {
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub version: String,
    pub installed: String,
    pub size: u64,
    pub location: String,
    pub quiet: bool,
    #[serde(skip)]
    pub uninstall: String,
    #[serde(skip)]
    pub quiet_cmd: String,
}

const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
const UNINSTALL32: &str = r"Software\Wow6432Node\Microsoft\Windows\CurrentVersion\Uninstall";

fn s(k: &RegKey, n: &str) -> String {
    k.get_value::<String, _>(n).unwrap_or_default().trim().to_string()
}

pub fn list_installed() -> Vec<App> {
    let mut out: Vec<App> = Vec::new();
    for (h, loc, path) in [("U", "64", UNINSTALL), ("M", "64", UNINSTALL), ("M", "32", UNINSTALL32)] {
        let root = RegKey::predef(if h == "M" { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER });
        let base = match root.open_subkey_with_flags(path, KEY_READ) {
            Ok(b) => b,
            Err(_) => continue,
        };
        for sub in base.enum_keys().flatten() {
            let k = match base.open_subkey_with_flags(&sub, KEY_READ) {
                Ok(k) => k,
                Err(_) => continue,
            };
            let name = s(&k, "DisplayName");
            let un = s(&k, "UninstallString");
            let quiet = s(&k, "QuietUninstallString");
            if name.is_empty() || (un.is_empty() && quiet.is_empty()) {
                continue;
            }
            if k.get_value::<u32, _>("SystemComponent").unwrap_or(0) == 1 || !s(&k, "ParentKeyName").is_empty() {
                continue;
            }
            let rel = s(&k, "ReleaseType").to_lowercase();
            if rel.contains("update") || rel.contains("hotfix") {
                continue;
            }
            let d = s(&k, "InstallDate");
            let installed = if d.len() == 8 && d.chars().all(|c| c.is_ascii_digit()) { format!("{}-{}-{}", &d[0..4], &d[4..6], &d[6..8]) } else { String::new() };
            out.push(App {
                id: format!("{}|{}|{}", h, loc, sub),
                name,
                publisher: s(&k, "Publisher"),
                version: s(&k, "DisplayVersion"),
                installed,
                size: k.get_value::<u32, _>("EstimatedSize").unwrap_or(0) as u64 * 1024,
                location: s(&k, "InstallLocation"),
                quiet: !quiet.is_empty(),
                uninstall: un,
                quiet_cmd: quiet,
            });
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out.dedup_by(|a, b| a.name == b.name && a.version == b.version && a.publisher == b.publisher);
    out
}

/// Starts the program's own uninstaller. Emits `uninstall-done` {id, code} when it exits.
pub fn uninstall(app: AppHandle, id: &str, quiet: bool) -> Result<String, String> {
    let all = list_installed();
    let it = all.iter().find(|a| a.id == id).ok_or("That program is no longer listed.")?.clone();
    let mut cmdline = if quiet && !it.quiet_cmd.is_empty() { it.quiet_cmd.clone() } else if !it.uninstall.is_empty() { it.uninstall.clone() } else { it.quiet_cmd.clone() };
    // MSI entries often say /I (modify); /X is the uninstall switch
    let low = cmdline.to_lowercase();
    if low.contains("msiexec") && low.contains("/i{") {
        cmdline = cmdline.replacen("/I{", "/X{", 1).replacen("/i{", "/X{", 1);
    }
    let mut c = Command::new("cmd.exe");
    c.arg("/D").arg("/C").raw_arg(format!("\"{}\"", cmdline)).creation_flags(CREATE_NO_WINDOW);
    let mut child = c.spawn().map_err(|e| e.to_string())?;
    let name = it.name.clone();
    let rid = id.to_string();
    std::thread::spawn(move || {
        let code = child.wait().ok().and_then(|s| s.code());
        log_history("uninstall", &format!("{} (exit {:?})", name, code), 0);
        let _ = app.emit("uninstall-done", json!({"id": rid, "code": code}));
    });
    Ok(it.name)
}

// ---------------------------------------------------------------- Store app debloat

/// (package name, label, note, caution). Only these can ever be removed from the UI.
pub const BLOAT: &[(&str, &str, &str, bool)] = &[
    ("Microsoft.BingNews", "Microsoft News", "News feed app.", false),
    ("Microsoft.BingWeather", "Weather", "Weather app. The taskbar weather widget is separate.", false),
    ("Microsoft.BingSearch", "Bing Search", "Search helper app.", false),
    ("Microsoft.GetHelp", "Get Help", "Support chat app.", false),
    ("Microsoft.Getstarted", "Tips", "Windows tips app.", false),
    ("Microsoft.MicrosoftOfficeHub", "Microsoft 365 (Office hub)", "Only a launcher/ad page. Installed Office apps are not affected.", false),
    ("Microsoft.MicrosoftSolitaireCollection", "Solitaire Collection", "Card games with ads.", false),
    ("Microsoft.WindowsFeedbackHub", "Feedback Hub", "Sends feedback to Microsoft.", false),
    ("Microsoft.People", "People", "Contacts app.", false),
    ("Microsoft.WindowsMaps", "Maps", "Offline maps app.", false),
    ("Microsoft.MixedReality.Portal", "Mixed Reality Portal", "Only needed with a VR headset.", false),
    ("Microsoft.Microsoft3DViewer", "3D Viewer", "Opens 3D model files.", false),
    ("Microsoft.PowerAutomateDesktop", "Power Automate", "Automation tool. Remove only if you do not use it.", false),
    ("Microsoft.549981C3F5F10", "Cortana", "Old voice assistant.", false),
    ("Clipchamp.Clipchamp", "Clipchamp", "Video editor. Remove only if you do not use it.", false),
    ("MicrosoftTeams", "Microsoft Teams (personal)", "Chat app that auto-starts. Work Teams is a different program.", false),
    ("Microsoft.YourPhone", "Phone Link", "Remove if you do not link an Android/iPhone.", true),
    ("Microsoft.ZuneVideo", "Movies & TV", "Video player. Check you do not rely on it.", true),
    ("Microsoft.ZuneMusic", "Media Player / Groove", "On newer Windows this is the Media Player app. Check you do not rely on it.", true),
    ("Microsoft.XboxGamingOverlay", "Xbox Game Bar", "Used by some game capture overlays. Keep it if you use Win+G.", true),
    ("Microsoft.XboxIdentityProvider", "Xbox Identity Provider", "Needed to sign in to Xbox / some Store games.", true),
    ("Microsoft.GamingApp", "Xbox app", "Needed for Game Pass and some Store games.", true),
];

pub fn bloat_label(name: &str) -> Option<&'static (&'static str, &'static str, &'static str, bool)> {
    BLOAT.iter().find(|b| b.0 == name)
}

pub fn list_bloat() -> Result<Value, String> {
    let names: Vec<&str> = BLOAT.iter().map(|b| b.0).collect();
    let env = serde_json::to_string(&names).map_err(|e| e.to_string())?;
    let v = ps::powershell_json(
        "$n=@(ConvertFrom-Json $env:CMS_NAMES | ForEach-Object { $_ }); $r=@(Get-AppxPackage | Where-Object { $n -contains $_.Name -and -not $_.NonRemovable } | ForEach-Object { [pscustomobject]@{ name=[string]$_.Name; version=[string]$_.Version } }); ConvertTo-Json -InputObject $r -Compress",
        &[("CMS_NAMES", &env)],
        60,
    )?;
    let mut out = Vec::new();
    for it in v.as_array().cloned().unwrap_or_default() {
        let n = it["name"].as_str().unwrap_or("");
        if let Some(b) = bloat_label(n) {
            out.push(json!({"name": n, "label": b.1, "note": b.2, "caution": b.3, "version": it["version"]}));
        }
    }
    out.sort_by(|a, b| a["caution"].as_bool().cmp(&b["caution"].as_bool()).then(a["label"].as_str().cmp(&b["label"].as_str())));
    Ok(Value::Array(out))
}
