# Codex Provider Hub

macOS menu-bar (tray) dashboard for a local [Sub2API](https://github.com/Wei-Shaw/sub2api) multi-provider Codex gateway, plus live usage / quota cards.

**Features**

- **Local gateway** — start / stop / health for Docker Sub2API on `127.0.0.1:18080`, provider & model counts
- **Sub2API pool** — 5-hour / 7-day remaining quota, pool available accounts
- **AIHub (relay)** — wallet balance & today’s spend via `ANYROUTER_API_KEY`
- **Cursor accounts** — add multiple Cursor sessions (import local or paste JWT), show plan usage per account (tokens encrypted at rest)

## Stack

- Tauri v2 + Rust
- React + TypeScript + Vite

## Requirements

- macOS (Apple Silicon recommended)
- [Node.js](https://nodejs.org/) 20+
- [Rust](https://rustup.rs/) stable
- A running / installable Sub2API deployment (Docker Compose) with the `./sub2api` helper script

## Configure

| Source | How the app finds credentials / paths |
|--------|----------------------------------------|
| Sub2API install dir | Env `SUB2API_DIR` or `CODEX_PROVIDER_HUB_SUB2API_DIR`, else `$HOME/Documents/Codex/sub2api-ready` |
| Gateway API key | `$SUB2API_DIR/state/gateway-api-key` |
| AIHub key | Env `ANYROUTER_API_KEY`, or `export ANYROUTER_API_KEY=...` in `~/.zshrc` |
| Codex config (optional save) | `~/.codex/config.toml` + model catalog JSON |
| Cursor tokens | Stored encrypted under the app data dir; optional import from Cursor’s `state.vscdb` |

Example:

```bash
export SUB2API_DIR="$HOME/path/to/your/sub2api-ready"
export ANYROUTER_API_KEY="sk-..."
```

## Dev

```bash
npm install
source "$HOME/.cargo/env"   # if needed
npm run tauri dev
```

Click the tray icon to show/hide the dashboard. Closing the window hides to tray (no Dock icon).

## Build

```bash
npm run tauri build
```

Outputs:

- `src-tauri/target/release/bundle/macos/Codex Provider Hub.app`
- `src-tauri/target/release/bundle/dmg/*_aarch64.dmg`

## Security notes

- API keys are **never** hardcoded; they are read at runtime from env / local files.
- Cursor access tokens are encrypted with AES-256-GCM (key derived from machine id + app salt).
- Saving provider config backs up `config.toml` / catalog with a timestamp before writing, and does **not** rename the `model_provider` id (Codex threads are filtered by that id).

## License

MIT
