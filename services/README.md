# meridian-agents

Python service that runs alongside the Rust daemon. It classifies completed `app_sessions` rows to Jira tasks using a persistent MLX inference server, and posts timed progress comments back to Jira. Writes task mappings and multi-label dimension tags into `~/.meridian/meridian.db`.

The Rust daemon owns all DDL; this service only does SELECT/INSERT/UPDATE on its agent-side tables.

For the deep technical reference (classification logic, schema, recipes), see [`agents/README.md`](agents/README.md).

---

## What it does

```
app_sessions row (Rust ETL writes it)
        │
        │  Rust daemon calls POST /classify_sessions
        ▼           on the persistent MLX server
┌──────────────────────────────────────────────┐
│ MLX inference server (FastAPI, port 7823)    │ Apple Silicon only
│   model: mlx-community/Qwen3.5-9B-OptiQ-4bit│
│   loaded once at startup, served until killed│
│   returns: task_key, session_type, confidence│
└──────────────────────────────────────────────┘
        │
        ▼
Rust writes ticket_links + session_dimensions → meridian.db
```

---

## Installation

Requires Python 3.13 and Apple Silicon.

```bash
cd services/

# Create a Python 3.13 virtual environment
python3.13 -m venv .venv313

# Install core + MLX inference dependencies
.venv313/bin/pip install -e ".[local-llm]"
```

The `hermes-agent` package is fetched from GitHub at the pinned tag; an internet connection is needed on first install.

---

## MLX server

The Rust daemon calls the MLX server for every classification. It must be running before the daemon starts.

### Install as a launchd daemon (recommended)

```bash
bash scripts/install-mlx-server-daemon.sh [--port 7823]

# Verify it started and the model loaded
tail -f ~/.meridian/logs/mlx-server.log
# Expected: "server: MLX model ready"

# Status / stop / restart
launchctl print gui/$(id -u)/com.meridiona.mlx-server
bash scripts/uninstall-mlx-server-daemon.sh
```

### Run manually (development)

```bash
RUST_LOG=meridian=debug CLASSIFIER_BACKEND=mlx cargo run --bin meridian
.venv313/bin/meridian-server --backend mlx --port 7823
python -m agents.server --backend mlx --port 7823
```

The model (`Qwen3.5-9B-OptiQ-4bit`) is downloaded from Hugging Face on first run (~4 GB). Subsequent starts load from local cache in ~5 s.

---

## Configuration

All variables are read in `agents/config.py`. Copy `../.env.example` to `.env` in the repo root and set what you need.

| Variable | Default | Purpose |
|---|---|---|
| `MERIDIAN_DB` | `~/.meridian/meridian.db` | Path to the SQLite file. Must already exist (the Rust daemon creates it). |
| `MLX_SERVER_PORT` | `7823` | Port the MLX inference server listens on. |
| `CLASSIFICATION_ENABLED` | `true` | Set to `false` to disable classification entirely (skips MLX server check). |

Additional variables are documented in [`agents/README.md`](agents/README.md#configuration).

---

## Running

### Task classifier

The task classifier runs automatically — the Rust daemon calls `POST /classify_sessions` on the MLX server on each intelligence tick. To inspect or re-run classification:

```bash
# Re-run classification with full logging — does NOT write to DB
python -m agents.tagger --session <id> --dry-run

# Re-run and persist (resets dims + ticket_link first)
python -m agents.tagger --session <id>

# Read-only view of what's stored
python -m agents.tagger --show <id>
```

### Install / uninstall the launchd daemon

```bash
# Installs plist → ~/Library/LaunchAgents/, starts the service
./scripts/install-tagger-daemon.sh

# Stops and removes the plist
./scripts/uninstall-tagger-daemon.sh

# Status and logs
launchctl print gui/$(id -u)/com.meridiona.tagger-daemon
tail -f ~/.meridian/logs/tagger-daemon.log
tail -f ~/.meridian/logs/tagger-daemon.err
```

---

## Jira updater

Fetches in-progress Jira tasks (via `mcp-atlassian`), queries Meridian MCP for session data on each task, generates a bullet-point summary via hermes, and posts as timed comments to Jira. All updates are logged to `jira_update_log` for idempotent deduplication per (task_key, period_start, period_end) slot.

Default schedule: fires at 1 PM and 5 PM within office hours (9–17), looking back over the preceding interval window (default: 4 hours).

### Prerequisites

Set these in `services/.env`:

```bash
JIRA_BASE_URL=https://your-instance.atlassian.net
JIRA_EMAIL=your-email@example.com
JIRA_API_TOKEN=your-api-token
```

Meridian MCP must be built first:

```bash
cd packages/meridian-mcp
npm run build
```

### Quick command reference

```bash
# One-shot update all in-progress tasks (use current 4-hour window)
python -m agents.jira_updater_daemon --trigger-now

# One-shot update a single task
python -m agents.jira_updater_daemon --task KAN-87

# Preview (print comments without posting to Jira)
python -m agents.jira_updater_daemon --dry-run

# Custom look-back window in hours
python -m agents.jira_updater_daemon --interval 2

# Long-running daemon (sleeps until next scheduled slot)
python -m agents.jira_updater_daemon
```

### Configuration

| Variable | Default | Purpose |
|---|---|---|
| `UPDATE_INTERVAL_HOURS` | `4` | Hours between scheduled slots (e.g. 4 → slots at 13:00, 17:00 within office hours). |
| `OFFICE_START_HOUR` | `9` | Office start hour (UTC). |
| `OFFICE_END_HOUR` | `17` | Office end hour (UTC). |
| `JIRA_POST_NO_ACTIVITY` | `1` | Post comment even if no sessions found in the slot (set to `0` to skip posting no-activity slots). |
| `MERIDIAN_MCP_PATH` | Auto-detected | Path to the compiled MCP server (`packages/meridian-mcp/dist/index.js`). |
| `JIRA_BASE_URL` | — | Jira Cloud instance URL. |
| `JIRA_EMAIL` | — | Email address for Jira API token auth. |
| `JIRA_API_TOKEN` | — | API token for Jira REST API. |

### Install / uninstall the launchd daemon

```bash
# Installs plist → ~/Library/LaunchAgents/, starts the service
./scripts/install-jira-updater-daemon.sh

# Stops and removes the plist
./scripts/uninstall-jira-updater-daemon.sh

# Status and logs
launchctl print gui/$(id -u)/com.meridiona.jira-updater-daemon
tail -f ~/.meridian/logs/jira-updater.log
```

---

## Dev mode (hermes source)

By default the pipeline imports `run_agent` and related modules from the installed `hermes-agent` package. To step into hermes internals instead:

1. Clone the hermes source into `services/.hermes/` (gitignored — do not commit it):

   ```bash
   git clone --branch v2026.4.30 https://github.com/NousResearch/hermes-agent.git services/.hermes
   ```

2. Set `HERMES_DEV_MODE=1`:

   ```bash
   echo "HERMES_DEV_MODE=1" >> services/.env
   ```

`agents/_hermes_setup.py` then prepends `services/.hermes/` to `sys.path` so local source takes precedence over the installed package. All other behaviour is identical. Unset or set `HERMES_DEV_MODE=0` to revert.

### Recovery if the pinned tag disappears

`requirements.txt` pins `hermes-agent` to a Git tag on the NousResearch public repo (`@v2026.4.30`). If that tag is ever deleted or the repo changes visibility, `pip install` will fail. To recover:

1. Obtain the source at that revision (from a team member's local clone or a fork).
2. Point the dependency at your mirror: replace the `git+https://github.com/NousResearch/...@v2026.4.30` URL in `requirements.txt` with your mirror URL.
3. Alternatively, use dev mode (above) with a local copy in `services/.hermes/`.

---

## Tests

```bash
python -m pytest agents/tests/
```

Smoke + unit tests run without external services. Integration tests (marked `integration`) require a live `meridian.db` and an LLM endpoint and are excluded by default.
