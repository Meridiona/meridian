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

The installer detects and offers to install each missing prerequisite, then builds the Rust daemon, the MCP server, the Next.js UI, sets up the Python services, walks you through granting screenpipe its three macOS permissions (Screen Recording, Accessibility, Microphone), and registers three launchd LaunchAgents: `com.meridiona.screenpipe`, `com.meridiona.daemon`, and `com.meridiona.jira-updater`.

Useful flags:

- `./install.sh --no-ui` — skip the dashboard build
- `./install.sh --dry-run` — preview actions without executing
- `./install.sh --no-daemon` — build only; don't register launchd agents
- `./install.sh --skip-permissions` — skip the macOS permissions walkthrough
- `./install.sh --skip-env` — skip the credential walkthrough

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

To edit any of them later:

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

### Run

```bash
meridian start          # bring up all three daemons (screenpipe + daemon + jira-updater)
meridian status         # check what's running
meridian logs           # tail the Rust daemon log
meridian doctor         # diagnose missing config / services / permissions
meridian permissions    # re-run the screenpipe permissions walkthrough
```

Stop with `meridian stop`. Remove everything with `meridian uninstall`.

## Configuration

These variables are collected interactively by `./install.sh`. The table below is the authoritative reference for what each one means.

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

## Utility Scripts

| Script | Purpose |
|---|---|
| `scripts/setup-services.sh` | One-time setup for the Python services layer — creates venv, installs deps, configures hermes. Run after cloning. |
| `scripts/refresh_pm_tasks.py` | Force-refresh `pm_tasks` from Jira without restarting the daemon. Stdlib only — no pip install needed. |
| `scripts/setup-hooks.sh` | Install git hooks (fmt + clippy pre-commit, full suite pre-push). |

```bash
# Force-refresh Jira task cache now
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
