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
| `category` | AI-assigned activity category (e.g. `coding`, `meeting`, `research`) |
| `confidence` | Category confidence score (0.0–1.0) |
| `window_titles` | JSON array of `{title, count}` — top windows seen |
| `ocr_samples` | JSON array of up to 20 deduplicated OCR text samples |
| `elements_samples` | JSON array of up to 20 deduplicated accessibility tree samples |
| `audio_snippets` | JSON array of transcribed audio (stored in DB; not sent to LLMs) |
| `signals` | JSON array of deduplicated clipboard copies and app-switch events |
| `min_frame_id` / `max_frame_id` | Frame-id range linking back to screenpipe |

## Prerequisites

- macOS with Apple Silicon (the MLX inference server requires Metal)
- [screenpipe](https://screenpi.pe) running and recording
- Rust 1.93.1 — install via `rustup` or `rust-toolchain.toml` is picked up automatically
- Python 3.13 — install via `brew install python@3.13` or `pyenv install 3.13`

## Getting started

### 1. Clone and build

```bash
git clone https://github.com/meridiona/meridian
cd meridian
cargo build --release
```

`SQLX_OFFLINE=true` is set automatically by `.cargo/config.toml` — no manual export needed.

### 2. Set up the Python services layer

Task classification uses a persistent MLX inference server (Qwen3.5-9B). Set it up once after cloning:

```bash
cd services

# Create a Python 3.13 virtual environment
python3.13 -m venv .venv313

# Install core dependencies + MLX inference extras
.venv313/bin/pip install -e ".[local-llm]"
```

### 3. Configure environment

Copy the example and fill in your credentials:

```bash
cp .env.example .env
$EDITOR .env
```

Minimum required variables:

```bash
# Jira (for task classification and Jira updater)
JIRA_BASE_URL=https://your-instance.atlassian.net
JIRA_EMAIL=you@example.com
JIRA_API_TOKEN=your-api-token

# Enable task classification
CLASSIFICATION_ENABLED=true
```

> Set `CLASSIFICATION_ENABLED=false` to skip classification — the daemon runs ETL and FM categorisation only, with no MLX server needed.

### 4. Start the MLX inference server

The Rust daemon calls this server for every session classification. It must be running before the daemon starts.

**Option A — launchd daemon (recommended, survives reboots and crashes):**

```bash
bash services/scripts/install-mlx-server-daemon.sh
# Check it started:
tail -f ~/.meridian/logs/mlx-server.log
```

**Option B — run manually in a terminal (dev/debugging):**

```bash
cd services
.venv313/bin/meridian-server --backend mlx --port 7823
```

The server loads the model once at startup (~30 s on first run while the model downloads). Subsequent starts from cache are ~5 s. You will see `server: MLX model ready` in the log when it is ready.

### 5. Run the daemon

```bash
./target/release/meridian
```

On startup the daemon TCP-connects to the MLX server to verify it is reachable before entering the poll loop. If the server is not running it exits immediately with a clear error message. Stop the daemon with `Ctrl-C` or `SIGTERM`.

## Configuration

All settings are via environment variables; defaults work out of the box.

| Variable | Default | Description |
|---|---|---|
| `SCREENPIPE_DB` | `~/.screenpipe/db.sqlite` | Path to screenpipe's database (read-only) |
| `MERIDIAN_DB` | `~/.meridian/meridian.db` | Path where Meridian writes its database |
| `POLL_INTERVAL_SECS` | `60` | How often to check for new screenpipe frames |
| `CLASSIFICATION_ENABLED` | `true` | Enable session→task classification via MLX server |
| `MLX_SERVER_PORT` | `7823` | Port the persistent MLX inference server listens on |
| `CLASSIFIER_BACKEND` | `mlx` | Classification backend (`mlx` is the only supported value) |
| `CLASSIFICATION_TIMEOUT_S` | `120` | Per-session inference timeout in seconds |

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

## Utility Scripts

| Script | Purpose |
|---|---|
| `services/scripts/install-mlx-server-daemon.sh` | Install the MLX server as a launchd daemon (KeepAlive, auto-restart). |
| `services/scripts/uninstall-mlx-server-daemon.sh` | Stop and remove the MLX server daemon. |
| `scripts/refresh_pm_tasks.py` | Force-refresh `pm_tasks` from Jira without restarting the daemon. |
| `scripts/setup-hooks.sh` | Install git hooks (fmt + clippy pre-commit, full suite pre-push). |

```bash
# Install MLX server as a background daemon
bash services/scripts/install-mlx-server-daemon.sh [--port 7823]

# MLX server logs
tail -f ~/.meridian/logs/mlx-server.log

# Force-refresh Jira task cache
python3 scripts/refresh_pm_tasks.py

# Custom JQL or DB path
python3 scripts/refresh_pm_tasks.py --jql "project=KAN ORDER BY updated DESC"
python3 scripts/refresh_pm_tasks.py --db /path/to/meridian.db
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
  ETL runner  ──────────────────────────────────────────────────┐
  (every 60s)                                                   │
       │                                                         │
       ├─ get_frames_since(cursor)                              │
       ├─ detect app-switch boundaries                          │
       ├─ extract_block_context (OCR, audio, signals)           │
       ├─ upsert active_session (open block)                    │
       └─ close completed sessions → app_sessions               │
                                                                 ▼
                                                        meridian.db
                                                                 │
                                                                 ▼
  Classification ──────── POST /classify_sessions ─────► MLX server
  (per tick)              http://127.0.0.1:7823          (Qwen3.5-9B,
       │                                                  model in memory)
       └─ writes ticket_links, session_dimensions ────────────► meridian.db
```

The MLX server is a long-running FastAPI process managed by launchd. It loads the model once at startup — no cold-load penalty per session. The Rust daemon HTTP-calls it and writes the classification results back to `meridian.db`.

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

> **Note**: `audio_snippets` are stored in the DB but intentionally excluded from MCP tool responses to reduce LLM noise. They remain searchable via `search-sessions`.

The MCP server uses [sql.js](https://github.com/sql-js/sql.js) (pure WebAssembly SQLite) — no native Node.js modules, works with any Node.js version.

## License

MIT — see [LICENSE](LICENSE).
