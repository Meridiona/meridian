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

The pre-commit hook enforces `cargo fmt` and `cargo clippy -- -D warnings`. CI runs the same checks plus `cargo test` and `cargo build --release`.

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

## Git Hygiene

- Commit message style: `type(scope): short description` — e.g. `fix(etl): detect sleep gaps that span ETL run boundaries`
- Pre-commit hook runs `cargo fmt --check` and `cargo clippy -- -D warnings` — fix failures before committing, never skip hooks with `--no-verify`
- Install hooks after cloning: `bash scripts/setup-hooks.sh`
- Never amend a commit that has already been pushed to `main`
