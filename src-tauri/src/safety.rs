//! Hard safety rules. Nothing the UI or the AI says can override these.

use crate::util::*;
use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::Path;

fn norm(p: &Path) -> String {
    let mut s = p.to_string_lossy().replace('/', "\\").to_lowercase();
    if let Some(r) = s.strip_prefix(r"\\?\") {
        s = r.to_string();
    }
    while s.len() > 3 && s.ends_with('\\') {
        s.pop();
    }
    s
}

fn under(n: &str, base: &str) -> bool {
    n == base || n.starts_with(&format!("{}\\", base))
}

/// Returns Some(reason) when the path must never be removed through the app.
pub fn denied_reason(p: &Path, protected: &[String]) -> Option<String> {
    let n = norm(p);
    if n.len() <= 3 {
        return Some("Drive roots can never be removed".into());
    }
    let sd = std::env::var("SystemDrive")
        .unwrap_or_else(|_| "C:".into())
        .to_lowercase();

    for b in [
        "windows",
        "program files",
        "program files (x86)",
        "programdata\\microsoft",
        "$recycle.bin",
        "system volume information",
        "recovery",
        "boot",
        "efi",
        "users\\default",
        "users\\all users",
    ] {
        if under(&n, &format!("{}\\{}", sd, b)) {
            return Some(format!("Protected system location ({}\\{})", sd.to_uppercase(), b));
        }
    }
    if under(&n, &format!("{}\\windows.old", sd)) {
        return Some("Use Windows Storage Sense to remove Windows.old safely".into());
    }

    if let Some(name) = p.file_name().map(|x| x.to_string_lossy().to_lowercase()) {
        if ["hiberfil.sys", "pagefile.sys", "swapfile.sys", "bootmgr", "desktop.ini"]
            .contains(&name.as_str())
            || name.starts_with("ntuser.dat")
        {
            return Some("Core Windows file".into());
        }
    }

    let prof = norm(&profile());
    if !prof.is_empty() {
        // Anything that contains the user profile (e.g. C:\Users) is off limits.
        if under(&prof, &n) {
            return Some("Contains your user profile".into());
        }
        for e in [
            "",
            "\\desktop",
            "\\documents",
            "\\downloads",
            "\\pictures",
            "\\music",
            "\\videos",
            "\\appdata",
            "\\appdata\\local",
            "\\appdata\\roaming",
            "\\appdata\\locallow",
            "\\appdata\\local\\microsoft",
            "\\appdata\\local\\packages",
            "\\onedrive",
            "\\contacts",
            "\\favorites",
            "\\links",
            "\\saved games",
            "\\searches",
        ] {
            if n == format!("{}{}", prof, e) {
                return Some("Core user folder: remove items inside it, not the folder itself".into());
            }
        }
    }

    if under(&n, &norm(&data_dir())) {
        return Some("Clean My System's own data folder".into());
    }

    for pr in protected {
        let pn = norm(Path::new(&fx(pr)));
        if pn.len() > 3 && (under(&n, &pn) || under(&pn, &n)) {
            return Some(format!("Protected by you in Settings ({})", pr));
        }
    }
    None
}

pub fn check_deletable(p: &Path, protected: &[String]) -> Result<(), String> {
    if !p.is_absolute() {
        return Err("Path must be absolute".into());
    }
    let md = fs::symlink_metadata(p).map_err(|e| format!("Cannot access: {}", e))?;
    if is_reparse(md.file_attributes()) && is_dir_attr(md.file_attributes()) {
        return Err("Junctions and symlinks are not removed through the app".into());
    }
    match denied_reason(p, protected) {
        Some(r) => Err(r),
        None => Ok(()),
    }
}
