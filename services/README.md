# meridian-agents

Python service that runs alongside the Rust daemon. It reads completed `app_sessions` rows from `~/.meridian/meridian.db`, classifies each one through a 3-stage pipeline (rules → embeddings → LLM tiebreak), and writes Jira task mappings and multi-label dimension tags back into the same DB.

The Rust daemon owns all DDL; this service only does SELECT/INSERT/UPDATE on its agent-side tables.

For the deep technical reference (per-stage detail, score formulas, schema, recipes), see [`agents/README.md`](agents/README.md).

---

## What it does

```
app_sessions row (Rust ETL writes it)
        │
        ▼
┌─────────────────────────────────────────────────────────┐
│ Stage 1  rules + ticket regex + trivial-overhead skip   │  no LLM
│   writes session_dimensions, may write ticket_links     │
└─────────────────────────────────────────────────────────┘
        │  (only when Stage 1 found no ticket-shaped string)
        ▼
┌─────────────────────────────────────────────────────────┐
│ Stage 2  bge-small embedding · cosine + dim_overlap +   │  no LLM
│          past_vote → top-K candidates                   │
│   may finalise ticket_links with method=semantic_embed  │
└─────────────────────────────────────────────────────────┘
        │  (only when Stage 2 returns routing=queue)
        ▼
┌─────────────────────────────────────────────────────────┐
│ Stage 3  hermes AIAgent — picks one candidate           │  LLM
│   refines ticket_links with method=agent_tiebreak       │
└─────────────────────────────────────────────────────────┘
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
| `STAGE1_ENABLED` | `1` | Set to `0` to skip Stage 1 (rules + regex). Almost never useful. |
| `STAGE2_ENABLED` | `1` | Set to `0` to skip Stage 2 (embeddings). Stage 1 result is final. |
| `STAGE3_ENABLED` | `1` | Set to `0` to skip Stage 3 (LLM). Stage 2 result is final. |
| `HERMES_DEV_MODE` | `0` | Set to `1` to load hermes from `services/.hermes/` instead of the installed package (see Dev mode below). |

Additional variables (`TAGGER_TICK_SECS`, `ONLY_TODAY`, `SESSION_BATCH_LIMIT`, etc.) are documented in [`agents/README.md`](agents/README.md#configuration).

---

## Running

### One-shot (process all untagged sessions, then exit)

```bash
python -m agents.tagger --once
```

### Long-running daemon

```bash
python -m agents.tagger_daemon
```

Polls every `TAGGER_TICK_SECS` (default 7 s). On each tick it runs `tagger.run_once` over the next batch of unprocessed sessions.

### Debug a single session

```bash
# Re-run all stages with full logging — does NOT write to DB
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

## Hot-toggle stages

The daemon re-reads `~/.meridian/tagger.config.json` every tick. CLI helpers write it for you:

```bash
python -m agents.tagger --stages-status         # show env / override file / resolved set
python -m agents.tagger --enable-stage 3        # turn Stage 3 on live
python -m agents.tagger --disable-stage 3       # turn Stage 3 off live
python -m agents.tagger --clear-stages-override # delete override → fall back to env vars
```

If you launch the daemon with an explicit `--stage 1,2`, the stage set is frozen for that process's lifetime and the override file is ignored.

---

## Jira updater

Fetches in-progress Jira tasks (via `mcp-atlassian`), queries Meridian MCP for session data on each task, generates a bullet-point summary via hermes, and posts as timed comments to Jira. All updates are logged to `jira_update_log` for idempotent deduplication per (task_key, period_start, period_end) slot.

Default schedule: fires at 1 PM and 5 PM within office hours (9–17), looking back over the preceding interval window (default: 4 hours).

### Prerequisites

Set these in `services/.env`:

```bash
JIRA_URL=https://your-instance.atlassian.net
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
| `JIRA_URL` | — | Jira Cloud instance URL. |
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
