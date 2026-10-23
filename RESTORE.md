# Fresh-PC setup

1. Install the toolchain (Windows 10/11):
   ```powershell
   winget install --id Rustlang.Rustup -e
   winget install --id Microsoft.VisualStudio.2022.BuildTools -e
   winget install --id Git.Git -e
   ```
   In the Visual Studio installer select the "Desktop development with C++" workload.
2. Clone and build:
   ```powershell
   git clone https://github.com/hasmeebd/clean-my-system.git
   cd clean-my-system\src-tauri
   cargo build --release --locked
   ```
   `rust-toolchain.toml` makes rustup fetch Rust 1.98.0 automatically.
3. Run `target\release\cdrive-studio.exe`.
4. Optional AI assistant: install Ollama (`winget install --id Ollama.Ollama -e`), then `ollama pull qwen3:8b`.

Known pitfalls
- `build.ps1` stops any running copy of the app before building.
- Without Visual Studio Build Tools the linker step fails.
- Everything under the app's own settings/quarantine lives in `%LOCALAPPDATA%`, not in the repo.