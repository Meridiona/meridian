# Observability — tracing the DB → UI flow in OpenObserve

How to read the logs, traces, and spans Meridian emits into OpenObserve (OO),
and the instrumentation convention behind them.

The dashboard read paths (`/api/active`, `/api/today`, `/api/week`,
`/api/tasks`, `/api/worklogs`, `/api/coding-agents`, `/api/integrations`) are
ported into `meridian-core` and run **inside the tray**. Each one is
instrumented so a single page render produces a complete, legible trace + log
flow from the DB up to the UI.

---

## 1. Quick start — get data flowing

The tray's OTel export is **dev-only**, behind the `otel` cargo feature:

```bash
cd tray
RUST_LOG=meridian=debug cargo tauri dev --features otel
```

- `--features otel` pulls in the daemon's OTLP exporter and tags spans/logs
  with `service.name = meridian-tray`. Without it, the tray ships lean (no OTel
  compiled in at all).
- `RUST_LOG=meridian=debug` is **required to see the per-operation flow**. At the
  default `INFO` you only get the per-page heartbeat (`… computed`) and the
  `get_*` request spans — the granular `*.read.*` children/events are DEBUG by
  design (see [§5 Levels](#5-the-level-convention)).
- OO credentials + endpoint come from `settings.json` (`oo_email`,
  `oo_password`, `otlp_enabled`, `otlp_endpoint`), set in the dashboard
  **Settings** page. The `MERIDIAN_OO_AUTH` env var is deprecated and ignored.

Default OTLP endpoint (override via `MERIDIAN_OTLP_ENDPOINT` or settings):
`http://localhost:5080/api/default/v1/traces` (logs are derived from it as
`…/v1/logs`).

---

## 2. Where the data lands

Two separate streams, filtered by one source name:

| | |
|---|---|
| `service.name = meridian-tray` | Everything the dashboard read paths emit (they run inside the tray). |
| `service.name = meridian-rust` | The always-on daemon (ETL, classifier, etc.) — a separate surface. |
| **Traces** stream | The spans — the `command → core fn → per-op` tree. |
| **Logs** stream | The `info!`/`debug!`/`warn!` events, each stamped with the `trace_id` + `span_id` of the span it fired inside. |

Start every search with `service_name='meridian-tray'`.

---

## 3. Navigate the TRACES (the tree)

OO → **Traces** → time range "last 15 min" → `service_name='meridian-tray'`.

Each row is one page render. Click `get_today` → the **waterfall**:

```
get_today                          (command — request root span)
 └─ get_today                      (meridian-core fn)
     ├─ today.read.columns         2.1ms
     ├─ today.read.app_sessions    8.4ms   ← widest bar = the cost
     ├─ today.read.active_session  0.6ms
     ├─ today.read.gaps            0.5ms
     └─ today.read.pm_tasks        0.7ms
```

- Click a span → **span detail**: duration, `code.filepath` / `code.lineno`, and
  the `debug!` events that fired inside it (the `rows=…` / `found=…` lines show
  here as span events).
- This is the **"where does the time go"** view — the widest bar is the slow
  query.

---

## 4. Navigate the LOGS (the narrative)

OO → **Logs** → the logs stream → `service_name='meridian-tray'`.

At DEBUG, sorted ascending by time, a Today render reads as:

```
today.read.columns         columns=18
today.read.app_sessions    rows=56
today.read.active_session  found=true
today.read.gaps            rows=3
today.read.pm_tasks        rows=12
today computed             focus_s=9228 session_count=14
```

The fields (`rows`, `found`, `columns`, `focus_s`, …) are **searchable columns**
— e.g. `rows > 100`, `found=false`, `level='WARN'`.

---

## 5. Pivot trace ↔ logs

The payoff of the `trace_id` correlation:

- **Trace → logs:** copy the `trace_id` from a span detail → **Logs** →
  `trace_id='<paste>'` → exactly the log lines for that one page render, in order.
- **Log → trace:** every log line carries `trace_id` + `span_id` → paste the
  `trace_id` into **Traces** to jump to the waterfall.

---

## 6. The instrumentation convention

Applied uniformly across every `meridian-core` read path:

```
command (#[tracing::instrument])              ← request root span (INFO) + warn! on error
 └─ core fn (#[tracing::instrument(skip pool)]) ← INFO span
     ├─ <mod>.read.<thing>  (debug_span!)     ← one per DB op: timing in the trace tree
     │   + debug!(rows/found = …)             ← the readable flow in the logs
     └─ "<mod> computed"    (info!)           ← one heartbeat per page render
```

### The level convention

- **INFO** — a light per-render heartbeat: the `get_*` request spans + one
  `… computed` summary line per page. This is what you see at the default level.
- **DEBUG** — the *complete* flow: every `*.read.*` child span (timed) and its
  `rows=…` / `found=…` event. Crank to `meridian=debug` to see it.
- **WARN** — surfaced failures, including reads that gracefully fall back to
  empty (`"today: gaps read failed…"`) and command failures (`"get_today failed"`).

`active` is the exception: it is **all-DEBUG** (no INFO heartbeat) because the
popover polls it — an INFO line per poll would be noise.

### Not instrumented (intentionally)

`intervals`, `date`, `hygiene` are pure functions (no I/O) — no spans. `settings`
is a file read and keeps only its existing `warn!` on parse failure.

---

## 7. Name cheat-sheet

**Per-op span / event names** (`<mod>.read.<thing>`):

| Module | Read spans |
|---|---|
| `active` | `active.read.active_session` |
| `today` | `today.read.{columns, app_sessions, active_session, gaps, pm_tasks}` |
| `week` | `week.read.{day, active}` (`day` carries a `day=YYYY-MM-DD` field) |
| `tasks` | `tasks.read.{pm_tasks, presence, sessions, unassigned, curation_exists, ignored_col, pm_task_curation}` (`presence`/`sessions` carry `scope=today\|week`) |
| `worklogs` | `worklogs.read.pm_worklogs` |
| `coding_agents` | `coding_agents.read.app_sessions` |
| `integrations` | `integrations.read.pm_sync_state` |

**INFO heartbeats** (visible without DEBUG): `"today computed"`,
`"week computed"`, `"tasks computed"`, `"coding-agents computed"`,
`"worklogs computed"`, `"integrations sync-errors served"`,
`"active served"` (DEBUG).

---

## 8. Recipes

- **"Why was the dashboard empty?"** → Logs: `service_name='meridian-tray' AND level='WARN'`.
  A silently-empty read leaves a `… read failed` warn with the DB error.
- **"Which query is slow on the Tasks page?"** → Traces: open a `get_tasks`
  trace, scan the waterfall — `tasks.read.presence{scope=week}` vs
  `tasks.read.pm_tasks` bar widths tell you instantly.
- **"Trace one page render end-to-end"** → open the trace, copy its `trace_id`,
  search Logs by it → the ordered flow with row counts.

---

## 9. Reference

| Setting | Value |
|---|---|
| Tray service name | `meridian-tray` (dev, `--features otel`) |
| Daemon service name | `meridian-rust` (always on) |
| OTel feature | `otel` cargo feature on `meridian-tray` (off in release) |
| Level to see the full flow | `RUST_LOG=meridian=debug` (or settings `log_level: DEBUG`) |
| Default OTLP endpoint | `http://localhost:5080/api/default/v1/traces` |
| Credentials / endpoint source | `settings.json` (`oo_email`, `oo_password`, `otlp_enabled`, `otlp_endpoint`) |

> Exact OO menu labels vary by version; the stream names and query fields above
> are what matter.
