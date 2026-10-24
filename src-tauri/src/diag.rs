//! Event-log diagnostics: reads recent critical/error events (plus hardware-error warnings)
//! from the Windows logs and groups them, so the AI assistant can explain the patterns.
//! Read-only: nothing here changes the system.

use crate::ps;
use serde_json::Value;

const SCRIPT: &str = r#"$days=[int]$env:CMS_DAYS; $st=(Get-Date).AddDays(-$days);
$a=@(Get-WinEvent -FilterHashtable @{LogName='System','Application'; Level=1,2; StartTime=$st} -MaxEvents 3000 -ErrorAction SilentlyContinue);
$w=@(Get-WinEvent -FilterHashtable @{LogName='System'; ProviderName='Microsoft-Windows-WHEA-Logger'; StartTime=$st} -MaxEvents 200 -ErrorAction SilentlyContinue);
$all=@($a)+@($w);
$g=@($all | Group-Object { "$($_.ProviderName)|$($_.Id)" } | Sort-Object Count -Descending | Select-Object -First 40 | ForEach-Object { $f=$_.Group | Sort-Object TimeCreated -Descending | Select-Object -First 1; $m=''; try { $m=(($f.Message -split "`r?`n")[0]) } catch {}; if($m.Length -gt 300){ $m=$m.Substring(0,300) }; [pscustomobject]@{ provider=[string]$f.ProviderName; id=[int]$f.Id; log=[string]$f.LogName; level=[string]$f.LevelDisplayName; count=[int]$_.Count; last=$f.TimeCreated.ToString('yyyy-MM-dd HH:mm'); message=$m } });
ConvertTo-Json -InputObject $g -Compress"#;

pub fn groups(days: u32) -> Result<Value, String> {
    let d = days.clamp(1, 30).to_string();
    ps::powershell_json(SCRIPT, &[("CMS_DAYS", &d)], 100)
}
