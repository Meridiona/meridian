# Meridian — Claude Code Instructions

Meridian is a single-process Rust daemon that reads screenpipe's SQLite database and normalises raw screen-capture frames into structured, app-based activity sessions stored in its own SQLite database at `~/.meridian/meridian.db`. A Next.js dashboard and a TypeScript MCP server sit alongside the daemon.

---

## Hard Rules

- Do what has been asked; nothing more, nothing less
- NEVER create files unless absolutely necessary — prefer editing existing files
- NEVER create documentation files unless explicitly requested
- ALWAYS read a file before editing it
- NEVER commit secrets, credentials, or `.env` files
- Keep files under 500 lines; split when a file grows beyond that
- Validate all input at system boundaries (config load, DB open, frame parsing)
- NEVER run `git reset`, `git push --force`, or delete local code — other agents may be working on the codebase in parallel

---

## File Header Requirement

Every `.rs`, `.ts`, and `.tsx` file must start with this comment as its very first line:

```
// meridian — normalises screenpipe activity into structured app sessions
```

SQL migration files use the SQL comment form:

```
-- meridian — normalises screenpipe activity into structured app sessions
```

The `commit-msg` hook enforces conventional commit format. The `pre-commit` hook enforces `cargo fmt` and `cargo clippy`. The `pre-push` hook runs the full suite: fmt + clippy + `cargo test` + UI build + UI tests.

---

## Repository Layout

```
meridian/
  src/
    main.rs              # daemon entry point — tokio::main, signal handling, poll loop
    lib.rs               # public crate root
    config.rs            # Config::from_env() — reads env vars, expands ~
    db/
      mod.rs
      meridian.rs        # writes app_sessions, active_session, etl_runs, etl_cursor, gaps
      screenpipe.rs      # read-only queries against screenpipe's frames/ocr/audio/ui_events
    etl/
      mod.rs
      runner.rs          # run_etl() — batch loop, gap detection, block state machine
      extractor.rs       # extract_block_context() — OCR, audio, signals, window titles
    migrations/
      001_initial.sql    # app_sessions, active_session, etl_runs, etl_cursor
      002_gaps.sql       # gaps table, idle_frame_count columns
  tests/
    integration_etl.rs   # integration tests — in-memory SQLite, no network
  ui/
    app/
      layout.tsx         # root layout
      page.tsx           # dashboard home
      sessions/          # session list and detail pages
      apps/              # per-app breakdown pages
      api/               # Next.js route handlers (active, sessions, stats, timeline)
    components/          # ActiveSessionCard, AppTable, DayTimeline, FocusDonut, Nav, …
  packages/
    meridian-mcp/        # TypeScript MCP server — exposes meridian.db to AI clients
      dist/index.js      # compiled output (committed)
```

---

## Build, Test, Lint

### Rust daemon

```bash
# Build (SQLX_OFFLINE=true is set automatically via .cargo/config.toml)
cargo build --release

# Run all tests
cargo test

# Lint (must pass before committing)
cargo clippy -- -D warnings

# Format (must pass before committing)
cargo fmt
```

Rust toolchain is pinned to **1.93.1** via `rust-toolchain.toml`.

### Next.js dashboard (`ui/`)

```bash
cd ui
npm install
npm run dev    # development server
npm run build  # production build
```

### MCP server (`packages/meridian-mcp/`)

```bash
cd packages/meridian-mcp
npm install
npm run build  # compiles TypeScript → dist/index.js
```

There are no JS/TS test suites yet. When adding them, place them under `ui/__tests__/` or `packages/meridian-mcp/src/__tests__/`.

---

## Environment Variables

| Variable | Default | Purpose |
|---|---|---|
| `SCREENPIPE_DB` | `~/.screenpipe/db.sqlite` | Path to screenpipe's SQLite file (read-only) |
| `MERIDIAN_DB` | `~/.meridian/meridian.db` | Path to meridian's output SQLite file |
| `POLL_INTERVAL_SECS` | `60` | ETL poll cadence in seconds |
| `RUST_LOG` | `meridian=info` | Tracing filter |
| `SQLX_OFFLINE` | `true` (via `.cargo/config.toml`) | Prevents sqlx from hitting the DB at compile time |

Tilde expansion is handled by `Config::from_env()`. Never hardcode paths.

---

## Architecture

### Single-process Rust daemon

- No network, no auth, no HTTP server, local-only SQLite
- Two connection pools: `screenpipe` (read-only WAL), `meridian` (read-write WAL)
- On startup: `cleanup_incomplete_runs` removes partial sessions left by a previous crash, then runs the first ETL pass immediately before entering the poll loop
- Poll loop: `tokio::select!` over `SIGINT`/`SIGTERM` and a sleep timer; on each tick calls `run_etl()`
- Graceful shutdown closes both pools

### ETL pipeline (`src/etl/runner.rs`)

`run_etl()` is the single entry point called every poll interval:

1. Read cursor (`etl_cursor`, last processed `frame_id`)
2. Insert an ETL run row with `status = 'running'`
3. **Cross-run gap check**: if `active_session` exists from a previous run and the first new frame is >300 s later, classify and record the gap, then close the stale session
4. Process frames in batches of 500 (`BATCH_SIZE`), maintaining a block state machine keyed on `app_name`
5. **Intra-batch gap check**: before every frame, if the inter-frame gap exceeds `GAP_THRESHOLD_SECS` (300 s), close the current block at its real `ended_at`, record the gap, then start fresh
6. **App-switch close** (`close_block`): when `app_name` changes, close the old block into `app_sessions`; apply Option C (ui_event refines `ended_at`) and Option D (single-frame sessions use `next_frame_ts`)
7. **Active session upsert** (`upsert_open_block`): the still-open block at end of all batches goes into `active_session` (single-row table, `id = 1`)
8. Advance cursor, mark ETL run `success` (or `failed` with error text on error)

### Gap classification

`count_frames_in_window(screenpipe, from, to)` counts all frames inside the gap window, including frames with NULL `app_name`. If `idle_count * 2 >= total_count` → `user_idle`, else → `system_sleep`. A gap of exactly 300 s does not trigger (threshold is strictly greater than).

### DB schema (`meridian.db`)

| Table | Role |
|---|---|
| `app_sessions` | Completed sessions — append-only, never updated after insert |
| `active_session` | Single-row in-progress block, upserted every poll |
| `etl_runs` | Audit log — one row per `run_etl()` call |
| `etl_cursor` | Single-row cursor tracking `last_frame_id` |
| `gaps` | Sleep/idle periods — `user_idle` or `system_sleep` |

JSON columns (`window_titles`, `ocr_samples`, `elements_samples`, `audio_snippets`, `signals`) store structured sub-documents. `ocr_samples` and `elements_samples` are capped at 20 entries per session via `OCR_SAMPLE_CAP`. Audio snippets are uncapped. Window title counts are merged and re-sorted descending on each upsert.

### Screenpipe schema (read-only)

Key tables: `frames`, `ocr_text`, `elements`, `audio_transcriptions`, `ui_events`. Never write to this database. `screenpipe.rs` is the only file that should query these tables.

---

## Before Making Changes

### ETL logic, DB schema, or migrations

Read `TESTING.md` first. Integration tests live in `tests/integration_etl.rs` and use in-memory SQLite — they must continue to pass after any ETL or schema change. Run `cargo test` before committing.

Key invariants the tests enforce:

- A block with no app switch stays in `active_session`, never in `app_sessions`
- An app switch closes the old block into `app_sessions` with correct `frame_count` and `duration_s`
- `duration_s` never includes gap time — the pre-gap block closes at the last real frame timestamp
- Option C applies only when the `ui_event` timestamp is strictly after the last frame timestamp
- A gap of exactly 299 s must not produce a gap row (threshold is strictly greater than 300)
- `cleanup_incomplete_runs` deletes partial sessions and marks the run `aborted`
- `idle_frame_count` reflects screenpipe `capture_trigger = 'idle'` frames only

### Product decisions

Read `VISION.md` first.

---

## Coding Conventions

### Rust

- Error handling: `anyhow::Result` throughout; add `.context("…")` to every `?` in DB calls
- Logging: `tracing::info!/warn!/error!/debug!` with structured fields — no format strings for data values
- Clippy: all warnings are errors (`-D warnings` enforced in `.cargo/config.toml` and CI)
- Argument limit: clippy's 7-argument limit applies; group related params into a struct (see `BlockBounds` in `runner.rs`)
- Avoid `unwrap()` outside tests; use `?` or explicit error handling
- ETL state machine lives in `runner.rs` — add sub-step helpers inside that module rather than new top-level modules

### TypeScript / Next.js

- Use `better-sqlite3` (synchronous) in the MCP server — it runs in a single-threaded Node process
- UI API routes live in `ui/app/api/`; keep them thin — query, transform, return JSON
- No `any` types unless unavoidable and justified with a comment

### SQL migrations

- Add a new numbered file in `src/migrations/` — never modify an existing migration
- Include the file header comment on line 1
- The integration test helper `make_meridian_db()` runs all migrations; new migrations are covered automatically by `cargo test`

---

## Common Tasks

### Add a new DB query

1. Read `src/db/meridian.rs` or `src/db/screenpipe.rs`
2. Follow the `sqlx::query_as` + `.context("description")` pattern
3. Export from `src/db/mod.rs` if needed
4. Run `cargo clippy -- -D warnings && cargo test`

### Add a new ETL extraction signal

1. Read `src/etl/extractor.rs` and `src/db/screenpipe.rs`
2. Add the screenpipe read query in `screenpipe.rs`
3. Extend `BlockContext` in `extractor.rs` and wire it in `extract_block_context()`
4. Update `build_active_session` and `merge_into_active` in `runner.rs`
5. Add a migration if the signal needs its own column; otherwise store as JSON in `signals`
6. Add an integration test in `tests/integration_etl.rs`

### Add a new UI API route

1. Create `ui/app/api/<name>/route.ts`
2. Query `meridian.db` using `better-sqlite3` (see existing routes for the pattern)
3. Return a typed JSON response; define the response type inline

### Add a new MCP tool

1. Read `packages/meridian-mcp/dist/index.js` to understand existing tool structure
2. Edit the TypeScript source in `packages/meridian-mcp/src/`
3. Run `npm run build` in `packages/meridian-mcp/` and verify `dist/index.js` is updated

---

## Python agent service (`services/`)

Two independent Python services run alongside the Rust daemon:

### 1. Task Classifier (`run_task_linker.py`)

The Rust intelligence module spawns `run_task_linker.py` as a one-shot subprocess to classify `app_sessions` rows into Jira task links. The pipeline:

1. Reads JSON from stdin: `{sessions: [...], pm_tasks: [...], traceparent: ...}`
2. For each session:
   - Skip if `duration_s < MIN_LLM_DURATION_S` (default 30 s) → `routing=skip` (no LLM call)
   - Otherwise call `classify_session()` which:
     - Calls `select_model_for_hermes()` for **local-first LLM selection** (Ollama, LM Studio, llama.cpp, or managed `mlx_lm.server`)
     - Falls back to cloud LLM if no local endpoint is found
     - Uses **hermes AIAgent** with the `task-classifier` skill (system prompt from `skills/activity/task-classifier/SKILL.md`)
     - Returns `{task_key, confidence, routing}` where `routing` is one of: `auto` (confidence ≥ `AGENT_AUTO_FLOOR`), `queue` (confidence ≥ `AGENT_QUEUE_FLOOR`), `skip` (below thresholds)
3. Writes JSON to stdout and exits; Rust reads it and writes `ticket_links` table

The classifier runs on **every processed session** via the Rust intelligence module. Configuration: `MERIDIAN_DB`, `MIN_LLM_DURATION_S`, `LLM_PREFER_LOCAL`, `LLM_BUDGET_PCT`, `AGENT_AUTO_FLOOR`, `AGENT_QUEUE_FLOOR`.

For technical detail (module layout, response parser, LLM selection), see `services/agents/README.md#pipeline`.

### 2. Jira Updater Daemon (`jira_updater_daemon.py`)

Optional long-running daemon that periodically fetches in-progress Jira tasks, queries Meridian MCP for recent session data on each task, generates a bullet-point summary via hermes, and posts as timed comments to Jira. Fires on office-hour slots (default 1 PM and 5 PM, looking back 4 hours).

Setup requires `JIRA_BASE_URL`, `JIRA_EMAIL`, `JIRA_API_TOKEN` in `.env`. Runs as a launchd daemon or via `python -m agents.jira_updater_daemon`. See `services/README.md#jira-updater` for configuration and commands.

### Hard rules

- **Every `.py` file in `services/agents/` must start with a `"""…"""` module docstring** describing its purpose. The Rust/TS file-header convention does not apply — Python uses docstrings. Match the prose style of existing modules (terse, opinionated).
- **`agent_cursor.last_session_id` only advances; never decreases.** Cursor advances after EVERY session in the batch, regardless of routing outcome. A SIGTERM mid-batch loses at most the in-flight session.
- **`ticket_links` writes must be idempotent.** The table has a UNIQUE constraint on `session_id` with an `ON CONFLICT … DO UPDATE` policy. Never `DELETE` then `INSERT`.

### Quick command reference

```bash
# Verify which LLM will be selected
cd services
.venv/bin/python -c "
from agents.llm_selector import discover_running_servers, select_model_for_hermes
for s in discover_running_servers():
    print(f'running: {s.runtime}  loaded={s.models}')
ep = select_model_for_hermes()
print(f'selected: {ep.model}  runtime={ep.runtime}' if ep else 'cloud fallback')
"

# Test task classifier on a single session
python -m agents.task_classifier_agent --session <ID>

# Jira updater — one-shot or daemon
python -m agents.jira_updater_daemon --trigger-now
python -m agents.jira_updater_daemon --task KAN-87
python -m agents.jira_updater_daemon  # long-running daemon

# launchd lifecycle (Jira updater only)
./scripts/install-jira-updater-daemon.sh
./scripts/uninstall-jira-updater-daemon.sh
tail -f ~/.meridian/logs/jira-updater.log
```

---

## Git Hygiene

- Commit message style: `type(scope): short description` — e.g. `fix(etl): detect sleep gaps that span ETL run boundaries`
- `commit-msg` hook validates conventional commits format — fix message before retrying
- `pre-commit` hook runs `cargo fmt --check` and `cargo clippy -- -D warnings`
- `pre-push` hook runs the full suite: `cargo fmt` + `cargo clippy` + `cargo test` + `cd ui && npm run build` + `cd ui && bun test`
- Never skip hooks with `--no-verify`
- Install hooks after cloning: `bash scripts/setup-hooks.sh`
- Never amend a commit that has already been pushed to `main`
