# Assets, Models and External Dependencies

Nothing in this table is stored in the repository.

| Item | Size | Purpose | How to get it | Irreplaceable? |
|---|---|---|---|---|
| `dist/System-Care-Suite.exe` | ~10 MB | Built executable | Rebuild: `cargo build --release --locked` in `src-tauri` | No |
| `src-tauri/gen/` | ~0.3 MB | Tauri generated schemas | Recreated automatically by `tauri-build` during build | No |
| `screens/` | ~0.9 MB | Local screenshots (contain personal paths) | Not published; retake with the current app | No |
| Ollama + a model (e.g. `qwen3:8b`) | several GB | Optional AI assistant | Install Ollama, then `ollama pull qwen3:8b`. Host and model are configurable in Settings | No |

No API keys, tokens or `.env` files are used. Environment variables read by the app are Windows built-ins only: `SystemDrive`, `LOCALAPPDATA`, `USERPROFILE`.