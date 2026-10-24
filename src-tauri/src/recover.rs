//! File recovery, the safe layers: the Recycle Bin (list and restore through the shell, exactly
//! like Explorer does) and Previous Versions (read-only browsing of Volume Shadow Copies, with
//! copy-out that never overwrites anything).

use crate::ps;
use crate::util::*;
use serde::Serialize;
use serde_json::{json, Value};
use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

// ---------------------------------------------------------------- recycle bin

const BIN_LIST: &str = r#"$sh = New-Object -ComObject Shell.Application; $rb = $sh.NameSpace(10); $items = @($rb.Items() | ForEach-Object { [pscustomobject]@{ name=[string]$_.Name; orig=[string]$rb.GetDetailsOf($_,1); deleted=[string]$rb.GetDetailsOf($_,2); size=[int64]$_.Size; path=[string]$_.Path; isdir=[bool]$_.IsFolder } }); ConvertTo-Json -InputObject $items -Compress"#;

const BIN_RESTORE: &str = r#"$want = @(ConvertFrom-Json $env:CMS_PATHS | ForEach-Object { $_ }); $sh = New-Object -ComObject Shell.Application; $rb = $sh.NameSpace(10); foreach($it in @($rb.Items())){ if($want -contains [string]$it.Path){ try { $v = @($it.Verbs() | Where-Object { ($_.Name -replace '&','') -match '^(Restore|Undelete)' })[0]; if($v){ $v.DoIt() } else { $it.InvokeVerb('undelete') } } catch {} } }"#;

pub fn bin_list() -> Result<Value, String> {
    let mut v = ps::powershell_json(BIN_LIST, &[], 120)?;
    // the shell inserts invisible direction marks into the date text
    if let Some(a) = v.as_array_mut() {
        for it in a.iter_mut() {
            for k in ["orig", "deleted"] {
                if let Some(s) = it[k].as_str() {
                    let clean: String = s.chars().filter(|c| !matches!(*c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}')).collect();
                    it[k] = json!(clean);
                }
            }
        }
    }
    Ok(v)
}

pub fn bin_restore(paths: Vec<String>) -> Result<Value, String> {
    let wanted = paths.len();
    for chunk in paths.chunks(100) {
        let json = serde_json::to_string(chunk).map_err(|e| e.to_string())?;
        ps::powershell(BIN_RESTORE, &[("CMS_PATHS", &json)], 180)?;
    }
    let after = bin_list()?;
    let still: usize = after
        .as_array()
        .map(|a| a.iter().filter(|i| paths.iter().any(|p| Some(p.as_str()) == i["path"].as_str())).count())
        .unwrap_or(0);
    Ok(json!({"requested": wanted, "restored": wanted.saturating_sub(still), "remaining": still}))
}

// ---------------------------------------------------------------- previous versions

#[derive(Serialize, Clone)]
pub struct Shadow {
    pub id: String,
    pub device: String,
    pub created: i64,
    pub letter: String,
}

#[derive(Serialize)]
pub struct ShadowEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: u64,
}

const SHADOWS: &str = r#"$lv=@{}; Get-Volume | ForEach-Object { if($_.UniqueId){ $lv[[string]$_.UniqueId]=[string]$_.DriveLetter } }; $s = @(Get-CimInstance Win32_ShadowCopy | ForEach-Object { [pscustomobject]@{ id=[string]$_.ID; device=[string]$_.DeviceObject; created=([DateTimeOffset]$_.InstallDate).ToUnixTimeSeconds(); letter=[string]$lv[[string]$_.VolumeName] } }); ConvertTo-Json -InputObject $s -Compress"#;

pub fn shadows(admin: bool) -> Result<Vec<Shadow>, String> {
    if !admin {
        return Err("Previous Versions needs Administrator rights (use Restart as Administrator).".into());
    }
    let v = ps::powershell_json(SHADOWS, &[], 60)?;
    let mut out: Vec<Shadow> = v
        .as_array()
        .map(|a| {
            a.iter()
                .map(|s| Shadow {
                    id: s["id"].as_str().unwrap_or("").into(),
                    device: s["device"].as_str().unwrap_or("").into(),
                    created: s["created"].as_i64().unwrap_or(0),
                    letter: s["letter"].as_str().unwrap_or("").into(),
                })
                .filter(|s| s.device.starts_with(r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy"))
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| b.created.cmp(&a.created));
    Ok(out)
}

fn clean_rel(rel: &str) -> Result<PathBuf, String> {
    let mut out = PathBuf::new();
    for c in Path::new(&rel.replace('/', "\\")).components() {
        match c {
            Component::Normal(n) => out.push(n),
            Component::CurDir | Component::RootDir => {}
            _ => return Err("Invalid path".into()),
        }
    }
    Ok(out)
}

fn shadow_root(device: &str, admin: bool) -> Result<PathBuf, String> {
    let list = shadows(admin)?;
    if !list.iter().any(|s| s.device == device) {
        return Err("That snapshot no longer exists.".into());
    }
    Ok(PathBuf::from(format!("{}\\", device)))
}

pub fn shadow_browse(device: &str, rel: &str, admin: bool) -> Result<Vec<ShadowEntry>, String> {
    let root = shadow_root(device, admin)?;
    let dir = root.join(clean_rel(rel)?);
    let rd = fs::read_dir(&dir).map_err(|e| format!("{}: {}", dir.display(), e))?;
    let mut v: Vec<ShadowEntry> = Vec::new();
    for e in rd.flatten().take(5000) {
        if let Ok(md) = e.metadata() {
            v.push(ShadowEntry {
                name: e.file_name().to_string_lossy().to_string(),
                is_dir: is_dir_attr(md.file_attributes()),
                size: md.len(),
                modified: mtime(&md),
            });
        }
    }
    v.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(v)
}

fn unique_dest(dir: &Path, name: &std::ffi::OsStr) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let stem = Path::new(name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = Path::new(name).extension().map(|s| format!(".{}", s.to_string_lossy())).unwrap_or_default();
    for n in 1..1000 {
        let p = dir.join(format!("{} (recovered {}){}", stem, n, ext));
        if !p.exists() {
            return p;
        }
    }
    first
}

fn copy_tree(src: &Path, dst: &Path, files: &mut u64, bytes: &mut u64, errors: &mut Vec<String>) {
    let md = match fs::metadata(src) {
        Ok(m) => m,
        Err(e) => {
            errors.push(format!("{}: {}", src.display(), e));
            return;
        }
    };
    if md.is_dir() {
        if let Err(e) = fs::create_dir_all(dst) {
            errors.push(format!("{}: {}", dst.display(), e));
            return;
        }
        if let Ok(rd) = fs::read_dir(src) {
            for e in rd.flatten() {
                copy_tree(&e.path(), &dst.join(e.file_name()), files, bytes, errors);
            }
        }
    } else {
        match fs::copy(src, dst) {
            Ok(n) => {
                *files += 1;
                *bytes += n;
            }
            Err(e) => errors.push(format!("{}: {}", src.display(), e)),
        }
    }
}

/// Copies files or folders out of a snapshot into `dest`. Existing files are never replaced.
pub fn shadow_restore(device: &str, rels: &[String], dest: &str, admin: bool) -> Result<Value, String> {
    let root = shadow_root(device, admin)?;
    let dest = PathBuf::from(dest.trim().trim_matches('"'));
    if dest.as_os_str().is_empty() || dest.to_string_lossy().to_uppercase().contains("GLOBALROOT") {
        return Err("Choose a normal folder as the destination.".into());
    }
    fs::create_dir_all(&dest).map_err(|e| format!("{}: {}", dest.display(), e))?;
    let (mut files, mut bytes) = (0u64, 0u64);
    let mut errors: Vec<String> = Vec::new();
    for rel in rels {
        let r = clean_rel(rel)?;
        let name = match r.file_name() {
            Some(n) => n.to_os_string(),
            None => continue,
        };
        let src = root.join(&r);
        copy_tree(&src, &unique_dest(&dest, &name), &mut files, &mut bytes, &mut errors);
    }
    log_history("recover", &format!("{} file(s) copied from a previous version", files), bytes);
    Ok(json!({"files": files, "bytes": bytes, "errors": errors, "dest": dest.to_string_lossy()}))
}
