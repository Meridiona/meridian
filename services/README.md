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
│   may finalise ticket_links with method=stage2_embed    │
└─────────────────────────────────────────────────────────┘
        │  (only when Stage 2 returns routing=queue)
        ▼
┌─────────────────────────────────────────────────────────┐
│ Stage 3  hermes AIAgent — LLM picks one candidate       │  LLM
│   refines ticket_links with method=stage3_llm           │
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
| `HERMES_MODEL` | `nemotron-3-super` | Model name passed to hermes `AIAgent` for Stage 3. |
| `HERMES_BASE_URL` | `https://ollama.com/v1` | OpenAI-compatible LLM endpoint for Stage 3. |
| `OLLAMA_API_KEY` | — | API key for the LLM endpoint (also accepts standard OpenAI-compat keys). |
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

---

## Tests

```bash
python -m pytest agents/tests/
```

Smoke + unit tests run without external services. Integration tests (marked `integration`) require a live `meridian.db` and an LLM endpoint and are excluded by default.
