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

### Prerequisites

- macOS 13+
- Internet connection (for Homebrew + dependency downloads)

`install.sh` handles everything else — Homebrew, Rust, Node 18+, Python 3.11+, and screenpipe itself.

### Install

```bash
git clone https://github.com/meridiona/meridian
cd meridian
./install.sh
```

The installer detects and offers to install each missing prerequisite, then builds the Rust daemon, the MCP server, the Next.js UI, sets up the Python services, walks you through granting screenpipe its three macOS permissions (Screen Recording, Accessibility, Microphone), and registers four launchd LaunchAgents: `com.meridiona.screenpipe`, `com.meridiona.daemon`, `com.meridiona.jira-updater`, and `com.meridiona.ui` (the dashboard at http://localhost:3000).

Useful flags:

- `./install.sh --no-ui` — skip the dashboard build
- `./install.sh --dry-run` — preview actions without executing
- `./install.sh --no-daemon` — build only; don't register launchd agents
- `./install.sh --skip-permissions` — skip the macOS permissions walkthrough
- `./install.sh --skip-env` — skip the credential walkthrough
- `./install.sh --mlx` — use the persistent MLX inference server (Apple Silicon only)

<!-- TODO: expose a stop.sh / `meridian stop` and `meridian start` flow so users don't need to
     know launchctl commands to start/stop services after install. Currently:
       stop:  bash scripts/meridian-cli.sh stop
       start: bash scripts/meridian-cli.sh start   (or re-run install.sh)
     Goal: single documented command for each action, ideally `meridian start` / `meridian stop`,
     with clear notes on what each one covers (screenpipe, daemon, jira-updater, ui, mlx-server). -->

Task classification uses a persistent MLX inference server (Qwen3.5-9B). Set it up once after cloning:

```bash
cd services

# Create a Python 3.13 virtual environment
python3.13 -m venv .venv313

# Install core dependencies + MLX inference extras
.venv313/bin/pip install -e ".[local-llm]"
```

### Configure

`./install.sh` walks you through credential prompts grouped by category:

- **Cloud LLM** — `OPENROUTER_API_KEY` (skip if you're running a local LLM)
- **Jira** — URL, email, API token, project keys (gated by `[y/N]`)
- **GitHub** — personal access token, org, repos
- **Linear** — API key, team IDs
- **Observability (OpenObserve)** — base64 auth + OTLP endpoints

Empty input skips that variable. Values already in the relevant .env file are preserved on re-run.

The credentials are written to three files under the hood, one per daemon:

- `~/.meridian/.env` — Rust daemon (Jira/GitHub/Linear + observability)
- `services/.env` — Python agents (LLM endpoint + Jira + observability)
- `services/.hermes/.env` — hermes-agent library (`OPENROUTER_API_KEY`)

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

To edit credentials later:

```bash
meridian config edit            # opens ~/.meridian/.env in $EDITOR
$EDITOR services/.env           # Python agents
$EDITOR services/.hermes/.env   # hermes
```

To re-run only the credential walkthrough (skipping builds/permissions):

```bash
# Re-prompt: delete the value(s) you want to re-set, then re-run install.sh.
# Note: install.sh skips any variable already populated.
./install.sh --skip-permissions
```

If you want the prompts off entirely:

```bash
./install.sh --skip-env
```

### Start the MLX inference server

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

### Run

```bash
meridian start          # bring up all four daemons (screenpipe + daemon + jira-updater + ui)
meridian status         # check what's running
meridian logs           # tail the Rust daemon log
meridian logs ui        # tail the dashboard log
meridian doctor         # diagnose missing config / services / permissions
meridian permissions    # re-run the screenpipe permissions walkthrough
```

Once started, the dashboard is at **http://localhost:3000**. Stop with `meridian stop`. Remove everything with `meridian uninstall`.

On startup the daemon TCP-connects to the MLX server to verify it is reachable before entering the poll loop. If the server is not running it exits immediately with a clear error message. Stop the daemon with `Ctrl-C` or `SIGTERM`.

## Configuration

These variables are collected interactively by `./install.sh`. The table below is the authoritative reference for what each one means.

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

`./install.sh` builds the MCP server into `packages/meridian-mcp/dist/index.js`. To rebuild it manually:

```bash
cd packages/meridian-mcp && npm run build
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
