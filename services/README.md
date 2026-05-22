# meridian-agents

Python service that runs alongside the Rust daemon. The Rust intelligence module spawns it as a subprocess to classify `app_sessions` rows into Jira task links, and a separate long-running daemon posts timed progress comments back to Jira.

The Rust daemon owns all DDL; this service only does SELECT/INSERT/UPDATE on its agent-side tables.

For the deep technical reference (pipeline, schema, LLM selection), see [`agents/README.md`](agents/README.md).

---

## What it does

```
app_sessions row (Rust ETL writes it)
        │
        │  Rust intelligence module spawns run_task_linker.py
        │  as a one-shot subprocess (JSON stdin → JSON stdout)
        ▼
┌─────────────────────────────────────────────────────────┐
│ Task classifier  (run_task_linker.py)               LLM │
│   hermes AIAgent + skill: task-classifier               │
│   LLM: local-first (LM Studio / Ollama / mlx_lm)       │
│         falls back to cloud (OLLAMA_HOST)               │
│   returns: task_key, confidence, routing per session    │
└─────────────────────────────────────────────────────────┘
        │
        ▼
Rust reads stdout JSON, writes ticket_links, advances cursor

        ┌──────────────────────────────────────────────┐
        │  Jira updater daemon  (jira_updater_daemon)  │
        │  fires on office-hour slots (default 4 h)    │
        │  posts activity summaries as Jira comments   │
        └──────────────────────────────────────────────┘
```

---

## Installation

```bash
cd services/

# Option A — editable install (recommended for development)
python3.11 -m venv .venv
source .venv/bin/activate
pip install -e .

# Option B — bare dependencies only
pip install -r requirements.txt
```

Requires Python 3.11+. The `hermes-agent` package is fetched from the NousResearch GitHub repo at the pinned tag; an internet connection is needed on first install.

---

## Configuration

All variables are read in `agents/config.py`. Copy `.env.example` to `.env` in this directory and set what you need.

| Variable | Default | Purpose |
|---|---|---|
| `MERIDIAN_DB` | `~/.meridian/meridian.db` | Path to the SQLite file. Must already exist (the Rust daemon creates it). |
| `LLM_PREFER_LOCAL` | `1` | On Apple Silicon, try a running local LLM server first; fall back to cloud when none is found. Set to `0` to always use cloud. |
| `LLM_BUDGET_PCT` | `0.5` | Fraction of free Metal GPU memory to allocate when starting an mlx_lm.server (0.0–1.0). `0.8` is recommended on 64 GB+ machines. |
| `OLLAMA_MODEL` | — | Cloud fallback model ID (any OpenAI-compatible endpoint). Used when no local server is detected or `LLM_PREFER_LOCAL=0`. Also the primary LLM config for the Jira updater. |
| `OLLAMA_HOST` | — | Cloud fallback base URL (e.g. `https://api.openai.com/v1`). |
| `OLLAMA_API_KEY` | — | API key for the cloud fallback endpoint. |
| `MIN_LLM_DURATION_S` | `30` | Sessions shorter than this (seconds) are skipped by the classifier. |
| `HERMES_DEV_MODE` | `0` | Set to `1` to load hermes from `services/.hermes/` instead of the installed package (see Dev mode below). |

Additional variables (`AGENT_AUTO_FLOOR`, `AGENT_MAX_TOKENS`, `CONFIDENCE_THRESHOLD`, etc.) are documented in [`agents/README.md`](agents/README.md#configuration).

---

## Running

### Task classifier

The task classifier runs automatically — the Rust daemon spawns `run_task_linker.py` as a subprocess on each intelligence tick. There is no user-facing CLI for it. To verify the selected LLM:

```bash
cd services
.venv/bin/python -c "
from agents.llm_selector import discover_running_servers, select_model_for_hermes
for s in discover_running_servers():
    print(f'running: {s.runtime}  loaded={s.models}')
ep = select_model_for_hermes()
print(f'selected: {ep.model}  runtime={ep.runtime}' if ep else 'cloud fallback')
"
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
