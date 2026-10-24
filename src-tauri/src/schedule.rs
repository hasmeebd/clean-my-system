//! Scheduled automatic clean. A normal per-user Windows scheduled task starts this program
//! with `--auto-clean`, which runs only the safe, non-admin targets you ticked, writes the
//! History entry and exits. No background service is installed.

use crate::hogs;
use crate::ps;
use crate::util::*;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::atomic::AtomicBool;

const DAYS: &[&str] = &["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

/// Targets that may ever run unattended.
pub fn choices() -> Vec<Value> {
    hogs::defs()
        .into_iter()
        .filter(|d| d.auto && !d.admin && d.risk == "safe")
        .map(|d| json!({"id": d.id, "name": d.name, "group": d.group, "tip": d.tip}))
        .collect()
}

pub fn auto_clean() -> Value {
    let s = load_settings();
    let allowed: HashSet<String> = hogs::defs().into_iter().filter(|d| d.auto && !d.admin && d.risk == "safe").map(|d| d.id.to_string()).collect();
    let ids: Vec<String> = s.auto_clean_ids.iter().filter(|i| allowed.contains(*i)).cloned().collect();
    let cancel = AtomicBool::new(false);
    let items = hogs::clean(&ids, &cancel);
    let freed: u64 = items.iter().map(|i| i.freed).sum();
    log_history("auto-clean", &items.iter().map(|i| i.name.clone()).collect::<Vec<_>>().join(", "), freed);
    if s.quarantine_days > 0 {
        let r = crate::quarantine::purge_expired(s.quarantine_days);
        if r.ok > 0 {
            log_history("auto-purge", &format!("{} expired quarantine item(s)", r.ok), r.bytes);
        }
    }
    snapshot();
    json!({"targets": ids.len(), "freed": freed})
}

pub fn status() -> Result<Value, String> {
    let v = ps::powershell_json(
        r#"$t=Get-ScheduledTask -TaskPath '\System Care Suite\' -TaskName 'Auto Clean' -ErrorAction SilentlyContinue; if(-not $t){ '[]' } else { $i=$t | Get-ScheduledTaskInfo; $tr=@($t.Triggers)[0]; $nx=''; if($i.NextRunTime -and $i.NextRunTime.Year -gt 2000){ $nx=$i.NextRunTime.ToString('s') }; $ls=''; if($i.LastRunTime -and $i.LastRunTime.Year -gt 2000){ $ls=$i.LastRunTime.ToString('s') }; $at=''; try { $at=([datetime]$tr.StartBoundary).ToString('HH:mm') } catch {}; ConvertTo-Json -InputObject @([pscustomobject]@{ state=[string]$t.State; next=$nx; last=$ls; result=[int]$i.LastTaskResult; kind=[string]$tr.CimClass.CimClassName; at=$at; days=[int]$tr.DaysOfWeek }) -Compress }"#,
        &[],
        30,
    )?;
    Ok(v.as_array().and_then(|a| a.first()).cloned().unwrap_or(Value::Null))
}

pub fn set(freq: &str, day: &str, time: &str) -> Result<(), String> {
    if freq != "daily" && freq != "weekly" {
        return Err("Unknown schedule".into());
    }
    if !DAYS.contains(&day) {
        return Err("Unknown day".into());
    }
    let ok_time = time.len() == 5 && time.as_bytes()[2] == b':' && time[..2].parse::<u32>().map(|h| h < 24).unwrap_or(false) && time[3..].parse::<u32>().map(|m| m < 60).unwrap_or(false);
    if !ok_time {
        return Err("Time must look like 18:30".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let script = r#"$a=New-ScheduledTaskAction -Execute $env:CMS_EXE -Argument '--auto-clean'; $at=[datetime]::ParseExact($env:CMS_TIME,'HH:mm',[Globalization.CultureInfo]::InvariantCulture); if($env:CMS_FREQ -eq 'daily'){ $t=New-ScheduledTaskTrigger -Daily -At $at } else { $t=New-ScheduledTaskTrigger -Weekly -DaysOfWeek $env:CMS_DAY -At $at }; $s=New-ScheduledTaskSettingsSet -StartWhenAvailable -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit (New-TimeSpan -Hours 1); Register-ScheduledTask -TaskName 'Auto Clean' -TaskPath '\System Care Suite\' -Action $a -Trigger $t -Settings $s -Description 'System Care Suite automatic safe clean' -Force | Out-Null"#;
    ps::powershell(script, &[("CMS_EXE", &exe.to_string_lossy()), ("CMS_TIME", time), ("CMS_FREQ", freq), ("CMS_DAY", day)], 40)?;
    log_history("schedule", &format!("Auto clean {} at {}", freq, time), 0);
    Ok(())
}

pub fn remove() -> Result<(), String> {
    ps::powershell(r#"Unregister-ScheduledTask -TaskName 'Auto Clean' -TaskPath '\System Care Suite\' -Confirm:$false -ErrorAction SilentlyContinue"#, &[], 30)?;
    log_history("schedule", "Auto clean schedule removed", 0);
    Ok(())
}
