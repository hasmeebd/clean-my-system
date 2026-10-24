# System Care Suite
R&D by OOFFOO · Developed by HB Mostafa

Disk analyzer + safe cleaner + local Ollama assistant (Tauri 2, Rust).

Run: `dist\System-Care-Suite.exe`. Rebuild: double-click `Build-And-Run.bat` (needs Rust MSVC + VS Build Tools).

Safety:
- Safe Clean only touches a hard-coded allowlist of caches/temp.
- Ollama only explains and suggests; it never deletes. AI suggestions need preview + confirm.
- Deletes go to Quarantine (undo, auto-purge after 7 days) unless you choose permanent.
- System paths and `Desktop\Arian` are protected (Settings -> protected paths).
- Admin-only items (Windows Temp, update cache) need "Restart as Administrator".

Notes: sizes are size on disk; hard-linked files can make totals slightly exceed used space.
Bengali AI answers are mediocre with qwen3:8b; try gemma or qwen2.5:14b in Settings.

## Dev Junk
Scans one or more project folders for regenerable build output and caches (node_modules, .next/.nuxt, Rust target, Maven target,
Python venv and caches, Unity Library/Temp, Unreal Intermediate/DerivedDataCache, Gradle, Flutter, .NET bin/obj, CocoaPods,
Terraform, Blender .blend1 backups). Each folder is verified against a manifest file next to it, or against `git check-ignore`.
Shows project age and git state (uncommitted changes). Rebuildable items default to permanent delete, `review` items to Quarantine.
Rust `target` can be trimmed to just `target\debug`.

## Thinking models
Models that support thinking are detected from Ollama (marked with a brain). Thinking is off by default (faster); switch it on per model on
the AI Assistant page or in Settings.

## System Care Suite - Phase 1 tools

- **System Health**: score from 8 transparent checks (space, disk health, TRIM, startup load, uptime, pending restart, RAM, antivirus). Drive TRIM / analyze / defrag (HDD only).
- **Duplicates**: size -> quick fingerprint -> full BLAKE3. Each removal is re-verified against the kept copy; at least one copy always stays. Goes to Quarantine by default.
- **Startup**: same on/off switch as Task Manager (registry, Startup folders, logon tasks, third-party services). Reversible; AI only explains.
- **Troubleshoot**: fixed list of built-in repairs (sfc, DISM, chkdsk scan, network reset, Windows Update reset, ...) with live output and cancel.
- **Recover Files**: Recycle Bin restore; Previous Versions (shadow copies, Administrator) copied out without overwriting.

Not included on purpose: registry cleaner, RAM booster, own antivirus.

## Phase 2 tools

- **Optimize**: reversible registry tweaks (originals are saved and put back on undo), power plan switch, hibernation off (frees hiberfil.sys), one-click restore point, and Store-app debloat from a fixed allowlist (reinstallable from the Store). No speed-boost claims.
- **Programs**: installed-program list with each program's own uninstaller (silent mode when the program offers it).
- **Updates**: winget upgrade list; update selected or all, with live output.
