# Clean My System
R&D by OOFFOO · Developed by HB Mostafa

Disk analyzer + safe cleaner + local Ollama assistant (Tauri 2, Rust).

Windows only. See [Build from source](#build-from-source) below.

Safety:
- Safe Clean only touches a hard-coded allowlist of caches/temp.
- Ollama only explains and suggests; it never deletes. AI suggestions need preview + confirm.
- Deletes go to Quarantine (undo, auto-purge after 7 days) unless you choose permanent.
- System paths are always protected; add your own folders under Settings -> protected paths.
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

## Requirements
- Windows 10/11 (WebView2 runtime, already included in Windows 11)
- Rust 1.98.0, MSVC toolchain (`stable-x86_64-pc-windows-msvc`); pinned in `rust-toolchain.toml`
- Visual Studio Build Tools (C++ workload)
- Optional: [Ollama](https://ollama.com) running locally (default `http://127.0.0.1:11434`) for the AI assistant. Default model is `qwen3:8b`; no models are bundled.

## Build from source
```powershell
git clone https://github.com/hasmeebd/clean-my-system.git
cd clean-my-system\src-tauri
cargo build --release --locked
# result: target\release\cdrive-studio.exe
```
Convenience script: `Build-And-Run.bat` (runs `build.ps1`, which stops any running copy of the app, builds into `%USERPROFILE%\.cargo-targets\cdrive-studio`, and copies the exe to `dist\Clean-My-System.exe`).

## Project layout
| Path | Purpose |
|---|---|
| `src-tauri/` | Rust backend (Tauri 2): scanning, safety rules, quarantine, Ollama client |
| `ui/` | Static frontend (plain HTML/CSS/JS, no npm build step) |
| `build.ps1`, `Build-And-Run.bat` | Windows build helpers |

## Assets, Models and External Dependencies (not in repo)
See [ASSETS.md](ASSETS.md). Short version: no models, no binaries, no secrets. The app needs no API keys or `.env` file.

## Contributing
Issues and pull requests are welcome. Please keep the safety rules intact: the app must never delete anything outside its allowlist without preview and confirmation.

## License
[MIT](LICENSE) (c) 2026 OOFFOO (HB Mostafa)