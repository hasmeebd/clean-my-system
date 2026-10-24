//! Reversible Windows tweaks. Every tweak is a fixed list of registry values (or one fixed
//! command). Before a value is changed its original is saved, and "Undo" puts that original
//! back, so nothing here is one-way. No tweak claims a speed gain it cannot prove: each one
//! says plainly what it changes.

use crate::util::*;
use crate::ps;
use serde_json::{json, Value};
use std::collections::HashMap;
use winreg::enums::*;
use winreg::RegKey;

struct Reg {
    hive: &'static str,
    key: &'static str,
    name: &'static str,
    on: u32,
    /// value to write on undo when no backup exists (None = delete the value)
    default: Option<u32>,
}

enum Kind {
    Reg(Vec<Reg>),
    Hibernate,
}

struct Def {
    id: &'static str,
    group: &'static str,
    title: &'static str,
    desc: &'static str,
    caution: &'static str,
    machine: bool,
    kind: Kind,
}

fn r(hive: &'static str, key: &'static str, name: &'static str, on: u32, default: Option<u32>) -> Reg {
    Reg { hive, key, name, on, default }
}

const CDM: &str = r"Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager";
const ADV: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";

fn defs() -> Vec<Def> {
    vec![
        Def { id: "suggestions", group: "Less clutter", title: "Turn off Start menu suggestions and silent app installs", desc: "Stops Windows from showing promoted apps in Start and Settings, and from installing sponsored apps in the background.", caution: "", machine: false,
            kind: Kind::Reg(vec![
                r("U", CDM, "SystemPaneSuggestionsEnabled", 0, None), r("U", CDM, "SilentInstalledAppsEnabled", 0, None), r("U", CDM, "PreInstalledAppsEnabled", 0, None),
                r("U", CDM, "OemPreInstalledAppsEnabled", 0, None), r("U", CDM, "SoftLandingEnabled", 0, None), r("U", CDM, "SubscribedContent-338388Value", 0, None),
                r("U", CDM, "SubscribedContent-338389Value", 0, None), r("U", CDM, "SubscribedContent-353694Value", 0, None), r("U", CDM, "SubscribedContent-353696Value", 0, None),
            ]) },
        Def { id: "lockscreen_ads", group: "Less clutter", title: "Turn off lock-screen tips and spotlight overlays", desc: "Removes the \"fun facts and tips\" text on the lock screen.", caution: "", machine: false,
            kind: Kind::Reg(vec![r("U", CDM, "RotatingLockScreenOverlayEnabled", 0, None), r("U", CDM, "SubscribedContent-338387Value", 0, None)]) },
        Def { id: "welcome_tips", group: "Less clutter", title: "Turn off \"Get even more out of Windows\" prompts", desc: "Stops the welcome and set-up-your-device screens that appear after updates.", caution: "", machine: false,
            kind: Kind::Reg(vec![r("U", r"Software\Microsoft\Windows\CurrentVersion\UserProfileEngagement", "ScoobeSystemSettingEnabled", 0, None), r("U", CDM, "SubscribedContent-310093Value", 0, None)]) },
        Def { id: "bing_search", group: "Privacy", title: "Keep Start search local (no web results)", desc: "Start search shows only your apps and files, and stops sending what you type to Bing.", caution: "", machine: false,
            kind: Kind::Reg(vec![r("U", r"Software\Microsoft\Windows\CurrentVersion\Search", "BingSearchEnabled", 0, None), r("U", r"Software\Policies\Microsoft\Windows\Explorer", "DisableSearchBoxSuggestions", 1, None)]) },
        Def { id: "ad_id", group: "Privacy", title: "Turn off the advertising ID", desc: "Apps can no longer use a per-user ID to personalise ads.", caution: "", machine: false,
            kind: Kind::Reg(vec![r("U", r"Software\Microsoft\Windows\CurrentVersion\AdvertisingInfo", "Enabled", 0, None)]) },
        Def { id: "bg_apps", group: "Background activity", title: "Stop Store apps running in the background", desc: "Store apps only run while you use them. Fewer background processes; notifications from those apps may arrive later.", caution: "Some Store apps will not refresh or notify until opened.", machine: false,
            kind: Kind::Reg(vec![r("U", r"Software\Microsoft\Windows\CurrentVersion\BackgroundAccessApplications", "GlobalUserDisabled", 1, None)]) },
        Def { id: "game_dvr", group: "Background activity", title: "Turn off Game DVR background recording", desc: "Xbox Game Bar stops recording gameplay in the background.", caution: "Disables Game Bar clip capture (Win+Alt+R). Other recorders such as Bandicam are not affected.", machine: false,
            kind: Kind::Reg(vec![r("U", r"System\GameConfigStore", "GameDVR_Enabled", 0, None), r("U", r"Software\Microsoft\Windows\CurrentVersion\GameDVR", "AppCaptureEnabled", 0, None)]) },
        Def { id: "startup_delay", group: "Startup", title: "Remove the delay before startup apps launch", desc: "Windows waits about 10 seconds after the desktop appears before starting your startup apps. This lets them start at once. Total load stays the same.", caution: "Apps all start together, so the desktop can feel busy for a moment right after sign-in.", machine: false,
            kind: Kind::Reg(vec![r("U", r"Software\Microsoft\Windows\CurrentVersion\Explorer\Serialize", "StartupDelayInMSec", 0, None)]) },
        Def { id: "file_ext", group: "Convenience", title: "Always show file extensions", desc: "Shows .exe, .pdf and so on. Helps spot disguised files such as invoice.pdf.exe.", caution: "", machine: false,
            kind: Kind::Reg(vec![r("U", ADV, "HideFileExt", 0, Some(1))]) },
        Def { id: "hibernate", group: "Disk space", title: "Turn off hibernation (deletes hiberfil.sys)", desc: "hiberfil.sys is about the size of a large part of your RAM and is rewritten on every hibernate. Turning hibernation off frees that space at once.", caution: "Also disables Fast Startup and Hibernate. Turn it on again here at any time (the file is recreated).", machine: true, kind: Kind::Hibernate },
    ]
}

fn hive(tag: &str) -> RegKey {
    RegKey::predef(if tag == "M" { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER })
}

fn read(h: &str, key: &str, name: &str) -> Option<u32> {
    hive(h).open_subkey_with_flags(key, KEY_READ).ok()?.get_value::<u32, _>(name).ok()
}

fn write(h: &str, key: &str, name: &str, v: Option<u32>) -> Result<(), String> {
    let (k, _) = hive(h).create_subkey_with_flags(key, KEY_READ | KEY_SET_VALUE).map_err(|e| e.to_string())?;
    match v {
        Some(x) => k.set_value(name, &x).map_err(|e| e.to_string()),
        None => match k.delete_value(name) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        },
    }
}

fn applied(d: &Def) -> bool {
    match &d.kind {
        Kind::Reg(sets) => sets.iter().all(|s| read(s.hive, s.key, s.name) == Some(s.on)),
        Kind::Hibernate => read("M", r"SYSTEM\CurrentControlSet\Control\Power", "HibernateEnabled") == Some(0),
    }
}

fn backup_path() -> std::path::PathBuf {
    data_dir().join("tweaks-backup.json")
}

fn load_backup() -> HashMap<String, Vec<Option<u32>>> {
    std::fs::read_to_string(backup_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_backup(b: &HashMap<String, Vec<Option<u32>>>) {
    let _ = std::fs::create_dir_all(data_dir());
    let _ = std::fs::write(backup_path(), serde_json::to_string_pretty(b).unwrap_or_default());
}

pub fn list() -> Value {
    let hib = std::fs::metadata(format!("{}\\hiberfil.sys", std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()))).map(|m| m.len()).unwrap_or(0);
    let tw: Vec<Value> = defs()
        .iter()
        .map(|d| json!({"id": d.id, "group": d.group, "title": d.title, "desc": d.desc, "caution": d.caution, "machine": d.machine, "applied": applied(d)}))
        .collect();
    json!({"tweaks": tw, "hiber_bytes": hib})
}

pub fn set(id: &str, on: bool, admin: bool) -> Result<String, String> {
    let all = defs();
    let d = all.iter().find(|d| d.id == id).ok_or("Unknown tweak")?;
    if d.machine && !admin {
        return Err("This tweak needs Administrator rights (use Restart as Administrator).".into());
    }
    match &d.kind {
        Kind::Reg(sets) => {
            let mut bk = load_backup();
            if on {
                if !applied(d) && !bk.contains_key(id) {
                    bk.insert(id.to_string(), sets.iter().map(|s| read(s.hive, s.key, s.name)).collect());
                    save_backup(&bk);
                }
                for s in sets {
                    write(s.hive, s.key, s.name, Some(s.on))?;
                }
            } else {
                let saved = bk.get(id).cloned();
                for (i, s) in sets.iter().enumerate() {
                    let v = match &saved {
                        Some(o) if o.len() == sets.len() => o[i],
                        _ => s.default,
                    };
                    write(s.hive, s.key, s.name, v)?;
                }
                bk.remove(id);
                save_backup(&bk);
            }
        }
        Kind::Hibernate => {
            let o = ps::run_capture("powercfg", &["/hibernate", if on { "off" } else { "on" }], &[], 30)?;
            if o.code != Some(0) {
                return Err(format!("powercfg: {}", o.stdout.trim()));
            }
        }
    }
    log_history("tweak", &format!("{}: {}", d.title, if on { "applied" } else { "undone" }), 0);
    Ok(d.title.to_string())
}

// ---------------------------------------------------------------- power plans

fn guid_in(line: &str) -> Option<String> {
    line.split_whitespace().map(|t| t.trim_matches(|c: char| !c.is_ascii_hexdigit() && c != '-')).find(|t| t.len() == 36 && t.matches('-').count() == 4).map(|t| t.to_lowercase())
}

pub fn power_plans() -> Result<Vec<Value>, String> {
    let o = ps::run_capture("powercfg", &["/list"], &[], 15)?;
    let mut out = Vec::new();
    for l in o.stdout.lines() {
        if let Some(g) = guid_in(l) {
            let name = l.find('(').and_then(|a| l.rfind(')').map(|b| l[a + 1..b].to_string())).unwrap_or_else(|| g.clone());
            out.push(json!({"guid": g, "name": name, "active": l.trim_end().ends_with('*')}));
        }
    }
    Ok(out)
}

pub fn set_power_plan(guid: &str) -> Result<(), String> {
    let g = guid.to_lowercase();
    let plans = power_plans()?;
    let p = plans.iter().find(|p| p["guid"].as_str() == Some(g.as_str())).ok_or("That power plan does not exist.")?;
    let o = ps::run_capture("powercfg", &["/setactive", &g], &[], 15)?;
    if o.code != Some(0) {
        return Err(format!("powercfg: {}", o.stdout.trim()));
    }
    log_history("power", &format!("Power plan: {}", p["name"].as_str().unwrap_or("")), 0);
    Ok(())
}
