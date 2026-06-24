# Meridian Tray App

A Tauri-based macOS menu bar application that owns screen capture, serves the embedded dashboard, supervises the MLX runtime, and drives the live data streams the UI consumes.

## Architecture (v1.64.0+)

The tray is the central process — it replaces the old Node UI server and the external screenpipe process:

```
Tauri tray binary
├── In-process capture (screenpipe-screen + screenpipe-a11y)
│     └── writes capture_frames / capture_ui_events → meridian.db
├── Embedded dashboard (static Next.js export in ui/out/)
│     └── frontend reaches Rust via Tauri invoke / events only
├── Poll loop (every 30 s)
│     ├── refreshes health, active session, today stats, worklog drafts
│     ├── supervises the MLX server (restart budget + cooling period)
│     └── emits Tauri events: status-update, health-update, notices-update …
└── meridian-core readers (shared with the daemon)
      └── DB reads for dashboard commands — no HTTP, no Node
```

**Key source directories:**

| Path | What it is |
|---|---|
| `src-tauri/src/poll/` | Background poll loop — tick cadence, health/session/worklog refresh, MLX supervision, live Tauri event emission |
| `src-tauri/src/commands/` | `#[tauri::command]` surface, grouped by domain: `dashboard.rs` (DB reads), `daemon.rs`, `setup.rs`, `system.rs`, … |
| `src-tauri/src/mlx_server.rs` | MLX child-process lifecycle — resolve, spawn, health-check, supervise, auto-upgrade |
| `src-tauri/src/tray.rs` | Menu builder + menu-event dispatch + window openers |
| `src-tauri/src/capture/` | In-process screen / a11y capture wired to the forked screenpipe-screen / screenpipe-a11y crates |
| `src/` | Popover HTML/CSS/JS (lightweight web UI rendered in the menu-bar window) |

## Development

```bash
# From repo root — starts daemon + MLX + tray with hot reload in 3 Terminal windows
bash dev-start.sh

# Or run the tray alone (daemon + MLX must already be running)
cd tray
npm install
npm run tauri dev
```

`npm run tauri dev` automatically runs `cd ../ui && npm run dev` as its `beforeDevCommand`, serving the Next.js dashboard on `http://localhost:3939`. The tray webview connects to that URL — no separate Next.js terminal is needed.

**Known limitation:** the tray popover 404s under `tauri dev` (the Next.js dev server does not serve `popover/`). The main dashboard window works normally. Test the popover with a production build.

### Rust linting and tests

```bash
cd src-tauri
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

## Production build

```bash
cd tray
bash create-icons.sh   # generate all icon sizes from tray/meridiona-mark.png
npm install
npm run tauri build    # output: src-tauri/target/release/meridian-tray
```

The build bundles the static Next.js export (`ui/out/`) into the binary via Tauri's `frontendDist`. The popover HTML is copied into `ui/out/popover/` by the build step; the main window loads `WebviewUrl::App("today")`.

## Adding a new command

1. Write the Rust fn in `src-tauri/src/commands/<domain>.rs` — DB reads belong in `meridian-core/src/readers/`, file/env/process actions stay in `commands/`.
2. Register it in `src-tauri/src/commands.rs` (glob re-exports) and in `lib.rs`'s `invoke_handler!`.
3. Add it to `capabilities/default.json` if it needs new permissions.
4. Call it from the frontend via `load(apiPath, 'command_name', args)` in `ui/lib/bridge.ts`.

See the full playbook in `CLAUDE.md` → "Porting a dashboard route to Rust".

## Troubleshooting

**Tray icon not appearing / app won't launch**
```bash
# View tray logs
tail -f ~/.meridian/logs/tray.log

# Restart the tray launchd agent
launchctl kickstart -k gui/$(id -u)/com.meridiona.tray
```

**Dashboard shows blank / invoke errors in DevTools**
- The tray starts before the daemon on fresh launch — the DB may not be ready yet. The `subscribe()` bridge retries once after 2 s automatically.
- Check `meridian doctor` for L1 capture or daemon health faults.

**Capture not recording (no frames in `capture_frames`)**
- Grant **Screen Recording** and **Accessibility** to **Meridian** in System Settings → Privacy & Security.
- In dev mode, run `meridian doctor` — `capture.frames` shows the frame count and `capture.freshness` shows how recent the latest frame is.

**MLX server stuck / not classifying**
- `meridian logs mlx-server -f` — look for startup errors.
- The tray supervises the MLX server with a 5-restart budget + 10-tick (~10 min) cooling period before retrying. A persistent failure usually means the runtime needs re-downloading: `meridian setup download-runtime`.
