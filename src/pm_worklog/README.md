# `src/pm_worklog` — Jira worklog automation (Stage 4)

The last stage of the pipeline: turn classified `task` sessions into **Jira
worklogs**, one per `(task, hour)`, entirely in Rust **except the single LLM hop**
(the agno synthesiser, hosted on the MLX server's `/synthesise_worklog` endpoint).

It is the consumer of every upstream stage — it reads sessions that ETL captured
(Stage 0), the classifier mapped to a ticket (Stage 1), and the coding-agent
ingest summarised + classified (Stages 2–3) — so it must run *after* those have
settled for an hour.

---

## The flow

```
collect ─▶ synthesise ─▶ ground ─▶ route
(SQL)      (gated LLM)    (pure)     (persist + Jira POST)
```

- **collect** (`collect.rs`) — assemble the `SessionBundle` for one `(task, hour)`
  window from `meridian.db`: classified `task` sessions + their summaries/excerpts,
  the ticket context from `pm_tasks`, the idle-discounted `real_seconds`, and the
  already-posted "earlier today" summaries (so the synth doesn't repeat itself).
- **synthesise** (`synth.rs`) — the **only** LLM call. POST the bundle to the MLX
  server's `/synthesise_worklog`, which runs the agno synth agent in-process and
  returns a `JiraUpdate` (a 2–4 line summary + evidence-bearing bullets). Wrapped
  in the global LLM gate (see below).
- **ground** (`ground.rs`) — drop every bullet that cites no session, compute
  coverage, attach risk flags (`low_confidence` / `ticket_closed_upstream` /
  `cross_ticket_leak` / `low_evidence`). Pure logic, no LLM.
- **route** (`route.rs`) — persist the worklog row, then (unless dry-run) post it
  to Jira and stamp it POSTED. Idempotent: a window already posted short-circuits.

---

## The hour-driven driver (`scheduler.rs`)

Walks each day's hours from **local-midnight → now**:

```
for each hour H:
    if H is done in the ledger → skip
    if H not over yet         → skip (current incomplete hour)
    if H is READY:
        for each task with classified work in H:  collect → synth → ground → route
        mark H done (even with 0 tasks)
    else: leave for the next pass   # does NOT block later hours
```

Hours are **independent** — a not-ready hour never blocks later hours, so one
stuck classification can't freeze the day.

### Readiness (`ledger.rs`)

An hour `H` is processed when `now ≥ H_end` **and**:

- **upstream settled** — ETL has crossed the hour boundary *and* no session
  started in the hour is still **in-flight**. "In-flight" mirrors the classifier's
  own candidate rule, not a crude "any unclassified row" test: a row blocks only
  if the pipeline will still advance it —
    - regular row the classifier will pick up:
      `task_method IS NULL AND duration_s > min_classification_duration_s`, or
    - coding-agent row still mid-pipeline:
      `task_method IN ('coding_agent_live','pending_summariser','pending_classifier')`.

  A sub-threshold blip (`duration_s ≤ min_classification_duration_s`) is **ignored**
  — the classifier never touches it, so its `task_session_type` stays NULL forever
  and waiting for it would be a bug. (It also never becomes a `task` row, so
  ignoring it loses no worklog content.) The min-duration value is sourced from the
  same `Config.min_classification_duration_s` the classifier uses, so the two stages
  agree by construction; **or**
- **aged out** — `H` has been over longer than `PM_WORKLOG_READINESS_AGING_MIN`
  (default 90 min). This is the escape hatch: after the aging window we process
  best-effort with whatever is classified, so a genuinely-stuck row (e.g. a crashed
  summariser) can never deadlock the day. With the in-flight predicate this is once
  again the *rare* backstop it was designed to be — previously, any hour containing
  a short blip could only ever fire via this path.

### The ledger table

`pm_worklog_hours(hour_start PK, day_utc, hour_end, status, task_count, processed_at)`
records every hour's state. `status='done'` (incl. 0-task hours) means it's never
re-scanned. "Where are we today?" is one `SELECT`.

---

## Known limitation: cross-hour attribution of coding segments

A session is bucketed into an hour by its **`started_at`** (`tasks_in_hour` /
`collect.rs` use `started_at >= H AND started_at < H+1`). Screen sessions are
short ETL blocks, so this is exact for them. A **coding-agent segment**, however,
can span up to an hour (the 1 h time-box), and the whole segment is billed to the
hour it *started* in — so work that physically happened in the next clock hour is
logged under the earlier hour's worklog.

> Example: a coding segment running 2:50 → 3:50 is billed entirely to the
> **2–3 pm** worklog, even though most of its minutes fall in the 3 pm clock hour.

The **daily total per ticket stays correct** (segments sum without loss or
double-count); only the per-hour distribution can shift by up to one segment.
This is accepted by design for now. The clean fix, if per-hour accuracy is later
required, is overlap-based apportionment in the collect layer (split each
segment's time across the hours it overlaps), with the segment's narrative
attached only to its majority hour — contained entirely to `src/pm_worklog/`,
no change to segmentation.

---

## The single LLM gate

Stage 1 (classify), Stages 2–3 (summarise fallback), and Stage 4 (synth) all call
the **one** local MLX model. They share a process-global `Semaphore(1)`
(`crate::llm_gate`) acquired per request, so **exactly one model call is ever in
flight** — the classifier and the worklog synthesiser can never contend on the
GPU. (The gate is per-process; it serialises the daemon's tokio tasks. A
standalone `pm-worklog` CLI run has its own gate, so don't run it against a live
daemon.)

---

## Idempotency — never double-post to Jira

- `pm_worklogs` is keyed `(task_key, day_utc, cycle_index)`; a re-run UPSERTs and
  replaces a **DRAFTED** row.
- A partial unique index (`uq_pm_worklogs_worklog_window`) covers only **POSTED**
  rows, and `find_existing_worklog` short-circuits a window that already has a
  Jira worklog id — so restarts and backfills never post twice.

---

## Safety: posting is off by default

The daemon driver runs in **dry-run** (drafts rows, never POSTs) until
`PM_WORKLOG_POST_ENABLED=true`. The CLI `--dry-run` flag forces it independently.
`time_spent` always comes from `real_seconds` (the idle-discounted figure computed
in `collect.rs`, capped at one hour) — the LLM never decides how much time to log.

---

## Files

| File | Role |
|---|---|
| `models.rs` | `SessionBundle` / `JiraUpdate` / … — the JSON contract with the endpoint |
| `config.rs` | `PmWorklogConfig::from_env()` |
| `collect.rs` | build the bundle (SQL read) |
| `synth.rs` | gated POST to `/synthesise_worklog` |
| `ground.rs` | drop un-evidenced bullets, coverage, risk flags |
| `db.rs` | `pm_worklogs` + evidence upserts, find/mark for idempotency |
| `ledger.rs` | hour ledger + readiness predicate + per-hour task discovery |
| `route.rs` | per-task: collect → synth → ground → persist → post |
| `scheduler.rs` | the hour-driven driver + daemon loop + CLI |

The agno synth agent itself stays in Python (`services/agents/pm_worklog_update/`
+ the `/synthesise_worklog` endpoint in `services/agents/server.py`).

---

## CLI

```bash
# Draft (never post) one day's worklogs — the parity/preview path
meridian pm-worklog --day 2026-05-30 --dry-run

# Real run for today (only posts if PM_WORKLOG_POST_ENABLED=true)
meridian pm-worklog
```

---

## Config (env)

| Env | Default | Purpose |
|---|---|---|
| `PM_WORKLOG_POST_ENABLED` | `false` | Master switch — `true` lets the daemon POST to Jira |
| `PM_WORKLOG_INTERVAL_HOURS` | `1` | Driver pass cadence |
| `PM_WORKLOG_MIN_CONFIDENCE` | `0.65` | Below this → `low_confidence` flag |
| `PM_WORKLOG_READINESS_AGING_MIN` | `90` | Aging escape — max wait for an hour to settle |
| `PM_WORKLOG_MIN_POST_SECONDS` | `60` | Jira's worklog floor |
| `PM_WORKLOG_SYNTH_TIMEOUT_S` | `300` | Synth HTTP timeout |
| `MLX_SERVER_HOST` / `MLX_SERVER_PORT` | `127.0.0.1` / `7823` | synth endpoint |
| `JIRA_URL` / `JIRA_EMAIL` / `JIRA_API_TOKEN` | — | worklog POST auth (reused from the Jira provider) |
