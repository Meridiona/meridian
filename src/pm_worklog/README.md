# `src/pm_worklog` — PM worklog / ticket write-back

The PM-provider worklog + ticket layer: turn a day-task workstream into a drafted
status update matched to (or proposing) the ticket(s) it advanced, then — on human
approval — post it to the real tracker. **Entirely in Rust**; the single LLM hop
runs through the user's chosen third-party CLI provider (Claude / Codex / Cursor /
Copilot CLI, or a custom OpenAI-compatible cloud endpoint) via
[`crate::llm::complete`] — there is no local model server and no Python.

It is the consumer of every upstream stage — it reads sessions that ETL captured,
that the categorizer mapped to a ticket, and that the coding-agent ingest
summarised — so it runs *after* those have settled for an hour. The old hourly
`collect → synthesise → ground → route` Stage-4 driver was superseded by the
per-card `generate` engine below and has been removed.

---

## The per-card generate engine (`generate.rs`)

The "Generate worklog" action behind a day-task card. ONE provider-agnostic LLM
call matches a day-level workstream to the existing ticket(s) it advanced — one or
several — or proposes a new one, and writes a high-level status update:

```
generate ─▶ (review) ─▶ approve ─┬─ create ticket (if proposed)   `create.rs`
(LLM match/propose)               ├─ post the update as a plain    `post_comment.rs`
                                  │  comment on each ticket
                                  └─ link the day-task
```

- **generate** (`generate.rs`) — the only LLM call. Matches the workstream to
  tickets (or proposes one) and drafts the status update. Wrapped in the global
  LLM gate (see below); `time_spent` always comes from the idle-discounted
  `real_seconds`, never from the model.
- **approve** (`generate::approve`) — on approval: `create.rs` creates the ticket
  if it was a proposal (resolving the target per provider), then `post_comment.rs`
  posts the update as a **plain status-update comment** (not a native worklog — no
  time marker) to each matched ticket, and the day-task is linked.
- **Partial delivery is a real state.** Approving across several tickets is not
  atomic and a comment cannot be un-posted, so each target carries its own posted
  flag (`meridian_core::day_task_worklogs::targets`) and a retry only posts the
  ones still outstanding.

---

## The approved-poster sweep (`post.rs`)

The **only** path that writes to a real tracker. The hourly draft driver
(`src/worklog_pipeline/`) and the per-card engine only ever DRAFT; a human reviews,
edits, and approves in the dashboard, which flips the row to `approved`. This sweep
(~60 s, independent of the hourly driver so "approve in the UI" feels immediate)
picks approved rows up and posts to whichever tracker the row belongs to:

```
jira   → native worklog endpoint          (`jira.rs`)
linear → structured commentCreate         (`linear.rs`)  — no native worklog API
github → structured issue comment (REST)  (`github.rs`)  — no native time tracking
```

Idempotent per-row: a row already POSTED short-circuits (`find_existing_worklog`),
so a restart mid-sweep never double-posts. A ticket may legitimately carry more
than one worklog for the same hour (a manual re-match creates a sibling row), so
idempotency is scoped per-row (`id`), not per `(task, window)`.

---

## The hour ledger + readiness (`ledger.rs`)

`pm_worklog_hours(hour_start PK, day_utc, hour_end, status, task_count,
processed_at)` records, per `(day, hour)`, whether that hour has been processed.
The driver walks hours from local-midnight forward and processes each READY hour,
recording even 0-task hours as done so they are never re-scanned. Hours are
**independent** — a not-ready hour never blocks later hours, so one stuck upstream
row can't freeze the day.

An hour `H` is READY when `now ≥ H_end` **and** either:

- **upstream settled** — ETL has crossed the hour boundary *and* no session started
  in the hour is still **in-flight**. "In-flight" mirrors the categorizer's own
  candidate rule, not a crude "any unclassified row" test: a row blocks only if the
  pipeline will still advance it —
    - a regular row: `task_method IS NULL AND duration_s > min_classification_duration_s`, or
    - a coding-agent row still mid-pipeline:
      `task_method IN ('coding_agent_live','pending_summariser')`.

  A sub-threshold blip (`duration_s ≤ min`) is ignored — the categorizer never
  touches it, so waiting for it would be a bug; **or**
- **aged out** — `H` has been over longer than `PM_WORKLOG_READINESS_AGING_MIN`
  (default 90 min). The escape hatch: after the aging window we process
  best-effort with whatever is classified, so a genuinely-stuck row (e.g. a crashed
  summariser) can never deadlock the day.

---

## Known limitation: cross-hour attribution of coding segments

A session is bucketed into an hour by its **`started_at`**. Screen sessions are
short ETL blocks, so this is exact for them. A **coding-agent segment**, however,
can span up to an hour (the 1 h time-box), and the whole segment is billed to the
hour it *started* in — so work that physically happened in the next clock hour is
logged under the earlier hour.

> Example: a coding segment running 2:50 → 3:50 is billed entirely to the
> **2–3 pm** hour, even though most of its minutes fall in the 3 pm clock hour.

The **daily total per ticket stays correct** (segments sum without loss or
double-count); only the per-hour distribution can shift by up to one segment.
Accepted by design for now.

---

## The single LLM gate

All LLM work in the daemon flows through the provider-agnostic `crate::llm` layer.
A process-global `Semaphore(1)` acquired per request keeps **exactly one LLM call
in flight** — the categorizer and the worklog generator can never run concurrently,
which also serialises access to the local candle embedder (the one Metal-backed
model still resident, used only for the distiller's semantic dedup). The gate is
per-process; a standalone CLI run has its own gate, so don't run it against a live
daemon.

---

## Safety: nothing posts without human approval

The draft drivers **only ever draft** — they never post. Every worklog lands as a
`drafted` row for a human to review, edit, and approve in the dashboard (Worklogs
view). Approval flips the row to `approved`; the `post` sweep is the **sole** path
that writes to a real tracker. There is no unattended auto-post.

```
drafted ──(UI edit)──▶ drafted ──(UI approve)──▶ approved ──(post sweep)──▶ posted
                                                    │
                          terminal (empty / < 60s)  └──▶ failed
```

A driver re-run can never clobber an `approved`/`posted` row (the UPSERT guard in
`db.rs`), so a human decision is never silently overwritten.

---

## Files

| File | Role |
|---|---|
| `generate.rs` | the per-card "Generate worklog" engine: gated LLM match/propose + draft, and `approve` (create → post_comment → link) |
| `create.rs` | ticket CREATE across providers, for an approved proposal |
| `post_comment.rs` | the plain status-update comment primitive (dispatches per provider) |
| `comment.rs` | worklog-comment formatting helpers |
| `post.rs` | the approved-poster sweep — the sole path to a real tracker |
| `ledger.rs` | hour ledger + readiness predicate + per-hour task discovery |
| `db.rs` | `pm_worklogs` + evidence upserts; approval-guarded upsert; find/mark/fail for idempotency |
| `models.rs` | the worklog / update data types |
| `config.rs` | `PmWorklogConfig::from_env()` |
| `status.rs` | `meridian worklog-status` — human-readable day report |
| `jira.rs` / `linear.rs` / `github.rs` / `trello.rs` / `azure_devops.rs` | per-provider create + post adapters |

The UI approval surface is `ui/components/views/WorklogsView.tsx` + the
`ui/app/api/worklogs/` routes (edit / approve / reject; DB writes only — the UI
never calls a tracker). All synthesis runs in Rust via `crate::llm` — no Python.

---

## CLI

```bash
# Generate + draft the worklog for one day-task (never posts — awaits UI approval)
meridian worklog-generate --day 2026-05-30 --task-id KAN-123

# Approve a generated draft (creates the ticket if proposed, posts on the sweep)
meridian worklog-generate-approve --day 2026-05-30 --task-id KAN-123

# Post everything approved in the dashboard now (same sweep the daemon runs ~60s)
meridian worklog-post-approved

# Human-readable day report (hours done/pending, rows by state)
meridian worklog-status --day 2026-05-30
```

---

## Config (env)

| Env | Default | Purpose |
|---|---|---|
| `PM_WORKLOG_INTERVAL_HOURS` | `1` | Driver pass cadence |
| `PM_WORKLOG_MIN_CONFIDENCE` | `0.65` | Below this → `low_confidence` flag |
| `PM_WORKLOG_MIN_COVERAGE` | `0.80` | Evidence-coverage floor |
| `PM_WORKLOG_READINESS_AGING_MIN` | `90` | Aging escape — max wait for an hour to settle |
| `PM_WORKLOG_MIN_POST_SECONDS` | `60` | Tracker worklog floor |
