# Meridian agent service

Python service that runs alongside the Rust daemon and the Next.js dashboard. It reads completed `app_sessions` rows out of `~/.meridian/meridian.db`, classifies each one through a 3-stage pipeline (rules → embeddings → LLM tiebreak), and writes Jira task mappings + multi-label dimension tags back into the same DB. The Rust daemon owns all DDL; this service only does SELECT/INSERT/UPDATE on a small number of agent-side tables.

For the deep technical reference (per-stage detail, schema, tuning, recipes for adding rules / swapping models), see [`agents/README.md`](agents/README.md). For the Rust-side ETL and product rules, see the project root `CLAUDE.md`.

---

## Quickstart

### Prerequisites

- Python **3.11+** (see `services/pyproject.toml`)
- The Rust daemon already running, with at least one `app_sessions` row present at `~/.meridian/meridian.db` — start screenpipe + meridian first
- A working LLM endpoint if you want Stage 3 to fire. Defaults: `HERMES_BASE_URL=https://ollama.com/v1`, `HERMES_MODEL=nemotron-3-super`, `OLLAMA_API_KEY=…` (read from `~/.hermes/.env` if present)
- (Optional) Atlassian creds in `JIRA_URL` / `JIRA_EMAIL` / `JIRA_API_TOKEN` if you want the eventual dispatcher to write to Jira

### Install

```bash
cd services/
python3.11 -m venv .venv
source .venv/bin/activate
pip install -e .
```

### First-run sanity check

Before installing the daemon, run a single manual cycle and inspect the output:

```bash
# Run a full pass over the next batch (default 50 unprocessed sessions)
python -m agents.tagger --once

# Inspect one session end-to-end (rules → stage 2 → stage 3) without persisting
python -m agents.tagger --session 1234 --dry-run

# List the most recent 20 sessions and their tags
python -m agents.tagger --list-recent
```

If `--once` exits cleanly with `tickets_decided` >= 0 and the log at `~/.meridian/logs/tagger.log` is non-empty, the pipeline is wired up correctly.

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
        │  (only when Stage 1 deferred — no ticket-shaped string seen)
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

- **Stage 1** (`agents/tagger.py`, `agents/rules/`) — deterministic regexes against window titles, OCR, and audio. Writes multi-label dimensions (`activity`, `intent`, `engagement`, `tool`, `topic`, …) and resolves ticket keys that match an active `pm_tasks` row. Sessions that fail the trivial-overhead pre-filter (duration < `MIN_LLM_DURATION_S`, no titles/OCR/audio) get tagged `overhead/skip` and don't see Stage 2 or 3.
- **Stage 2** (`agents/stage2.py`) — only runs when Stage 1 didn't see a ticket-shaped string at all. Embeds the session as a multi-vector matrix (titles + audio + per-OCR-sample), max-pool cosine against `pm_task_embeddings`, blends with Stage-1 `dim_overlap` and a softmax-weighted `past_vote` over similar tagged sessions. Routing decision: `auto` (top1 ≥ 0.62 and gap ≥ 0.08), `queue` (≥ 0.40), or `skip`.
- **Stage 3** (`agents/stage3.py` + `skills/activity/stage3-tiebreaker/SKILL.md`) — only runs when Stage 2 returns `routing=queue`. Calls hermes `AIAgent` in single-shot mode (`max_iterations=1`, no toolsets) with the candidate descriptions, parses one JSON object back, and writes the final `ticket_links` row.

The **daemon** (`agents/tagger_daemon.py`) wraps `tagger.run_once` in a polling loop. Every `TAGGER_TICK_SECS` (default 7s) it checks `app_sessions.id > agent_cursor.last_session_id`. If there's nothing new, the tick is one cheap `SELECT 1` and the loop sleeps. On startup it sweeps any zombie `agent_runs` rows left in `'running'` by a previous crash, mirroring the Rust daemon's `cleanup_incomplete_runs`.

---

## Common ops

### Install / uninstall the launchd daemon

```bash
# Install — symlinks the plist into ~/Library/LaunchAgents/, kickstarts it
./services/scripts/install-tagger-daemon.sh

# Uninstall — bootouts and removes the plist
./services/scripts/uninstall-tagger-daemon.sh

# Status / logs
launchctl print gui/$(id -u)/com.meridiona.tagger-daemon
tail -f ~/.meridian/logs/tagger-daemon.log
tail -f ~/.meridian/logs/tagger-daemon.err
```

The plist template is at `services/scripts/com.meridiona.tagger-daemon.plist` — env vars (`MERIDIAN_DB`, `STAGE{1,2,3}_ENABLED`, `ONLY_TODAY`, `TAGGER_TICK_SECS`) are set inline there.

### Inspect a single session

```bash
# Re-run all stages on session 1234 with full dump (resets dimensions/ticket_link first)
python -m agents.tagger --session 1234

# Read-only view of what's stored right now
python -m agents.tagger --show 1234

# Last 20 sessions in a compact table
python -m agents.tagger --list-recent
```

### Hot-toggle stages (without restarting the daemon)

The daemon re-reads `~/.meridian/tagger.config.json` every tick when invoked with the default `--stage auto`. CLI helpers write the file for you:

```bash
python -m agents.tagger --stages-status         # show resolved live set
python -m agents.tagger --disable-stage 3       # turn Stage 3 off live
python -m agents.tagger --enable-stage 3        # back on
python -m agents.tagger --clear-stages-override # delete the override file
```

If you launched the daemon with an explicit `--stage 1,2`, the stage set is frozen for that process's lifetime — predictable for ad-hoc runs.

### Embed pm_tasks (warm-up)

The first Stage 2 cycle embeds every active `pm_tasks` row, which can take a few seconds. Run this once after Jira sync populates `pm_tasks`:

```bash
python -m agents.tagger --embed-tasks
```

Subsequent runs only re-embed when the task title/description/etc. changes (tracked via `text_hash`).

---

## Pointers

- Deep technical reference: [`services/agents/README.md`](agents/README.md)
- Rust ETL rules + repository layout: project-root `CLAUDE.md`
- Stage-3 system prompt: [`services/skills/activity/stage3-tiebreaker/SKILL.md`](skills/activity/stage3-tiebreaker/SKILL.md)
- Schema source of truth (Rust-owned): `src/migrations/003_intelligence.sql`, `005_agents.sql`, `007_session_dimensions.sql`, `008_session_embeddings.sql`, `009_multi_sample_embeddings.sql`
- Logs: `~/.meridian/logs/tagger.log` (CLI runs), `~/.meridian/logs/tagger-daemon.log` (long-running daemon)
