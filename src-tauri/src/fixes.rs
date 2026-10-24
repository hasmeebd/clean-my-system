//! Troubleshooting center. A fixed allowlist of well-known Windows repair commands, each with a
//! plain explanation. Nothing here accepts command text from the UI: the UI sends an id and
//! this module maps it to the exact program and arguments.

use crate::ps;
use crate::util::*;
use serde::Serialize;
use serde_json::json;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

#[derive(Serialize, Clone)]
pub struct FixDef {
    pub id: String,
    pub title: String,
    pub desc: String,
    pub group: String,
    pub admin: bool,
    pub long: bool,
    pub reboot: bool,
    /// briefly disturbs the desktop or the connection
    pub caution: bool,
}

#[allow(clippy::too_many_arguments)]
fn d(id: &str, group: &str, title: &str, desc: &str, admin: bool, long: bool, reboot: bool, caution: bool) -> FixDef {
    FixDef { id: id.into(), title: title.into(), desc: desc.into(), group: group.into(), admin, long, reboot, caution }
}

pub fn defs() -> Vec<FixDef> {
    vec![
        d("sfc", "Repair Windows", "System File Checker (sfc)", "Scans protected Windows files and replaces damaged ones. Fixes odd crashes and missing-DLL errors. Takes 5 to 15 minutes.", true, true, false, false),
        d("dism_check", "Repair Windows", "DISM: quick health check", "Asks Windows whether its component store is marked damaged. Takes seconds.", true, false, false, false),
        d("dism_scan", "Repair Windows", "DISM: full image scan", "Scans the Windows image for corruption without changing anything.", true, true, false, false),
        d("dism_restore", "Repair Windows", "DISM: repair Windows image", "Downloads clean copies from Windows Update and repairs the image. Run this first when sfc cannot fix everything, then run sfc again.", true, true, false, false),
        d("chkdsk_scan", "Repair Windows", "Check the system drive for errors", "Online, read-only scan of the file system (chkdsk /scan). Reports problems without taking the drive offline.", true, true, false, false),
        d("net_info", "Network", "Network diagnostics (read-only)", "Shows adapters, IP, DNS, and tests the internet and name resolution. Start here for any connection problem.", false, false, false, false),
        d("flush_dns", "Network", "Flush the DNS cache", "Clears remembered website addresses. Fixes sites that load for others but not for you.", true, false, false, false),
        d("renew_ip", "Network", "Renew the IP address", "Releases and requests a new IP address. The connection drops for a few seconds.", true, false, false, true),
        d("prefer_ipv4", "Network", "Prefer IPv4 over IPv6", "Makes Windows try IPv4 first. Fixes programs (winget, Store, some downloads) that hang because the IPv6 route of your internet provider is broken. Nothing is disabled, it stays after a restart, and it can be undone below. Run it again if an older version of this app applied it: that version saved it incompletely, so Windows ignored it.", true, false, false, false),
        d("undo_prefer_ipv4", "Network", "Undo: Windows default IPv4/IPv6 order", "Puts the address-order policy back to the Windows default.", true, false, false, false),
        d("reset_winsock", "Network", "Reset Winsock", "Rebuilds the network socket catalog. Fixes 'connected but no internet' after malware or VPN leftovers. Needs a restart.", true, false, true, false),
        d("reset_tcpip", "Network", "Reset the TCP/IP stack", "Resets network settings to defaults. Needs a restart. Static IP settings are lost.", true, false, true, true),
        d("wu_reset", "Windows Update & Store", "Reset Windows Update", "Stops the update services and renames the SoftwareDistribution and catroot2 folders so Windows rebuilds them. Fixes updates stuck at a percentage or failing with errors.", true, false, false, false),
        d("store_reset", "Windows Update & Store", "Reset the Microsoft Store cache", "Runs wsreset. The Store opens when it finishes.", false, false, false, false),
        d("icon_cache", "Windows shell", "Rebuild icon and thumbnail cache", "Fixes blank or wrong icons and thumbnails. The taskbar and open File Explorer windows restart for a moment.", false, false, false, true),
        d("spooler_reset", "Windows shell", "Clear stuck print jobs", "Stops the print spooler, deletes queued jobs, starts it again.", true, false, false, false),
        d("time_sync", "Windows shell", "Resync the clock", "Asks the Windows time service to sync now. Fixes certificate and sign-in errors caused by a wrong clock.", true, false, false, false),
        d("recent_errors", "Diagnostics", "Recent errors from the event log", "Lists critical and error events from the last two days (System and Application logs). Read-only. Handy to see what crashed and when.", false, false, false, false),
        d("restore_point", "Safety", "Create a restore point", "Saves a System Restore snapshot so Windows settings can be rolled back. Windows allows one automatic restore point per 24 hours, and System Protection must be on.", true, false, false, false),
    ]
}

fn exe(p: &str, args: &[&str]) -> (String, Vec<String>) {
    (p.into(), args.iter().map(|s| s.to_string()).collect())
}

fn pws(script: &str) -> (String, Vec<String>) {
    (
        "powershell".into(),
        ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &format!("[Console]::OutputEncoding=[Text.Encoding]::UTF8; {}", script)]
            .iter()
            .map(|s| s.to_string())
            .collect(),
    )
}

/// Writes the whole IPv6 prefix-policy table (RFC 6724, Windows' built-in values) with the given
/// precedence for IPv4 (35 is the Windows default, 100 puts IPv4 first). The whole table, because
/// once netsh saves any row Windows drops its built-in table and uses only the saved rows: a lone
/// IPv4 row leaves IPv6 addresses matching nothing, and Windows then keeps trying IPv6 first.
fn prefix_table(v4: u32, done: &str) -> (String, Vec<String>) {
    pws(&format!(
        "$ErrorActionPreference='Continue'; $bad=0; foreach($r in @(@('::1/128',50,0),@('::/0',40,1),@('::ffff:0:0/96',{},4),@('2002::/16',30,2),@('2001::/32',5,5),@('fc00::/7',3,13),@('fec0::/10',1,11),@('3ffe::/16',1,12),@('::/96',1,3))){{ netsh interface ipv6 set prefixpolicy $r[0] $r[1] $r[2] | Out-Null; if($LASTEXITCODE -ne 0){{ netsh interface ipv6 add prefixpolicy $r[0] $r[1] $r[2] | Out-Null }}; if($LASTEXITCODE -ne 0){{ Write-Output \"Could not save $($r[0])\"; $bad=1 }} }}; if($bad -eq 0){{ Write-Output '{}' }}; exit $bad",
        v4, done
    ))
}

const V4_FIRST: &str = "Done. Windows now tries IPv4 first. IPv6 stays switched on.";

/// Prefers IPv4 without the fix dialog, for the updater when it has proven the IPv6 route broken.
/// Needs Administrator. True when the table was saved.
pub fn prefer_ipv4_quietly() -> bool {
    let (prog, args) = prefix_table(100, V4_FIRST);
    let args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let ok = ps::run_capture(&prog, &args, &[], 30).map(|o| !o.timed_out && o.code == Some(0)).unwrap_or(false);
    log_history("fix", &format!("Prefer IPv4 over IPv6 (automatic: the IPv6 route to Microsoft's servers is broken) ({})", if ok { "done" } else { "failed" }), 0);
    ok
}

fn valid_letter(l: &str) -> Option<String> {
    let l = l.trim().trim_end_matches(':').to_uppercase();
    if l.len() == 1 && l.chars().all(|c| c.is_ascii_alphabetic()) && fixed_drives().iter().any(|d| d.starts_with(&l)) {
        Some(l)
    } else {
        None
    }
}

/// (definition, steps) for an id. Dynamic ids look like `trim:C`.
pub fn resolve(id: &str) -> Option<(FixDef, Vec<(String, Vec<String>)>)> {
    if let Some(pkg) = id.strip_prefix("winget:") {
        if !crate::updater::valid_id(pkg) {
            return None;
        }
        let def = d(id, "Updates", &format!("Update {}", pkg), "Installs the newest version with winget, silently.", false, true, false, false);
        let mut args: Vec<String> = ["upgrade", "--id", pkg, "--exact", "--silent", "--accept-package-agreements", "--accept-source-agreements", "--disable-interactivity"].iter().map(|s| s.to_string()).collect();
        if pkg == "all" {
            return None;
        }
        args.shrink_to_fit();
        return Some((def, vec![("winget".to_string(), args)]));
    }
    if id == "winget_all" {
        let def = d(id, "Updates", "Update all programs", "Installs every available winget update, silently.", false, true, false, false);
        let args: Vec<String> = ["upgrade", "--all", "--silent", "--accept-package-agreements", "--accept-source-agreements", "--disable-interactivity"].iter().map(|s| s.to_string()).collect();
        return Some((def, vec![("winget".to_string(), args)]));
    }
    if let Some(name) = id.strip_prefix("debloat:") {
        let b = crate::apps::bloat_label(name)?;
        let def = d(id, "Debloat", &format!("Remove {}", b.1), "Removes this Store app for your account. Reinstall it from the Microsoft Store any time.", false, false, false, b.3);
        let script = format!("$ErrorActionPreference='Continue'; $p = @(Get-AppxPackage -Name '{0}'); if(-not $p){{ Write-Output 'Not installed.' }} else {{ foreach($x in $p){{ Write-Output \"Removing $($x.PackageFullName)\"; Remove-AppxPackage -Package $x.PackageFullName -ErrorAction Continue }}; Write-Output 'Done.' }}", b.0);
        return Some((def, vec![pws(&script)]));
    }
    if let Some((kind, letter)) = id.split_once(':') {
        let l = valid_letter(letter)?;
        let (title, desc, flag, long) = match kind {
            "trim" => (format!("TRIM drive {}", l), "Tells the SSD which blocks are free (retrim). Safe and quick.", "-ReTrim", false),
            "analyze" => (format!("Analyze drive {}", l), "Reports fragmentation and free-space state without changing anything.", "-Analyze", false),
            "defrag" => (format!("Defragment drive {}", l), "Rearranges files on a hard disk. Can take hours. Never use on an SSD.", "-Defrag", true),
            _ => return None,
        };
        let script = format!("Optimize-Volume -DriveLetter {} {} -Verbose 4>&1 | ForEach-Object {{ \"$_\" }}", l, flag);
        let def = d(id, "Drives", &title, desc, true, long, false, false);
        return Some((def, vec![pws(&script)]));
    }
    let def = defs().into_iter().find(|x| x.id == id)?;
    let sd = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let steps: Vec<(String, Vec<String>)> = match id {
        "sfc" => vec![exe("sfc", &["/scannow"])],
        "dism_check" => vec![exe("DISM.exe", &["/Online", "/Cleanup-Image", "/CheckHealth"])],
        "dism_scan" => vec![exe("DISM.exe", &["/Online", "/Cleanup-Image", "/ScanHealth"])],
        "dism_restore" => vec![exe("DISM.exe", &["/Online", "/Cleanup-Image", "/RestoreHealth"])],
        "chkdsk_scan" => vec![exe("chkdsk", &[&sd, "/scan"])],
        "net_info" => vec![
            exe("ipconfig", &["/all"]),
            exe("ping", &["-n", "3", "1.1.1.1"]),
            exe("nslookup", &["example.com"]),
            exe("ping", &["-n", "3", "example.com"]),
        ],
        "flush_dns" => vec![exe("ipconfig", &["/flushdns"])],
        "renew_ip" => vec![exe("ipconfig", &["/release"]), exe("ipconfig", &["/renew"])],
        "reset_winsock" => vec![exe("netsh", &["winsock", "reset"])],
        "reset_tcpip" => vec![exe("netsh", &["int", "ip", "reset"])],
        "wu_reset" => vec![pws(
            "$ErrorActionPreference='Continue'; $svc='wuauserv','bits','cryptsvc','msiserver'; foreach($s in $svc){ Write-Output \"Stopping $s\"; Stop-Service $s -Force -ErrorAction SilentlyContinue }; $stamp=Get-Date -Format yyyyMMdd-HHmmss; foreach($p in \"$env:windir\\SoftwareDistribution\",\"$env:windir\\System32\\catroot2\"){ if(Test-Path $p){ $n=\"$p.old-$stamp\"; try { Rename-Item $p $n -ErrorAction Stop; Write-Output \"Renamed $p -> $n\" } catch { Write-Output \"Could not rename $p : $($_.Exception.Message)\" } } }; foreach($s in $svc){ Write-Output \"Starting $s\"; Start-Service $s -ErrorAction SilentlyContinue }; Write-Output 'Done. Windows rebuilds these folders. You can delete the .old-* folders later to free space.'",
        )],
        "store_reset" => vec![exe("wsreset.exe", &[])],
        "icon_cache" => vec![pws(
            "$ErrorActionPreference='Continue'; Stop-Process -Name explorer -Force -ErrorAction SilentlyContinue; Start-Sleep 1; Remove-Item \"$env:LOCALAPPDATA\\IconCache.db\" -Force -ErrorAction SilentlyContinue; Remove-Item \"$env:LOCALAPPDATA\\Microsoft\\Windows\\Explorer\\iconcache_*.db\" -Force -ErrorAction SilentlyContinue; Remove-Item \"$env:LOCALAPPDATA\\Microsoft\\Windows\\Explorer\\thumbcache_*.db\" -Force -ErrorAction SilentlyContinue; Start-Process explorer; Write-Output 'Icon and thumbnail caches cleared, Explorer restarted.'",
        )],
        "spooler_reset" => vec![pws(
            "$ErrorActionPreference='Continue'; Stop-Service spooler -Force; Remove-Item \"$env:windir\\System32\\spool\\PRINTERS\\*\" -Force -ErrorAction SilentlyContinue; Start-Service spooler; Write-Output 'Print queue cleared, spooler restarted.'",
        )],
        "prefer_ipv4" => vec![prefix_table(100, V4_FIRST)],
        "undo_prefer_ipv4" => vec![prefix_table(35, "Done. Windows uses its default IPv4/IPv6 order again.")],
        "time_sync" => vec![pws(
            "$ErrorActionPreference='Continue'; $s=Get-Service w32time; if($s.Status -ne 'Running'){ Write-Output 'Starting the Windows Time service...'; Start-Service w32time; Start-Sleep 2 }; $o = (w32tm /resync 2>&1 | Out-String).Trim(); Write-Output $o; if($o -notmatch 'completed successfully'){ Write-Output 'Clock was NOT resynced (see message above).'; exit 1 }",
        )],
        "recent_errors" => vec![pws(
            "$e = Get-WinEvent -FilterHashtable @{LogName='System','Application'; Level=1,2; StartTime=(Get-Date).AddDays(-2)} -MaxEvents 40 -ErrorAction SilentlyContinue; if(-not $e){ Write-Output 'No critical or error events in the last 2 days.' } else { $e | ForEach-Object { $m = ($_.Message -split \"`r?`n\")[0]; Write-Output ('{0:yyyy-MM-dd HH:mm}  [{1}]  {2} (id {3}): {4}' -f $_.TimeCreated,$_.LogName,$_.ProviderName,$_.Id,$m) } }",
        )],
        "restore_point" => vec![pws(
            "Checkpoint-Computer -Description 'System Care Suite' -RestorePointType MODIFY_SETTINGS -ErrorAction Stop; Write-Output 'Restore point created.'",
        )],
        _ => return None,
    };
    Some((def, steps))
}

/// Runs a fix on its own thread and reports through `fix-line` / `fix-done` events.
pub fn run(app: AppHandle, id: String, req_id: String, admin: bool, cancel: Arc<AtomicBool>) -> Result<(), String> {
    let (def, steps) = resolve(&id).ok_or("Unknown action")?;
    if def.admin && !admin {
        return Err("This action needs Administrator rights. Use 'Restart as Administrator' first.".into());
    }
    std::thread::spawn(move || {
        if id.starts_with("winget") {
            let _ = app.emit("fix-line", json!({"id": req_id, "line": "Starting winget. It may refresh its package sources first, which can take a minute or two on a slow connection…"}));
        }
        let mut last: Option<i32> = Some(0);
        let mut err: Option<String> = None;
        for (prog, args) in steps {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let a = app.clone();
            let rid = req_id.clone();
            let cb: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |line| {
                let _ = a.emit("fix-line", json!({"id": rid, "line": line}));
            });
            match ps::run_stream(&prog, &args, &cancel, cb) {
                Ok(c) => {
                    last = c;
                    if c != Some(0) && c.is_some() && id != "net_info" {
                        // keep going only for read-only network checks
                        break;
                    }
                }
                Err(e) => {
                    err = Some(e);
                    break;
                }
            }
        }
        let cancelled = cancel.load(std::sync::atomic::Ordering::Relaxed);
        log_history("fix", &format!("{} ({})", def.title, if cancelled { "cancelled".into() } else if let Some(e) = &err { e.clone() } else { format!("exit {:?}", last) }), 0);
        let _ = app.emit(
            "fix-done",
            json!({"id": req_id, "code": last, "error": err, "cancelled": cancelled, "reboot": def.reboot && last == Some(0) && !cancelled}),
        );
    });
    Ok(())
}
