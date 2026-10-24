# Builds System Care Suite (R&D by OOFFOO, developed by HB Mostafa) and copies the executable to .\dist
# Needs: Rust (MSVC toolchain), Visual Studio Build Tools, WebView2 (already part of Windows 11).
$ErrorActionPreference = 'Continue'
$root = $PSScriptRoot
# Keep the multi-GB build output out of the project folder
$env:CARGO_TARGET_DIR = Join-Path $env:USERPROFILE '.cargo-targets\cdrive-studio'

Get-Process cdrive-studio, System-Care-Suite -ErrorAction SilentlyContinue | Stop-Process -Force
Push-Location (Join-Path $root 'src-tauri')
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
}
finally { Pop-Location }

New-Item -ItemType Directory -Force -Path (Join-Path $root 'dist') | Out-Null
Remove-Item (Join-Path $root 'dist\CDrive-Studio.exe') -Force -ErrorAction SilentlyContinue
Copy-Item (Join-Path $env:CARGO_TARGET_DIR 'release\cdrive-studio.exe') (Join-Path $root 'dist\System-Care-Suite.exe') -Force
Write-Host ("Built: " + (Join-Path $root 'dist\System-Care-Suite.exe')) -ForegroundColor Green
