# Meridian

Meridian is a lightweight Rust daemon that reads [screenpipe](https://screenpi.pe)'s
ambient recording database and normalises raw frames into structured, app-based activity
sessions stored in its own SQLite database.

It runs continuously on the developer's machine alongside screenpipe, consuming zero
network and minimal CPU/RAM.

## What it produces

Every time the focused app changes, Meridian closes the previous session and writes a row
to `app_sessions`:

| Column | Description |
|---|---|
| `app_name` | App that owned the session |
| `started_at` / `ended_at` | ISO8601 UTC timestamps |
| `duration_s` | Wall-clock seconds |
| `frame_count` | Number of screenpipe frames in the block |
| `window_titles` | JSON array of `{title, count}` — top windows seen |
| `ocr_samples` | JSON array of up to 20 OCR text samples |
| `elements_samples` | JSON array of up to 20 accessibility tree samples |
| `audio_snippets` | JSON array of transcribed audio during this block |
| `signals` | JSON array of clipboard copies and app-switch events |
| `min_frame_id` / `max_frame_id` | Frame-id range linking back to screenpipe |

## Prerequisites

- macOS (screenpipe records to `~/.screenpipe/db.sqlite`)
- [screenpipe](https://screenpi.pe) running and recording
- Rust 1.93.1 — install via `rustup` or `rust-toolchain.toml` is picked up automatically

## Build

```bash
git clone https://github.com/meridiona/meridian
cd meridian
cargo build --release
```

`SQLX_OFFLINE=true` is set automatically by `.cargo/config.toml` — no manual export needed.

## Run

```bash
./target/release/meridian
```

The daemon starts, runs an immediate ETL pass over all existing screenpipe data, then
polls every 60 seconds for new frames. Stop it with `Ctrl-C` or `SIGTERM`.

## Configuration

All settings are via environment variables; defaults work out of the box.

| Variable | Default | Description |
|---|---|---|
| `SCREENPIPE_DB` | `~/.screenpipe/db.sqlite` | Path to screenpipe's database (read-only) |
| `MERIDIAN_DB` | `~/.meridian/meridian.db` | Path where Meridian writes its database |
| `POLL_INTERVAL_SECS` | `60` | How often to check for new screenpipe frames |

Example:
```bash
POLL_INTERVAL_SECS=30 ./target/release/meridian
```

## Data location

```
~/.meridian/meridian.db       — the normalised database (10 MB per ~9k frames)
```

Query it directly:
```bash
sqlite3 ~/.meridian/meridian.db \
  "SELECT app_name, ROUND(SUM(duration_s)/60.0,1) as min, COUNT(*) as n
   FROM app_sessions GROUP BY app_name ORDER BY min DESC LIMIT 10;"
```

## Development

```bash
# Format
cargo fmt

# Lint (warnings are errors)
cargo clippy -- -D warnings

# Tests
cargo test

# Install git hooks (runs fmt + clippy before each commit)
bash scripts/setup-hooks.sh
```

## Architecture

```
screenpipe.db (read-only)
       │
       ▼
  ETL runner  ──────────────────────────────────────────────┐
  (every 60s)                                               │
       │                                                     │
       ├─ get_frames_since(cursor)                          │
       ├─ detect app-switch boundaries                      │
       ├─ extract_block_context (OCR, audio, signals)       │
       ├─ upsert active_session (open block)                │
       └─ close completed sessions → app_sessions           │
                                                             ▼
                                                    meridian.db
```

## MCP Server

Meridian ships a TypeScript MCP server that exposes your session data to any MCP-compatible AI tool (Claude Code, Claude Desktop, Cursor, etc.).

```bash
cd packages/meridian-mcp
npm install
npm run build
```

Add to your MCP client config (e.g. `~/Library/Application Support/Claude/claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "meridian": {
      "command": "node",
      "args": ["/path/to/meridian/packages/meridian-mcp/dist/index.js"]
    }
  }
}
```

Available tools: `get-sessions`, `get-timeline`, `get-stats`, `get-active-session`, `get-apps`, `search-sessions`, `get-session-detail`, `health-check`.

The MCP server uses [sql.js](https://github.com/sql-js/sql.js) (pure WebAssembly SQLite) — no native Node.js modules, works with any Node.js version.

## License

MIT — see [LICENSE](LICENSE).
