# `src/coding_agent_session_ingest` — Claude Code / Codex pipeline

Turns terminal coding-agent transcripts (Claude Code + Codex JSONLs) into
classified `app_sessions` rows, entirely inside the Rust daemon.

Spawned as **gated tokio tasks** from `main.rs`: an indexer that segments JSONLs
into rows, and a summariser that turns sealed rows into prose. Both stay dormant
unless a coding agent is actually present on the machine (device gate). The
local MLX model server provides the `/summarise` endpoint the pipeline
calls over HTTP as the summariser fallback.

---

## The lifecycle (`task_method` column)

A coding-agent row is any `app_sessions` row with a non-NULL
`coding_agent_session_uuid`. It walks one column through its life:

```
coding_agent_live ──seal──▶ pending_summariser ──summarise──▶ pending_classifier ──classify──▶ mlx_direct
   (indexer)                   (indexer)                        (summariser)                     (classify trigger)
   live, re-upserted           sealed, immutable                has session_summary             terminal: task_key set
```

- **`coding_agent_live`** — burst still growing. Re-UPSERTed every indexer tick.
- **`pending_summariser`** — sealed (`sealed_at` set). **Immutable** — the UPSERT
  carries `WHERE sealed_at IS NULL`, so downstream only ever reads sealed rows.
- **`pending_classifier`** — summarised. `session_summary` + `summary_source`
  written. This is the classifier's queue.
- **`mlx_direct`** — classified on its **summary** (not transcript), `task_key`
  assigned. Terminal.

---

## The three stages

### 1. Indexer (`indexer.rs`, `jsonl.rs`, `segment.rs`, `db.rs`)

A low-frequency tokio loop (`run_loop`, default 600 s) plus the SessionEnd hook.

- **`jsonl.rs`** — normalises Claude and Codex's different on-disk schemas into a
  common `NormRecord` (timestamp, cwd, is_turn, is_user, **is_user_prompt**,
  body). Everything downstream is agent-agnostic. `is_user_prompt` is true only
  for a *real* human prompt — Claude logs tool-results as `type:user`, which do
  **not** count.
- **`segment.rs`** — slices one JSONL into segments, splitting on an idle gap
  > 1 h **or** a 1 h time-box. The time-box only cuts at the next real user
  prompt, so a row always ends on a complete assistant turn and the next opens
  on a user message (continuity). One row per `(uuid, segment_started_at)`.
- **`db.rs`** — the segment UPSERT (`ON CONFLICT(coding_agent_session_uuid,
  segment_started_at) … WHERE sealed_at IS NULL`), stale-row sealing, endpoints.
- **`indexer.rs`** — per tick: (1) seal settled live rows (the crash / force-quit
  / sleep backstop), then (2) re-parse changed files and refresh their live tail.
  A never-seen file is backfilled **only if touched today** (local) — a fresh DB
  or post-downtime start never re-indexes weeks of history. On a seal/write it
  notifies the summariser in-process (near-instant wake).

### 2. Summariser (`summariser/`)

Turns each sealed segment into a factual prose summary for the PM work-log.
Claude summarises Claude sessions, Codex summarises Codex sessions, MLX is the
shared fallback. See [`summariser/README.md`](summariser/README.md).

### 3. Classify trigger (`src/intelligence/task_linker/`)

A **non-cursor** branch in the daemon's MLX drain loop classifies
`pending_classifier` rows on their **summary** (`fetch_pending_classifier_sessions`).
`update_coding_agent_task` writes the task fields but **preserves
`session_summary`** (unlike the regular `update_session_task`, which clobbers it).
Lives outside this directory but closes the pipeline.

---

## Entry points

| Path | Trigger | What it does |
|---|---|---|
| `indexer::run_loop` | daemon task | poll + seal + backfill-today |
| `summariser::run_loop` | daemon task | drain today's sealed rows |
| classify trigger | daemon MLX loop | drain `pending_classifier` |
| `hook::run_hook` | `meridian coding-agent-hook` | Claude SessionEnd — seal one session, always exit 0 |

### CLI subcommands (one-shot, manual backfill / debug)

```bash
# Seal one session from a SessionEnd-style JSON payload on stdin
echo '{"transcript_path":"…/<uuid>.jsonl"}' | meridian coding-agent-hook

# Summarise the pending_summariser queue for a day (manual backfill / eval)
meridian coding-agent-summarise [--dry-run] [--day YYYY-MM-DD] [--limit N]

# Classify every summarised (pending_classifier) row via the MLX server
meridian coding-agent-classify
```

> The daemon summariser only drains **today**. To summarise an older day
> (e.g. a historical backlog), run `coding-agent-summarise --day <date>` per day.

---

## Device gate

The whole pipeline is dormant unless `coding_agents_present()` is true (the
`~/.claude/projects` or `~/.codex/sessions` dir exists, or a `claude`/`codex`
binary is on PATH). No coding agent → both tasks log "dormant" and return.

---

## Migrations

| File | Change |
|---|---|
| `027_app_sessions_segments.sql` | segment columns + `(coding_agent_session_uuid, segment_started_at)` unique index |
| `028_drop_day_utc.sql` | dropped the earlier `day_utc` key |

The `(coding_agent_session_uuid, segment_started_at)` unique index is the idempotency
key. Never `DELETE`-then-`INSERT` a coding-agent row from the daemon path.
