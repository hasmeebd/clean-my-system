//! System health: disks (SMART-style reliability counters), volumes (SSD/HDD, TRIM),
//! memory, uptime, pending reboot, antivirus state, startup load. Every check reports the
//! points it earned so the score is transparent, not a magic number.

use crate::{ps, startup, util::*};
use serde::Serialize;
use serde_json::{json, Value};
use windows_sys::Win32::System::SystemInformation::{GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX};
use winreg::enums::*;
use winreg::RegKey;

#[derive(Serialize, Clone)]
pub struct Check {
    pub id: String,
    pub title: String,
    /// ok | warn | bad | unknown
    pub status: String,
    pub points: u32,
    pub max: u32,
    pub detail: String,
    /// page the "fix" button opens
    pub action: String,
}

#[derive(Serialize)]
pub struct Report {
    pub score: u32,
    pub grade: String,
    pub checks: Vec<Check>,
    pub disks: Value,
    pub volumes: Value,
    pub ram_total: u64,
    pub ram_used: u64,
    pub uptime_s: u64,
    pub cpu_threads: usize,
    pub trim: Option<bool>,
    pub startup_enabled: usize,
    pub admin: bool,
}

fn ck(id: &str, title: &str, status: &str, points: u32, max: u32, detail: String, action: &str) -> Check {
    Check { id: id.into(), title: title.into(), status: status.into(), points, max, detail, action: action.into() }
}

pub fn memory() -> (u64, u64) {
    unsafe {
        let mut m: MEMORYSTATUSEX = std::mem::zeroed();
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut m) != 0 {
            return (m.ullTotalPhys, m.ullTotalPhys.saturating_sub(m.ullAvailPhys));
        }
    }
    (0, 0)
}

pub fn uptime_s() -> u64 {
    unsafe { GetTickCount64() / 1000 }
}

fn reboot_pending() -> Vec<&'static str> {
    let lm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let mut why = Vec::new();
    if lm.open_subkey(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending").is_ok() {
        why.push("Windows servicing");
    }
    if lm.open_subkey(r"SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired").is_ok() {
        why.push("Windows Update");
    }
    if let Ok(k) = lm.open_subkey(r"SYSTEM\CurrentControlSet\Control\Session Manager") {
        if k.get_raw_value("PendingFileRenameOperations").is_ok() {
            why.push("pending file operations");
        }
    }
    why
}

fn trim_enabled() -> Option<bool> {
    let o = ps::run_capture("fsutil", &["behavior", "query", "DisableDeleteNotify"], &[], 10).ok()?;
    let line = o.stdout.lines().find(|l| l.contains("NTFS"))?;
    Some(line.contains("= 0"))
}

const DISKS: &str = r#"$d = @(Get-PhysicalDisk | ForEach-Object { $r = $_ | Get-StorageReliabilityCounter -ErrorAction SilentlyContinue; [pscustomobject]@{ name=[string]$_.FriendlyName; media=[string]$_.MediaType; bus=[string]$_.BusType; health=[string]$_.HealthStatus; status=((@($_.OperationalStatus)) -join ','); size=[int64]$_.Size; temp=$r.Temperature; wear=$r.Wear; hours=$r.PowerOnHours; readErr=$r.ReadErrorsTotal; writeErr=$r.WriteErrorsTotal } }); ConvertTo-Json -InputObject $d -Compress"#;

const VOLUMES: &str = r#"$v = @(Get-Volume | Where-Object { $_.DriveLetter } | ForEach-Object { $dl=[string]$_.DriveLetter; $media='Unknown'; try { $media=[string](Get-Partition -DriveLetter $dl -ErrorAction Stop | Get-Disk | Get-PhysicalDisk | Select-Object -First 1 -ExpandProperty MediaType) } catch {}; [pscustomobject]@{ letter=$dl; label=[string]$_.FileSystemLabel; fs=[string]$_.FileSystem; type=[string]$_.DriveType; size=[int64]$_.Size; free=[int64]$_.SizeRemaining; media=$media } }); ConvertTo-Json -InputObject $v -Compress"#;

const AV: &str = r#"$a = @(Get-CimInstance -Namespace root/SecurityCenter2 -ClassName AntivirusProduct -ErrorAction Stop | ForEach-Object { $hex = '{0:X6}' -f [int]$_.productState; [pscustomobject]@{ name=[string]$_.displayName; enabled=($hex.Substring(2,2) -in '10','11'); uptodate=($hex.Substring(4,2) -eq '00') } }); ConvertTo-Json -InputObject $a -Compress"#;

pub fn check(sys_free: u64, sys_total: u64) -> Report {
    let t_disk = std::thread::spawn(|| ps::powershell_json(DISKS, &[], 45));
    let t_vol = std::thread::spawn(|| ps::powershell_json(VOLUMES, &[], 45));
    let t_av = std::thread::spawn(|| ps::powershell_json(AV, &[], 30));
    let trim = trim_enabled();
    let (ram_total, ram_used) = memory();
    let up = uptime_s();
    let startup_enabled = startup_count();
    let disks = t_disk.join().ok().and_then(|r| r.ok());
    let volumes = t_vol.join().ok().and_then(|r| r.ok());
    let av = t_av.join().ok().and_then(|r| r.ok());

    let mut checks: Vec<Check> = Vec::new();

    // 1. free space
    if sys_total > 0 {
        let pct = sys_free as f64 / sys_total as f64 * 100.0;
        let (s, p) = if pct >= 20.0 { ("ok", 15) } else if pct >= 10.0 { ("warn", 8) } else { ("bad", 0) };
        checks.push(ck("space", "Free space on the system drive", s, p, 15, format!("{:.0}% free ({:.1} GB). Windows slows down and cannot update below 10%.", pct, sys_free as f64 / 1e9), "clean"));
    }

    // 2. disk health
    match &disks {
        Some(d) if d.as_array().map(|a| !a.is_empty()).unwrap_or(false) => {
            let arr = d.as_array().unwrap();
            let mut worst = "ok";
            let mut notes: Vec<String> = Vec::new();
            for x in arr {
                let h = x["health"].as_str().unwrap_or("");
                let name = x["name"].as_str().unwrap_or("disk");
                if h != "Healthy" && !h.is_empty() {
                    worst = if h == "Unhealthy" { "bad" } else if worst == "bad" { "bad" } else { "warn" };
                    notes.push(format!("{}: {}", name, h));
                }
                if let Some(w) = x["wear"].as_f64() {
                    if w >= 80.0 {
                        if worst == "ok" {
                            worst = "warn";
                        }
                        notes.push(format!("{}: {:.0}% of rated wear used", name, w));
                    }
                }
                if x["readErr"].as_f64().unwrap_or(0.0) > 0.0 || x["writeErr"].as_f64().unwrap_or(0.0) > 0.0 {
                    if worst == "ok" {
                        worst = "warn";
                    }
                    notes.push(format!("{}: read/write errors logged", name));
                }
            }
            let p = match worst { "ok" => 20, "warn" => 10, _ => 0 };
            checks.push(ck("disk", "Disk health", worst, p, 20,
                if notes.is_empty() { format!("{} physical disk(s) report Healthy", arr.len()) } else { notes.join(" · ") }, "health"));
        }
        _ => checks.push(ck("disk", "Disk health", "unknown", 0, 0, "Windows did not return disk health data.".into(), "health")),
    }

    // 3. TRIM (only meaningful with an SSD)
    let has_ssd = volumes
        .as_ref()
        .and_then(|v| v.as_array())
        .map(|a| a.iter().any(|x| x["media"].as_str() == Some("SSD")))
        .unwrap_or(false);
    if has_ssd {
        match trim {
            Some(true) => checks.push(ck("trim", "SSD TRIM", "ok", 5, 5, "TRIM is on: Windows tells the SSD which blocks are free.".into(), "health")),
            Some(false) => checks.push(ck("trim", "SSD TRIM", "warn", 0, 5, "TRIM is off. Writes get slower over time.".into(), "health")),
            None => {}
        }
    }

    // 4. startup load
    {
        let (s, p) = if startup_enabled <= 10 { ("ok", 10) } else if startup_enabled <= 20 { ("warn", 6) } else { ("bad", 2) };
        checks.push(ck("startup", "Programs that start with Windows", s, p, 10, format!("{} enabled startup entries (registry and Startup folders).", startup_enabled), "startup"));
    }

    // 5. uptime
    {
        let days = up as f64 / 86_400.0;
        let (s, p) = if days < 7.0 { ("ok", 5) } else if days < 21.0 { ("warn", 3) } else { ("bad", 0) };
        checks.push(ck("uptime", "Time since last restart", s, p, 5, format!("Up for {:.1} days. Restarting finishes updates and frees leaked memory.", days), ""));
    }

    // 6. pending reboot
    {
        let why = reboot_pending();
        if why.is_empty() {
            checks.push(ck("reboot", "Restart pending", "ok", 10, 10, "No restart is waiting.".into(), ""));
        } else {
            checks.push(ck("reboot", "Restart pending", "warn", 0, 10, format!("A restart is waiting ({}).", why.join(", ")), ""));
        }
    }

    // 7. memory pressure
    if ram_total > 0 {
        let pct = ram_used as f64 / ram_total as f64 * 100.0;
        let (s, p) = if pct < 80.0 { ("ok", 5) } else if pct < 92.0 { ("warn", 3) } else { ("bad", 0) };
        checks.push(ck("ram", "Memory in use right now", s, p, 5, format!("{:.0}% of {:.1} GB. A snapshot, not an average.", pct, ram_total as f64 / 1e9), "startup"));
    }

    // 8. antivirus
    match &av {
        Some(a) if a.as_array().map(|x| !x.is_empty()).unwrap_or(false) => {
            let arr = a.as_array().unwrap();
            let on: Vec<&Value> = arr.iter().filter(|x| x["enabled"].as_bool().unwrap_or(false)).collect();
            if on.is_empty() {
                checks.push(ck("av", "Antivirus", "bad", 0, 15, "No antivirus reports itself as active.".into(), ""));
            } else if on.iter().any(|x| x["uptodate"].as_bool().unwrap_or(false)) {
                checks.push(ck("av", "Antivirus", "ok", 15, 15, format!("{} is active and up to date.", on[0]["name"].as_str().unwrap_or("Antivirus")), ""));
            } else {
                checks.push(ck("av", "Antivirus", "warn", 8, 15, format!("{} is active but its definitions are out of date.", on[0]["name"].as_str().unwrap_or("Antivirus")), ""));
            }
        }
        _ => checks.push(ck("av", "Antivirus", "unknown", 0, 0, "Security Center did not report an antivirus.".into(), "")),
    }

    let max: u32 = checks.iter().map(|c| c.max).sum();
    let got: u32 = checks.iter().map(|c| c.points).sum();
    let score = if max == 0 { 0 } else { ((got as f64 / max as f64) * 100.0).round() as u32 };
    let grade = match score { 90..=100 => "Excellent", 75..=89 => "Good", 55..=74 => "Needs attention", _ => "Poor" };

    Report {
        score,
        grade: grade.into(),
        checks,
        disks: disks.unwrap_or_else(|| json!([])),
        volumes: volumes.unwrap_or_else(|| json!([])),
        ram_total,
        ram_used,
        uptime_s: up,
        cpu_threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
        trim,
        startup_enabled,
        admin: is_admin(),
    }
}

fn startup_count() -> usize {
    let mut n = 0;
    for list in [startup::list_run(), startup::list_folders()] {
        n += list.iter().filter(|i| i.enabled).count();
    }
    n
}
