//! Startup manager: registry Run entries, Startup folders, logon scheduled tasks and
//! third-party auto-start services. Run entries and Startup-folder items are switched with the
//! same "StartupApproved" flag Task Manager uses, so nothing is deleted and Task Manager shows
//! the same state. Every change is reversible with the same switch.

use crate::ps;
use crate::util::*;
use serde::Serialize;
use std::fs;
use std::path::Path;
use winreg::enums::*;
use winreg::{RegKey, RegValue};

#[derive(Serialize, Clone)]
pub struct StartItem {
    pub id: String,
    pub name: String,
    pub command: String,
    pub source: String,
    pub kind: String,
    pub enabled: bool,
    pub exists: bool,
    pub needs_admin: bool,
    pub detail: String,
}

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN32: &str = r"Software\Wow6432Node\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

/// The program a startup command launches (quoted path, or up to the first .exe/.bat/.cmd).
fn exe_of(cmd: &str) -> String {
    let c = fx(cmd.trim());
    let c = c.trim();
    if let Some(rest) = c.strip_prefix('"') {
        return rest.split('"').next().unwrap_or("").to_string();
    }
    let low = c.to_lowercase();
    for ext in [".exe", ".bat", ".cmd", ".lnk", ".com"] {
        if let Some(i) = low.find(ext) {
            return c[..i + ext.len()].to_string();
        }
    }
    c.split_whitespace().next().unwrap_or("").to_string()
}

fn target_exists(cmd: &str) -> bool {
    let e = exe_of(cmd);
    if e.is_empty() {
        return true;
    }
    if e.contains('\\') || e.contains(':') {
        Path::new(&e).exists()
    } else {
        true // a bare command such as "cmd" resolves through PATH
    }
}

fn hive(tag: &str) -> RegKey {
    RegKey::predef(if tag == "M" { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER })
}

fn approved_enabled(h: &str, key: &str, name: &str) -> bool {
    hive(h)
        .open_subkey_with_flags(format!(r"{}\{}", APPROVED, key), KEY_READ)
        .ok()
        .and_then(|k| k.get_raw_value(name).ok())
        .map(|v| v.bytes.first().map(|b| b % 2 == 0).unwrap_or(true))
        .unwrap_or(true)
}

fn set_approved(h: &str, key: &str, name: &str, enable: bool) -> Result<(), String> {
    let (k, _) = hive(h)
        .create_subkey_with_flags(format!(r"{}\{}", APPROVED, key), KEY_READ | KEY_SET_VALUE)
        .map_err(|e| {
            if h == "M" {
                "Administrator rights are required for all-users entries (use Restart as Administrator).".to_string()
            } else {
                e.to_string()
            }
        })?;
    let mut bytes = vec![0u8; 12];
    if enable {
        bytes[0] = 2;
    } else {
        bytes[0] = 3;
        let ft = (now() + 11_644_473_600) * 10_000_000;
        bytes[4..12].copy_from_slice(&ft.to_le_bytes());
    }
    k.set_raw_value(name, &RegValue { bytes, vtype: REG_BINARY }).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- listing

pub fn list_run() -> Vec<StartItem> {
    let mut out = Vec::new();
    let srcs: [(&str, &str, &str, &str, &str, bool); 3] = [
        ("U", RUN, "Run", "r", "Registry · current user", false),
        ("M", RUN, "Run", "r", "Registry · all users", true),
        ("M", RUN32, "Run32", "r32", "Registry · all users (32-bit)", true),
    ];
    for (h, path, appr, tag, label, admin) in srcs {
        if let Ok(k) = hive(h).open_subkey_with_flags(path, KEY_READ) {
            for (name, _v) in k.enum_values().flatten() {
                let cmd: String = k.get_value(&name).unwrap_or_default();
                if name.is_empty() {
                    continue;
                }
                out.push(StartItem {
                    id: format!("run|{}|{}|{}", h, tag, name),
                    command: cmd.clone(),
                    source: label.into(),
                    kind: "run".into(),
                    enabled: approved_enabled(h, appr, &name),
                    exists: target_exists(&cmd),
                    needs_admin: admin,
                    detail: String::new(),
                    name,
                });
            }
        }
    }
    out
}

fn folder_of(h: &str) -> String {
    if h == "M" {
        fx(r"%ProgramData%\Microsoft\Windows\Start Menu\Programs\Startup")
    } else {
        fx(r"%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup")
    }
}

pub fn list_folders() -> Vec<StartItem> {
    let mut out = Vec::new();
    for (h, label, admin) in [("U", "Startup folder · current user", false), ("M", "Startup folder · all users", true)] {
        if let Ok(rd) = fs::read_dir(folder_of(h)) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.eq_ignore_ascii_case("desktop.ini") {
                    continue;
                }
                out.push(StartItem {
                    id: format!("folder|{}|{}", h, name),
                    command: e.path().to_string_lossy().to_string(),
                    source: label.into(),
                    kind: "folder".into(),
                    enabled: approved_enabled(h, "StartupFolder", &name),
                    exists: true,
                    needs_admin: admin,
                    detail: String::new(),
                    name,
                });
            }
        }
    }
    out
}

fn list_tasks() -> Vec<StartItem> {
    let script = r#"$t = @(Get-ScheduledTask -ErrorAction SilentlyContinue | Where-Object { $_.TaskPath -notlike '\Microsoft\*' -and @($_.Triggers | Where-Object { $_.CimClass.CimClassName -in 'MSFT_TaskLogonTrigger','MSFT_TaskBootTrigger' }).Count -gt 0 } | ForEach-Object { $a = @($_.Actions)[0]; [pscustomobject]@{ name=$_.TaskName; path=$_.TaskPath; state=[string]$_.State; cmd=(("$($a.Execute) $($a.Arguments)").Trim()) } }); ConvertTo-Json -InputObject $t -Compress"#;
    let v = match ps::powershell_json(script, &[], 40) {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|t| {
                    let name = t["name"].as_str().unwrap_or("").to_string();
                    let path = t["path"].as_str().unwrap_or("\\").to_string();
                    let cmd = t["cmd"].as_str().unwrap_or("").to_string();
                    StartItem {
                        id: format!("task|{}|{}", path, name),
                        exists: target_exists(&cmd),
                        command: cmd,
                        source: "Scheduled task · at logon / boot".into(),
                        kind: "task".into(),
                        enabled: t["state"].as_str().unwrap_or("") != "Disabled",
                        needs_admin: false,
                        detail: format!("Task folder {}", path),
                        name,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn list_services() -> Vec<StartItem> {
    let script = r#"$s = @(Get-CimInstance Win32_Service | Where-Object { $_.PathName -and $_.PathName -notmatch '(?i)\\Windows\\' -and $_.PathName -notmatch '(?i)svchost' } | ForEach-Object { [pscustomobject]@{ name=$_.Name; display=$_.DisplayName; state=[string]$_.State; mode=[string]$_.StartMode; cmd=[string]$_.PathName } }); ConvertTo-Json -InputObject $s -Compress"#;
    let v = match ps::powershell_json(script, &[], 40) {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|s| {
                    let name = s["name"].as_str().unwrap_or("").to_string();
                    let display = s["display"].as_str().unwrap_or("").to_string();
                    let cmd = s["cmd"].as_str().unwrap_or("").to_string();
                    let low = format!("{} {}", name, display).to_lowercase();
                    let sec = ["antivirus", "security", "defender", "firewall", "malware", "endpoint"].iter().any(|k| low.contains(k));
                    StartItem {
                        id: format!("service|{}", name),
                        exists: target_exists(&cmd),
                        command: cmd,
                        source: "Service (third-party)".into(),
                        kind: "service".into(),
                        enabled: s["mode"].as_str().unwrap_or("") == "Auto",
                        needs_admin: true,
                        detail: format!(
                            "{} · {}{}",
                            display,
                            s["state"].as_str().unwrap_or(""),
                            if sec { " · security software, usually best left on" } else { "" }
                        ),
                        name,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn list() -> Vec<StartItem> {
    let t = std::thread::spawn(list_tasks);
    let s = std::thread::spawn(list_services);
    let mut v = list_run();
    v.extend(list_folders());
    v.extend(t.join().unwrap_or_default());
    v.extend(s.join().unwrap_or_default());
    v
}

// ---------------------------------------------------------------- switching

pub fn set(id: &str, enable: bool, admin: bool) -> Result<String, String> {
    let parts: Vec<&str> = id.splitn(4, '|').collect();
    match parts.first().copied() {
        Some("run") if parts.len() == 4 => {
            let current = list_run();
            let it = current.iter().find(|i| i.id == id).ok_or("That startup entry no longer exists.")?;
            let appr = if parts[2] == "r32" { "Run32" } else { "Run" };
            set_approved(parts[1], appr, parts[3], enable)?;
            Ok(it.name.clone())
        }
        Some("folder") if parts.len() == 3 => {
            let current = list_folders();
            let it = current.iter().find(|i| i.id == id).ok_or("That startup item no longer exists.")?;
            set_approved(parts[1], "StartupFolder", parts[2], enable)?;
            Ok(it.name.clone())
        }
        Some("task") if parts.len() == 3 => {
            let current = list_tasks();
            let it = current.iter().find(|i| i.id == id).ok_or("That scheduled task no longer exists.")?;
            let verb = if enable { "Enable-ScheduledTask" } else { "Disable-ScheduledTask" };
            let script = format!("{} -TaskPath $env:CMS_TPATH -TaskName $env:CMS_TNAME | Out-Null", verb);
            ps::powershell(&script, &[("CMS_TPATH", parts[1]), ("CMS_TNAME", parts[2])], 30)
                .map_err(|e| format!("{} (some tasks need Administrator rights)", e))?;
            Ok(it.name.clone())
        }
        Some("service") if parts.len() == 2 => {
            if !admin {
                return Err("Administrator rights are required to change services (use Restart as Administrator).".into());
            }
            let current = list_services();
            let it = current.iter().find(|i| i.id == id).ok_or("That service no longer exists.")?;
            let mode = if enable { "auto" } else { "demand" };
            let o = ps::run_capture("sc.exe", &["config", &it.name, "start=", mode], &[], 20)?;
            if o.code != Some(0) {
                return Err(format!("sc.exe: {}", o.stdout.trim()));
            }
            Ok(it.name.clone())
        }
        _ => Err("Unknown startup item".into()),
    }
}
