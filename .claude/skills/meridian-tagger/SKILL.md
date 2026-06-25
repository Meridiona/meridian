---
name: meridian-tagger
description: "Debug and work with Meridian's task classification engine. Covers the MLX classifier server, the long-running daemon, and common misclassification recipes."
allowed-tools: Bash, Read, Edit, Grep
---

# Meridian Tagger Skill

The tagger reads completed `app_sessions` rows that the Rust ETL committed to `meridian.db`, classifies each session against the user's open Jira tickets, and writes back to `ticket_links` + `session_dimensions` + `dispatch_queue`. It runs as a long-lived Python daemon supervised by launchd.

## When to invoke this skill

- Debugging why a session got classified with the wrong ticket
- Tuning classifier prompts (MLX classifier server)
- Investigating why the dashboard's Today's Tickets tile is empty

## How the Tagger Works

```
new app_sessions row (Rust ETL, every ~60s)
       │
       ▼
 tagger_daemon.py polls every TAGGER_TICK_SECS (default 7s)
       │
       ├─ pre-filter (services/agents/tagger.py)
       │     trivial overhead → ticket_links overhead/skip, no agent
       │
       └─ Classification Engine (services/agents/agent_tiebreaker.py)
             • single-shot via the Rust task linker (src/intelligence/task_linker/)
             • matches session to task based on screen content + history
             • writes ticket_links with task assignment or overhead marker
             • infers session_dimensions (multi-label activity tags)
             • JSON-only response, parser tolerant of truncation
```

## Key Files

| File | Role |
|------|------|
| `services/agents/tagger.py` | per-session pipeline, single-pass cursor advance, CLI inspector |
| `services/agents/tagger_daemon.py` | long-running poll loop, zombie sweep |
| `services/agents/config.py` | env vars, configuration helpers |
| `services/agents/db.py` | sqlite3 layer for the agent tables |
| `services/agents/agent_tiebreaker.py` | classification engine, agent call, JSON repair |
| `services/agents/_system_context.py` | AI agent system context and capabilities |
| `services/scripts/install-tagger-daemon.sh` | launchd installer |

## Key DB Tables

| Table | Owner | What |
|------|------|------|
| `app_sessions` | Rust (read-only) | the input — completed sessions |
| `pm_tasks` | Rust + Python fallback | open Jira tickets the user can match against |
| `ticket_links` | Python | per-session task assignment (UNIQUE on session_id) |
| `session_dimensions` | Python | multi-label tags (one row per dim/value) |
| `dispatch_queue` | Python | outbox of decisions to push back to Jira (drainer TBD) |
| `agent_runs` | Python | audit log, one row per non-empty tick |
| `agent_cursor` | Python | high-water mark for processed sessions |
| `session_embeddings` | Python | per-sample vectors (multi-row per session) |
| `pm_task_embeddings` | Python | per-task vectors + expected_dims JSON |

## Quick Debug Commands

```bash
cd services && source .venv/bin/activate

# Inspect a single session — full input + classification trace
python -m agents.tagger --session 1482

# Re-classify a session
python -m agents.tagger --session 1482 --no-reset    # keep prior dims, UPSERT

# Show session DB state without re-running anything
python -m agents.tagger --show 1482

# List recent sessions and their current tags
python -m agents.tagger --list-recent 30 --all-history

# Tail daemon logs
tail -f ~/.meridian/logs/tagger-daemon.log
```

## Useful Debug SQL

```bash
sqlite3 ~/.meridian/meridian.db
```

```sql
-- Today's tag distribution
SELECT tl.method, tl.session_type, tl.routing, COUNT(*) AS n
FROM ticket_links tl
JOIN app_sessions s ON s.id = tl.session_id
WHERE date(s.started_at) >= date('now', 'localtime')
GROUP BY tl.method, tl.session_type, tl.routing
ORDER BY n DESC;

-- Sessions with low confidence (review these first)
SELECT tl.session_id, s.app_name, ROUND(s.duration_s/60.0,1) AS min,
       tl.task_key, tl.routing, ROUND(tl.confidence,2) AS conf
FROM ticket_links tl
JOIN app_sessions s ON s.id = tl.session_id
WHERE tl.routing = 'queue'
ORDER BY tl.confidence ASC LIMIT 20;

-- Sessions classified by the agent
SELECT tl.session_id, s.app_name, tl.task_key, tl.routing,
       ROUND(tl.confidence,2) AS conf, tl.method
FROM ticket_links tl
JOIN app_sessions s ON s.id = tl.session_id
WHERE tl.method LIKE 'agent%'
ORDER BY tl.session_id DESC LIMIT 20;

-- Cursor + backlog
SELECT 'cursor' AS k, last_session_id AS v FROM agent_cursor
UNION ALL SELECT 'max_session_id', MAX(id) FROM app_sessions
UNION ALL SELECT 'backlog', (SELECT MAX(id) FROM app_sessions) - last_session_id FROM agent_cursor;

-- Recent agent_runs
SELECT id, status, sessions_processed, links_written, dispatches_queued, started_at
FROM agent_runs ORDER BY id DESC LIMIT 10;

-- Top dimensions for a session
SELECT dimension, value, ROUND(confidence,2) AS conf, source
FROM session_dimensions
WHERE session_id = ?  -- substitute id
ORDER BY dimension, conf DESC;

-- pm_task_embeddings expected_dims (what Stage 2 thinks each task is about)
SELECT task_key, json_extract(expected_dims, '$.topic') AS topics,
       json_extract(expected_dims, '$.activity') AS activities
FROM pm_task_embeddings ORDER BY task_key;
```

## Common Pitfalls

### Classification returning empty/truncated JSON
Thinking models (e.g. nemotron-3-super) burn tokens on internal reasoning before emitting JSON. The truncation repair in `services/agents/agent_tiebreaker.py:_repair_truncated_json` salvages partial responses, but if `AGENT_MAX_TOKENS` is too low we see `agent_invalid_response`. Default is 4000; bump `AGENT_MAX_TOKENS` if needed.

### Cursor stuck or rewinding
The cursor advances per-session inside `tagger.run_once`'s loop. If it rewinds, something is calling `agent_cursor` UPDATE without the `WHERE ? > last_session_id` guard. Audit `services/agents/db.py:advance_cursor`.

### Daemon idle but cursor lagging
Check `ps aux | grep tagger_daemon`. If the launchd agent isn't running:
```bash
launchctl print gui/$(id -u)/com.meridiona.tagger-daemon | head
services/scripts/install-tagger-daemon.sh   # idempotent — re-bootstraps
```

## Reset and Re-tag from Scratch

```bash
# Sweep stale running rows + reset cursor
sqlite3 ~/.meridian/meridian.db "
  UPDATE agent_runs SET status='aborted' WHERE status='running';
  UPDATE agent_cursor SET last_session_id = 0 WHERE id = 1;
  DELETE FROM ticket_links;
  DELETE FROM session_dimensions;
  DELETE FROM dispatch_queue WHERE state = 'pending';
"

# Wait for the daemon to drain the backlog (or restart it)
launchctl kickstart -k gui/$(id -u)/com.meridiona.tagger-daemon
tail -f ~/.meridian/logs/tagger-daemon.log
```

## Backfill

Both backfill tools are Rust binaries that bypass the live cursor — safe to re-run.

### Session categories (Foundation Models)

```bash
# Re-classify sessions from today or yesterday
cargo run --bin backfill_session_categories -- --today
cargo run --bin backfill_session_categories -- --yesterday

# Explicit date range
cargo run --bin backfill_session_categories -- --from-date 2025-05-01 --to-date 2025-05-14

# By session id
cargo run --bin backfill_session_categories -- --from-id 100 --to-id 500
cargo run --bin backfill_session_categories -- --from-id 100   # from 100 onwards

# Dry run — print without writing
cargo run --bin backfill_session_categories -- --dry-run --today
```

### Task classification (MLX)

```bash
# Re-link sessions to Jira tasks from today or yesterday
cargo run --bin backfill_task_classification -- --today
cargo run --bin backfill_task_classification -- --yesterday

# Explicit date range
cargo run --bin backfill_task_classification -- --from-date 2025-05-01 --to-date 2025-05-14

# By session id
cargo run --bin backfill_task_classification -- --from-id 100 --to-id 500
cargo run --bin backfill_task_classification -- --from-id 100   # from 100 onwards

# Dry run — print sessions that would be classified without writing
cargo run --bin backfill_task_classification -- --dry-run --today
```

Neither backfill tool touches the live cursor (`agent_cursor`), so they are safe to run while the daemon is active.

## Running the Test Suite

```bash
cd services && source .venv/bin/activate
python -m pytest agents/tests/ -v
python -m pytest agents/tests/test_smoke_e2e.py -v   # end-to-end fixture run
```
