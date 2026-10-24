//! Process monitor: a read-only live list (CPU, private memory, path) and "End process" with a
//! hard-coded protection list. There is deliberately no RAM "booster": ending a process is
//! always an explicit, confirmed action on one named program.

use crate::ps;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Serialize)]
pub struct Proc {
    pub pid: u32,
    pub name: String,
    pub path: String,
    pub cpu: f64,
    pub mem: u64,
}

const PROTECTED: &[&str] = &[
    "system", "registry", "smss.exe", "csrss.exe", "wininit.exe", "winlogon.exe", "services.exe", "lsass.exe",
    "svchost.exe", "dwm.exe", "fontdrvhost.exe", "memory compression", "memcompression", "secure system",
    "msmpeng.exe", "securityhealthservice.exe", "explorer.exe", "sihost.exe", "ctfmon.exe", "audiodg.exe",
    "system idle process", "lsaiso.exe", "wudfhost.exe",
];

pub fn is_protected(name: &str) -> bool {
    let n = name.to_lowercase();
    PROTECTED.contains(&n.as_str())
}

const PERF: &str = "$p=@(Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -Property Name,IDProcess,PercentProcessorTime,WorkingSetPrivate | Where-Object { $_.Name -ne '_Total' -and $_.IDProcess -ne 0 } | ForEach-Object { [pscustomobject]@{ pid=[int]$_.IDProcess; cpu=[double]$_.PercentProcessorTime; mem=[int64]$_.WorkingSetPrivate } }); ConvertTo-Json -InputObject $p -Compress";
const PROCS: &str = "$p=@(Get-CimInstance Win32_Process -Property ProcessId,Name,ExecutablePath,WorkingSetSize | ForEach-Object { [pscustomobject]@{ pid=[int]$_.ProcessId; name=[string]$_.Name; path=[string]$_.ExecutablePath; ws=[int64]$_.WorkingSetSize } }); ConvertTo-Json -InputObject $p -Compress";

pub fn list() -> Result<Vec<Proc>, String> {
    let t = std::thread::spawn(|| ps::powershell_json(PERF, &[], 40));
    let procs = ps::powershell_json(PROCS, &[], 40)?;
    let perf = t.join().ok().and_then(|r| r.ok()).unwrap_or(Value::Array(vec![]));
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
    let mut pm: HashMap<u64, (f64, i64)> = HashMap::new();
    for x in perf.as_array().cloned().unwrap_or_default() {
        pm.insert(x["pid"].as_u64().unwrap_or(0), (x["cpu"].as_f64().unwrap_or(0.0), x["mem"].as_i64().unwrap_or(0)));
    }
    let mut out: Vec<Proc> = procs
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|x| x["pid"].as_u64().unwrap_or(0) != 0)
        .map(|x| {
            let pid = x["pid"].as_u64().unwrap_or(0);
            let (cpu, mem) = pm.get(&pid).copied().unwrap_or((0.0, x["ws"].as_i64().unwrap_or(0)));
            Proc {
                pid: pid as u32,
                name: x["name"].as_str().unwrap_or("").to_string(),
                path: x["path"].as_str().unwrap_or("").to_string(),
                cpu: ((cpu / threads) * 10.0).round() / 10.0,
                mem: mem.max(0) as u64,
            }
        })
        .collect();
    out.sort_by(|a, b| b.mem.cmp(&a.mem));
    Ok(out)
}

/// Ends one process. The pid must still belong to a program with the same name.
pub fn end(pid: u32, name: &str) -> Result<(), String> {
    if pid == std::process::id() {
        return Err("That is System Care Suite itself.".into());
    }
    if is_protected(name) {
        return Err(format!("{} is a protected Windows process and cannot be ended here.", name));
    }
    let pid_s = pid.to_string();
    let v = ps::powershell_json(
        "$r=@(Get-CimInstance Win32_Process -Filter ('ProcessId=' + [int]$env:CMS_PID) | ForEach-Object { [pscustomobject]@{ name=[string]$_.Name } }); ConvertTo-Json -InputObject $r -Compress",
        &[("CMS_PID", &pid_s)],
        20,
    )?;
    let cur = v.as_array().and_then(|a| a.first()).and_then(|x| x["name"].as_str()).ok_or("That process has already ended.")?;
    if !cur.eq_ignore_ascii_case(name) {
        return Err("The process list changed, refresh and try again.".into());
    }
    if is_protected(cur) {
        return Err(format!("{} is a protected Windows process.", cur));
    }
    let o = ps::run_capture("taskkill", &["/PID", &pid_s, "/F"], &[], 20)?;
    if o.code != Some(0) {
        let m = format!("{}{}", o.stdout, o.stderr);
        return Err(if m.to_lowercase().contains("access is denied") { "Access denied. This process needs Administrator rights to end.".into() } else { m.trim().to_string() });
    }
    crate::util::log_history("end-process", &format!("{} (pid {})", cur, pid), 0);
    Ok(())
}
