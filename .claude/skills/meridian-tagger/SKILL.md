---
name: meridian-tagger
description: "Debug and work with Meridian's Python tagger pipeline. Covers the 3-stage classifier (rules + embeddings + agent), the long-running daemon, hot-toggle config, and common misclassification recipes."
allowed-tools: Bash, Read, Edit, Grep
---

# Meridian Tagger Skill

The tagger reads completed `app_sessions` rows that the Rust ETL committed to `meridian.db`, classifies each session against the user's open Jira tickets, and writes back to `ticket_links` + `session_dimensions` + `dispatch_queue`. It runs as a long-lived Python daemon supervised by launchd.

## When to invoke this skill

- Debugging why a session got tagged with the wrong ticket
- Adding a new rule (e.g. tagging a new tool/topic/practice)
- Tuning Stage 2 score thresholds or Stage 3 prompts
- Investigating why the dashboard's Today's Tickets tile is empty
- Hot-toggling stages on/off without restarting the daemon

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
       ├─ STAGE 1 — rules + regex   (services/agents/rules/)
       │     • 30+ rules emit RuleHits across activity / intent /
       │       engagement / collaboration / tool / topic / practice
       │     • KAN-NN regex hit + lookup in pm_tasks → auto-dispatch
       │     • if no decision → stage1_deferred = True
       │
       ├─ STAGE 2 — embeddings      (services/agents/semantic_matcher.py)
       │     • bge-small-en-v1.5 multi-vector encoding (per OCR sample)
       │     • cosine vs pm_task_embeddings (max-pool over samples)
       │     • blend: 0.55*cosine + 0.30*dim_overlap + 0.15*past_vote
       │     • routes auto / queue / skip by gap + threshold
       │     • runs only when Stage 1 deferred AND stage2_attempted
       │
       └─ STAGE 3 — agent classifier (services/agents/agent_tiebreaker.py)
             • three modes, selected automatically by tagger.py:
               ┌─ tiebreak   — Stage 1+2 both ran; Stage 2 returned queue
               ├─ no_dims    — Stage 2 ran; Stage 1 was disabled
               └─ standalone — Stage 1+2 both disabled; picks from ALL tasks
                               and also infers dimension tags
             • single-shot via hermes AIAgent (run_agent.py)
             • base prompt: services/skills/activity/stage3-agent/SKILL.md
               + mode-specific pipeline context injected at call time
             • JSON-only response, parser tolerant of truncation
```

**Stage interactions:**
- Stage 3 runs in *tiebreak/no_dims* mode only when Stage 2 returns `routing=queue`.
- Stage 3 runs in *standalone* mode when Stage 2 was never attempted (`2 not in stages`).
- Stage 3's verdict is always final — including `null`. It falls back to Stage 2 only when it was unavailable or returned an unparseable response.

## Key Files

| File | Role |
|------|------|
| `services/agents/tagger.py` | per-session pipeline, single-pass cursor advance, CLI inspector |
| `services/agents/tagger_daemon.py` | long-running poll loop, zombie sweep, hot-toggle |
| `services/agents/config.py` | env vars, stage flags, override file helpers |
| `services/agents/db.py` | sqlite3 layer for the agent tables |
| `services/agents/rules/` | rule library (one file per dimension) |
| `services/agents/semantic_matcher.py` | Stage 2 — retrieval + scoring math |
| `services/agents/agent_tiebreaker.py` | Stage 3 — agent call, mode selection, JSON repair |
| `services/agents/embeddings.py` | sentence-transformers loader, BLOB <-> ndarray |
| `services/skills/activity/stage3-agent/SKILL.md` | Stage 3 base prompt (mode-agnostic) |
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

# Live state of stage flags + override file
python -m agents.tagger --stages-status

# Hot-toggle without restarting the daemon
python -m agents.tagger --disable-stage 2
python -m agents.tagger --enable-stage 2
python -m agents.tagger --clear-stages-override

# Inspect a single session — full input + every rule fire + Stage 2/3 trace
python -m agents.tagger --session 1482

# Re-tag a session against the current rules / embeddings / pm_tasks
python -m agents.tagger --session 1482 --no-reset    # keep prior dims, UPSERT

# Show session DB state without re-running anything
python -m agents.tagger --show 1482

# List recent sessions and their current tags
python -m agents.tagger --list-recent 30 --all-history

# Warm pm_task embeddings (after Jira refresh, before first daemon tick)
python -m agents.tagger --embed-tasks

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

-- Sessions tagged by Stage 2 with low confidence (review these first)
SELECT tl.session_id, s.app_name, ROUND(s.duration_s/60.0,1) AS min,
       tl.task_key, tl.routing, ROUND(tl.confidence,2) AS conf
FROM ticket_links tl
JOIN app_sessions s ON s.id = tl.session_id
WHERE tl.method LIKE 'semantic%' AND tl.routing = 'queue'
ORDER BY tl.confidence ASC LIMIT 20;

-- Sessions decided by Stage 3 agent (any mode)
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

-- Sessions where Stage 1 regex matched a ticket-shaped string NOT in pm_tasks
SELECT session_id, task_key, session_type, routing, method
FROM ticket_links
WHERE method = 'stage1_regex' AND task_key IS NULL;

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

### False-positive ticket keys
The regex `\b([A-Z][A-Z0-9]+-\d+)\b` matches `UTF-8`, `GPT-4`, `HTTP-404`, etc. The denylist in `services/agents/rules/__init__.py:_TICKET_FALSE_POSITIVES` filters them. If a new false positive shows up:
- Add it to `_TICKET_FALSE_POSITIVES`
- Add a unit test in `services/agents/tests/test_rules.py`

### OCR domination on noisy sessions
Single-vector encoding averages out the signal when an OCR sample is app chrome (settings menus, banner text). Fixed by multi-sample max-pooling (migration 009). If a session still misclassifies due to OCR noise:
- Run `--session ID` with `LOG_LEVEL=DEBUG` to see which sample (`ocr_3`, `titles`, `audio`) won
- Tune the OCR cap in `services/agents/text_for_embedding.py:_PER_OCR_SAMPLE_CAP`

### Self-reinforcing past_vote
Stage 2's `past_vote` term gives weight to ticket assignments on similar past sessions. Sessions tagged by Stage 2 itself are filtered out (`semantic_matcher.py`'s `method NOT LIKE 'semantic%'`) to prevent ossification of early mistakes. **Never rename the `semantic_*` method strings** without updating that filter.

### Cursor stuck or rewinding
The cursor advances per-session inside `tagger.run_once`'s loop. If it rewinds, something is calling `agent_cursor` UPDATE without the `WHERE ? > last_session_id` guard. Audit `services/agents/db.py:advance_cursor`.

### Stage 3 returning empty/truncated JSON
Thinking models (e.g. nemotron-3-super) burn tokens on internal reasoning before emitting JSON. The truncation repair in `services/agents/agent_tiebreaker.py:_repair_truncated_json` salvages partial responses, but if `AGENT_MAX_TOKENS` is too low we see `agent_invalid_response`. Default is 4000; bump `AGENT_MAX_TOKENS` if needed.

### Stage 3 running in wrong mode
Mode is selected in `tagger.py:_tag_session_inner` automatically:
- `MODE_TIEBREAK` — Stage 1+2 ran, Stage 2 returned `queue`
- `MODE_NO_DIMS` — Stage 1 disabled, Stage 2 ran and returned `queue`
- `MODE_STANDALONE` — Stage 2 not attempted at all (disabled or no tasks)

If Stage 3 fires in standalone mode unexpectedly, check `STAGE2_ENABLED` and that `pm_tasks` is non-empty.

### Daemon idle but cursor lagging
Check `ps aux | grep tagger_daemon`. If the launchd agent isn't running:
```bash
launchctl print gui/$(id -u)/com.meridiona.tagger-daemon | head
services/scripts/install-tagger-daemon.sh   # idempotent — re-bootstraps
```

## Adding a New Rule

1. Pick the right dimension file under `services/agents/rules/` (or create a new one)
2. Decorate a function with `@rule(name="<unique_slug>", dim="<dimension>")`
3. Function takes a session dict and returns `RuleHit | list[RuleHit] | None`
4. Add a unit test in `services/agents/tests/test_rules.py`
5. Run `python -m agents.tagger --session <recent_id>` to verify the new rule fires

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

## Running the Test Suite

```bash
cd services && source .venv/bin/activate
python -m pytest agents/tests/ -v
python -m pytest agents/tests/test_smoke_e2e.py -v   # end-to-end fixture run
```
