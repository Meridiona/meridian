# Meridian — Claude Code Instructions

Meridian is a single-process Rust daemon that normalises raw screen-capture frames into structured, app-based activity sessions stored in its own SQLite database at `~/.meridian/meridian.db`. A Next.js dashboard and a TypeScript MCP server sit alongside the daemon. (Capture source: historically screenpipe's SQLite DB; since the Bucket-2 cutover on `feat/in-process-capture` the frames are produced **in-process by the tray** and the daemon reads `meridian.db`'s own capture tables — see "Capture source — in-process" below.)

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
- NEVER merge a PR automatically — open/update PRs as needed, but leave the actual merge to a human reviewer
- NEVER push directly to `main` — always create a separate feature branch, commit there, and raise a PR to `main`
- ALWAYS use a separate branch per feature/fix — branch name format: `type/short-description` (e.g. `feat/trello-oauth`, `fix/ui-disconnect`)

---

## File Header Requirement

Every `.rs`, `.ts`, and `.tsx` file must start with this comment as its very first line:

```
//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
```

SQL migration files use the SQL comment form:

```
-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
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
  meridian-core/         # lean shared data layer — used by BOTH the daemon and the Tauri dashboard
    src/
      lib.rs             # thin manifest: declares modules + curated `pub use` re-exports (stable public API)
      db.rs              # ActiveSession + open_existing + get_active_session (daemon re-exports these)
      settings.rs        # settings.json runtime config reader (daemon re-exports)
      util/              # DB-free helpers, re-exported flat (meridian_core::{intervals,date,hygiene})
        intervals.rs     # wall-clock interval math (port of ui/lib/intervals.ts)
        date.rs          # local-day bounds (port of ui/lib/date-utils.ts)
        hygiene.rs       # board-hygiene reason → hint/fix mapping
      readers/           # the ported /api/* DB readers, re-exported flat (meridian_core::today, ::tasks, …)
        active.rs  coding_agents.rs  integrations.rs  tasks.rs  triage.rs  week.rs  worklogs.rs
        today/           # mod.rs + types.rs (size split — types co-located per module)
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
  tray/
    src-tauri/           # Tauri shell (Rust + Tauri framework)
      src/
        main.rs          # Tauri entry point
        lib.rs           # thin app bootstrap (builder, db pool, tray install, invoke_handler)
        tray.rs          # tray menu builder + menu-event dispatch + window openers
        sys.rs           # shared helpers: uid_str, notify, ui_base (deduped)
        install.rs       # install-mode detection + meridian_db_path / .env resolution
        state.rs         # app state and health tracking
        format.rs        # duration formatting helpers (with unit tests)
        poll/            # background poll loop
          mod.rs         # loop + tick cadence + tray-sync (emit/tooltip/menu)
          refresh.rs     # refresh_health/active/today/worklogs
          notifications.rs # outbox drain + notifications_allowed
        commands.rs      # commands module root: declares submodules + glob re-exports (commands::<fn>)
        commands/        # the #[tauri::command] surface, grouped by domain
          dashboard.rs   # DB reads (get_active/today/week/coding_agents/worklogs/tasks/triage/settings)
          daemon.rs      # restart/toggle/get_status/get_daemon_status
          system.rs      # open_dashboard/open_worklogs/open_permission_pane
          health.rs  logs.rs  openobserve.rs  integrations.rs  parents.rs  version.rs
      Cargo.toml         # Tauri dependencies
    src/
      index.html         # popover UI template
      app.js             # event listeners, UI rendering
      style.css          # popover styling
    package.json         # npm/Node build config
    create-icons.sh      # icon generation script
```

> **Dashboard → Tauri fold (cutover landed — branch `spike/meridian-core`).** The Next.js dashboard now
> runs **only inside the Tauri webview** as a **static export** (`output: 'export'` → `ui/out`) — **no Node
> server, no `/api` routes**. **DB-backed reads live in `meridian-core`** as the single source of truth (the
> daemon **re-exports** them, its code unchanged; the tray depends on them directly); **file/env/process
> routes are tray commands** (`tray/src-tauri/src/`). Frontend consumers reach Rust **only** via Tauri
> `invoke`/events through `ui/lib/bridge.ts` (`load`/`mutate` → `invoke`; `subscribe` → the event bus —
> the browser `/api` fetch + `EventSource` fallbacks were removed at cutover). The four SSE streams
> (health/notices/notifications/logs) are now **Tauri events** the tray poll loop emits (`tray/src-tauri/src/poll/live.rs`
> + the `log-tail` tailer); the tray poll loop is HTTP-free (direct `meridian-core` reads). Response types
> live in `ui/lib/api-types.ts` (moved out of the deleted routes). **Asset layout:** `frontendDist` →
> `../../ui/out`; the build copies the tray popover into `out/popover/` and the main window loads
> `popover/index.html`; dashboard/setup windows load `WebviewUrl::App("today"/"setup")` → `out/<route>/index.html`
> (`trailingSlash: true`). **Known limitation:** the popover 404s under `tauri dev` (next dev doesn't serve
> `popover/`); it renders in a packaged build. **When adding a route, follow the playbook in Coding
> Conventions → "Porting a dashboard route to Rust"**; exemplars: `meridian-core/src/readers/triage.rs`,
> `tray/src-tauri/src/commands/parents.rs`. The dashboard ships **embedded in the tray binary** (`tauri
> build` → `generate_context!` bundles `ui/out`); the standalone-Node-server release machinery (the
> `com.meridiona.ui` plist, `ui-start.sh`, the `ui.tar.gz` packing, the pinned Node runtime + better-sqlite3
> ABI dance) was retired, and `install-from-bundle.sh` boots out any leftover `com.meridiona.ui` agent on
> update. Dev-only `--features otel` on the tray exports spans to OpenObserve
> (`service.name = meridian-tray`) — release builds omit it. Rationale + full scope: Obsidian
> `Decisions/Dashboard frontend - keep Next in Tauri.md`, `~/.claude/plans/meridian-next-fold.md`.

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

### Tauri tray app (`tray/`)

```bash
cd tray

# Development (hot reload)
npm install
npm run tauri dev

# Production build
bash create-icons.sh
npm install
npm run tauri build  # outputs binary to src-tauri/target/release/meridian-tray

# Rust linting & testing (src-tauri/)
cd src-tauri
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

There are no JS/TS test suites yet. When adding them, place them under `ui/__tests__/`, `packages/meridian-mcp/src/__tests__/`, or `tray/src-tauri/src/__tests__/`.

---

## Environment Variables

| Variable | Default | Purpose |
|---|---|---|
| `SCREENPIPE_DB` | `~/.screenpipe/db.sqlite` | **Vestigial after the Bucket-2 cutover** — the daemon no longer reads screenpipe (capture is in-process → `meridian.db` capture tables). Still parsed into `Config` with a default; slated for removal. |
| `MERIDIAN_DB` | `~/.meridian/meridian.db` | Path to meridian's output SQLite file |
| `POLL_INTERVAL_SECS` | `60` | ETL poll cadence in seconds |
| `RUST_LOG` | `meridian=info` | Tracing filter |
| `SQLX_OFFLINE` | `true` (via `.cargo/config.toml`) | Prevents sqlx from hitting the DB at compile time |
| `MERIDIAN_OTLP_ENDPOINT` | (unset → no export) | OpenObserve OTLP/HTTP traces endpoint (loaded from `.env`) |
| `MERIDIAN_OO_AUTH` | DEPRECATED — ignored by the daemon | OO credentials live in `settings.json` (`oo_email`/`oo_password`, set via dashboard Settings); env var still read by Python services + installer fallback |
| `MLX_SERVER_URL` | (unset → in-process load) | URL of a running MLX classifier server (eval pipeline) |
| `MLX_IDLE_EVICT_S` | `120` (secs) | Idle-eviction window for the MLX model. The model holds ~7 GB of Metal unified memory while resident (invisible to `ps`/Activity Monitor — measure with `mx.get_active_memory()`), so the server lazy-loads it on first request and unloads it after this many seconds idle (~3s cold reload). `0` disables eviction (pins the model). Avoid values below ~30s — a TTL shorter than the gap between sessions in a classification burst causes repeated mid-burst evict+reload thrash. See `services/agents/server.py` `_idle_evictor` + `run_task_linker_mlx.py` `maybe_evict_idle`. |
| `EVAL_DATASET_PATH` | `services/tests/evals/data/generated/goldens_real.json` | Override Goldens file for the eval pipeline |
| `SESSION_TEXT_CAP` | `2500` (chars) | Per-session OCR/a11y excerpt cap in the classifier prompt. Set to `0` to disable truncation for eval experiments (caller is then responsible for not blowing the model's context window — phi-4 = 16k tokens). |

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

### Capture source — in-process (Gap-2 Bucket 2 cutover, branch `feat/in-process-capture`)

> **The daemon no longer reads screenpipe.** Since the slice-4b cutover, capture runs **in-process in the tray** (behind the `capture` feature): the forked `screenpipe-screen` + `screenpipe-a11y` crates produce a11y-tree/OCR frames + input events, which `meridian_core::capture` writes into **`capture_frames` / `capture_ui_events`** in `meridian.db`. The daemon ETL reads *those* tables (via `src/db/screenpipe.rs`, name unchanged for now) from the meridian pool — there is no screenpipe DB/process/pool anymore. **Implication:** a build with the `capture` feature OFF has **no data source** (the daemon produces no sessions); the shipping DMG must enable it. **Audio is dropped** (`get_audio_snippets` stubbed empty) and **gaps all classify `system_sleep`** (no in-process idle detection yet — `capture_trigger` is NULL); both are accepted v1 degradations with idle-detection/audio as future slices. `SCREENPIPE_DB` is now vestigial for the daemon.
>
> Column contract: `capture_frames` mirrors screenpipe's `frames` read-subset (`app_name`/`window_name`/`browser_url`/`timestamp`/`capture_trigger` + `full_text`(OCR)/`accessibility_text`(a11y)/`text_source`, resolved by `COALESCE(full_text, accessibility_text)`); `capture_ui_events` mirrors the `ui_events` read-subset (`event_type`/`app_name`/`text_content`/`timestamp`). **Inverted ownership:** these tables are written by the *tray*, read by the *daemon*.

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

### Porting a dashboard route to Rust (Next-fold playbook)

The fold replaces every `ui/app/api/*` route with a Rust command the frontend calls over Tauri `invoke`. **This is the standard for the work — follow every step when porting a route.** Exemplars to copy: `meridian-core/src/readers/triage.rs` (DB read) and `tray/src-tauri/src/commands/parents.rs` (shell-out).

1. **Place it by data source.** A **DB-backed read** → a new module under `meridian-core/src/readers/`, added to `readers/mod.rs` and re-exported flat from `lib.rs` (`pub use readers::<name>;`) so the public path stays `meridian_core::<name>` (the daemon re-exports it if it needs it too). Reuse `meridian-core::{intervals,date}` — never re-derive time/day math. Anything reading **files / env / a process / external HTTP** (`settings.json`, `.env`, `launchctl`, npm registry, shelling out to `meridian`) → a module under `tray/src-tauri/src/commands/` (grouped by domain), declared + glob-re-exported in `commands.rs`; keep `meridian-core` DB-only.
2. **Match the route byte-for-byte.** Replicate its shaping exactly: defaults, `null → ''` coercions, truncation, ordering, and graceful missing-table/column detection (`sqlite_master` / `pragma_table_info`). Comment any deliberate divergence (e.g. a `BTreeMap` sorts keys vs the route's insertion order — fine when consumers read by key).
3. **Thin command wrapper.** A `#[tauri::command]` resolves request-scoped values (today / now / day) and calls the core fn, so the core stays deterministic and testable. Register it in `lib.rs`'s `invoke_handler!`. Every new window label must be added to `capabilities/default.json` or its `invoke`s are silently denied.
4. **Document it (required).** Module `//!` header with: one-line purpose + which route it ports, a **`# Who calls this`** (the command + the frontend consumer), and a **`# Related`** section linking sibling modules / dependent fns via intra-doc links (`` [`crate::tasks`] ``). Every `pub` fn/struct gets a `///` covering purpose, key params, return, and any non-obvious behaviour carried from the source.
5. **Trace it (required).** `#[tracing::instrument(skip(pool))]` on **both** the command and the core fn; wrap each query in `.instrument(tracing::debug_span!("<module>.read.<table>"))`; `tracing::debug!(rows = …)` after a query; a `tracing::info!(…)` summary on serve; `tracing::warn!(error = %e, …)` on the command's error path. All of it exports to OpenObserve under the tray `otel` feature / the daemon's `observability::init`.
6. **Wire the consumer.** Call the command through `@/lib/bridge`: `load(apiPath, 'command', args)` for a read, `mutate(apiPath, 'command', body, method)` for a write, `subscribe(apiPath, 'command'|null, eventName, onData)` for a live stream. These are Tauri-only now (the `/api` fetch/`EventSource` fallbacks were removed at cutover); `apiPath` is vestigial (documents the former route). Response types go in `ui/lib/api-types.ts`, never a route file. A live stream also needs an emitter in `tray/src-tauri/src/poll/live.rs` (or the log tailer) and the event covered by the window's `core:event` permission.
7. **Test it.** Pure mappers/parsers → `#[cfg(test)]` unit tests in-module (see `hygiene.rs`). DB readers → an in-memory seeded test in `meridian-core/tests/readers.rs` (single-connection `:memory:` pool, hand-computed rows; place date-bounded rows *relative to* `local_day_bounds(today)` so the test is timezone-independent).

### TypeScript / Next.js

- Use `better-sqlite3` (synchronous) in the MCP server — it runs in a single-threaded Node process
- UI API routes live in `ui/app/api/`; keep them thin — query, transform, return JSON
- No `any` types unless unavoidable and justified with a comment
- **Spawning the `meridian` binary from a UI route: ALWAYS use `selectMeridianBinary(meridianCandidates())` from `@/lib/meridian-bin`.** Never spawn a bare `'meridian'` (relies on `$PATH`), and never hand-roll a candidate list. The dashboard runs under **launchd**, whose PATH lacks Homebrew's `node`, so the `#!/usr/bin/env node` wrapper at `~/.local/bin/meridian` dies with `env: node: No such file or directory`. The helper probes the **native binary first** (`~/.meridian/app/bin/meridian`, no runtime deps → works under launchd), so it behaves identically in dev and installed. This bug is invisible in `dev-start` (dev installs a bash wrapper, not a node one) — it only surfaces on bundle/npm installs. `__tests__/meridian-bin.test.ts` guards the ordering. The one sanctioned exception is launching `meridian` in a user Terminal (`open -a Terminal …`, e.g. `api/update`), where an interactive login shell *does* have node/PATH.

### SQL migrations

- Add a new numbered file in `src/migrations/` — never modify an existing migration
- Include the file header comment on line 1
- The integration test helper `make_meridian_db()` runs all migrations; new migrations are covered automatically by `cargo test`

### Observability (logs & traces → OpenObserve)

Any new or changed code path that does real work (daemon stages, the MLX server, the classifier, agents, ingest) **must be observable in OpenObserve** — not just `println!`/`print()` to a terminal. Add proper logs and traces as you write the code, not as an afterthought.

- **Python (`services/`)**: use the module logger created via `observability.setup("<service>")` (`log = logging.getLogger(...)`). `log.info/warning/error` already export to OpenObserve's logs stream, correlated to the active span by `trace_id`/`span_id` — never `print()`. Pass structured fields with `extra={...}` so they're queryable columns, not interpolated into the message string.
- **Rust**: `tracing::info!/warn!/error!/debug!` with **structured fields** — never format data values into the message string (already enforced).
- **Wrap discrete operations in spans** (`tracer.start_as_current_span(...)`) and put the meaningful inputs, outputs, and metrics as **span attributes**, not buried in log lines. For an LLM/model call, capture the EXACT input as sent and output as received (post-cap/post-template — reflect any truncation that actually happened), plus real token counts/latency from the model's own metadata (e.g. MLX `GenerationResponse`) rather than re-deriving them. See `run_task_linker_mlx.py`'s `classify_session → classifier_input / llm_inference / classifier_output` span tree for the reference shape.
- **No duplication, no truncation of debug data**: emit each fact once, on the span that owns it; don't truncate the values you'd actually need to debug a misclassification. Keep static/identical-every-call blobs (e.g. the full system prompt) out of every trace where a size + a single archived copy suffices.
- **Set span status `ERROR`** (with a message) on failures, and log a `warning`/`error` with `.context`/`extra` at the failure boundary.
- **Export is gated** by the OpenObserve Export toggle (`otlp_enabled` in `settings.json`); code must degrade silently (logs still go to file/stderr) when it's off — never crash because export is disabled.

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
4. If the route shells out to the `meridian` binary, resolve it with `selectMeridianBinary(meridianCandidates())` from `@/lib/meridian-bin` — never a bare `'meridian'` or an ad-hoc candidate list (see the launchd/node-wrapper note under Coding Conventions → TypeScript / Next.js)

### Add a new MCP tool

1. Read `packages/meridian-mcp/dist/index.js` to understand existing tool structure
2. Edit the TypeScript source in `packages/meridian-mcp/src/`
3. Run `npm run build` in `packages/meridian-mcp/` and verify `dist/index.js` is updated

### Add a Golden to the classifier eval dataset

Goldens are hand-authored seed sessions that target specific failure modes of the MLX classifier. The eval pipeline scores the classifier against them on every model swap, prompt edit, or temperature change.

1. Open `services/tests/evals/data/seeds/sessions_<persona>.json` — `sessions_a_meridian.json` for the Meridian dev persona, `sessions_b_generic.json` for the generic SaaS dev persona.
2. Append a new session object inside the `sessions` array. Required fields: `id` (next int), `app_name`, `started_at`, `ended_at`, `duration_s`, `category`, `confidence`, `session_text_source`, `window_titles`, `session_text` (the realistic OCR/a11y capture the classifier will see), `audio_snippets`, and a `ground_truth` block with `task_key`, `session_type`, `reasoning`, `difficulty` (`easy`/`medium`/`hard`/`hard-decoy`/`overhead`/`untracked`/`context-only`), `scoreable` (bool — `false` = timeline density only, excluded from Goldens and the recent-context block).
3. Add a `design_notes` field explaining the specific failure mode this case targets — required for future maintainers debugging regression diffs.
4. Re-render the Goldens: `services/.venv/bin/python services/tests/evals/render_seeds.py <persona>`
5. Re-run the eval (see `TESTING.md` §9): the new Golden appears in the OpenObserve trace tree as one more `eval.classify` child span.

The dataset's value lives in **what it discriminates**, not how many cases it has. Each Golden should target a documented failure mode (keyword-mention false positive, same-app context switch, decoy resistance, untracked-with-tempting-candidate, etc.). 95% on easy cases hides the failures that matter.

---

## Coding-agent pipeline (`src/coding_agent_session_ingest/`)

The coding-agent indexer + summariser run **inside the Rust daemon** (`src/coding_agent_session_ingest/`), spawned as gated tokio tasks from `main.rs`. They turn coding-agent conversations into segmented `app_sessions` rows, summarise sealed segments **with each agent's own CLI** (MLX as the shared fallback), and write the summary for the agno worklog pipeline to pick up. Lifecycle is the `task_method` column: `coding_agent_live → pending_summariser → summarised`.

### Ingested agents

| Agent | Store | Adapter | `app_name` / `session_text_source` |
|---|---|---|---|
| Claude Code | `~/.claude/projects/**/<uuid>.jsonl` | `jsonl.rs` (legacy path) | `Claude Code` / `claude_jsonl` |
| Codex | `~/.codex/sessions/**/rollout-*.jsonl` | `jsonl.rs` (legacy path) | `Codex` / `codex_jsonl` |
| GitHub Copilot CLI | `~/.copilot/session-state/<uuid>/events.jsonl` | `sources/copilot_cli.rs` | `GitHub Copilot` / `copilot_events_jsonl` |
| Copilot VS Code chat | `…/Code/User/**/chatSessions/*.jsonl` (op-log: kind 0 snapshot / 1 set / 2 append) | `sources/copilot_vscode.rs` | `GitHub Copilot` / `copilot_chat_jsonl` |
| Cursor (sidebar + IDE agent) | `state.vscdb` → `cursorDiskKV` (`composerData:` + `bubbleId:`) | `sources/cursor.rs` | `Cursor Agent` / `cursor_vscdb` |
| cursor-agent CLI | `~/.cursor/chats/<ws>/<uuid>/store.db` (content-addressed blobs) | `sources/cursor_cli.rs` | `Cursor Agent` / `cursor_cli_store` |
| Antigravity | detection-only stub (store format unpinned) | `sources/antigravity.rs` | — (logs presence, ingests nothing) |

New sources plug into the `AgentSource` enum in `sources/mod.rs` and are swept by the same indexer tick; everything downstream (segmentation, sealing, summarising, classifying) is agent-blind `NormRecord`s.

- **Indexer** (`indexer.rs`): per tick (`INDEXER_POLL_INTERVAL_S`, 600 s) seals settled rows, re-parses changed stores, sweeps the source adapters. Backfill is today-only. `meridian coding-agent-hook` is the Claude SessionEnd entry (seals one session immediately).
- **Session completion**: Claude seals via hook; CLI agents (codex / copilot / cursor-agent) seal promptly on **Ctrl+C / exit** (Copilot's `session.shutdown` marker force-seals at registration; otherwise a per-tick `ps -axo args=` probe seals every live row of a CLI whose process is gone) and on **/clear · /new** (a newer session of the same source supersedes older live rows). IDE chats and crashes fall back to the idle seal (`INDEXER_SEAL_IDLE_S`, 1 h). All acceleration paths only hasten what the idle backstop would do — a wrong call costs a segment split, never data.
- **Summariser** (`summariser/`): routes each row to its own agent CLI — `claude.rs` / `codex.rs` / `copilot.rs` / `cursor_agent.rs` (2 attempts) → `mlx.rs` fallback (`/summarise`); writes `session_summary` + `summary_source`, flips `task_method` to `summarised`. cursor-agent is auth-probed lazily on first use, and auto-installed only behind the `CURSOR_AGENT_AUTO_INSTALL=1` opt-in (`cursor_agent_init.rs`). CLI: `meridian coding-agent-summarise`. See `summariser/README.md`.
- **Self-ingest guard**: copilot/cursor-agent persist their own summary runs into stores we ingest; `sources::sweep()` drops any conversation whose first user prompt carries `SUMMARY_PROMPT_MARKER` (log: `skipping summariser-artifact session`). This is the loop cut — do not remove it.
- **Worklog trigger**: `summarised` rows are picked up by the agno worklog pipeline (`worklog_pipeline/`) via `session_summary IS NOT NULL` — folded verbatim into the hour's activity summary alongside the distilled OCR sessions, then matched to tasks and drafted.

Source-adapter env overrides: `COPILOT_SESSION_STATE_DIR`, `VSCODE_USER_DIR`, `CURSOR_STATE_VSCDB`, `CURSOR_CLI_CHATS_DIR`, `ANTIGRAVITY_APP_DIR`.

> **Daemon config gotcha:** the daemon loads env via `dotenvy::dotenv_override()`, which walks UP from its launchd `WorkingDirectory` and stops at the first `.env`. All install types converge on the **canonical `~/.meridian/.env`** (the same file the tray writes tracker creds to): the **npm bundle**'s `WorkingDirectory` is `~/.meridian/app` but no `~/.meridian/app/.env` is written (the installer creates `~/.meridian/.env`), so dotenvy walks up to it; the **`.app` DMG** (the tray stages the daemon via `tray/src-tauri/src/backend_install.rs`) sets `WorkingDirectory` to `~/.meridian` and reads it directly; **source/dev** reads the repo `.env`. Edit `~/.meridian/.env` (then `meridian restart`) to tune daemon env on an installed system.

The pipeline is fully ported to Rust; the former Python `coding_agent_indexer` + `coding_agent_summariser` packages have been removed. The MLX server (`agents/server.py`) is the only remaining Python hop (it serves `/summarise` + `/classify_sessions`).

## Python agent service (`services/`)

These Python services still run alongside the Rust daemon:

1. **MLX server** (`agents/server.py`) — the persistent FastAPI model server (`com.meridiona.mlx-server.plist`). Exposes `/summarise`, `/activity_report`, `/distill_hour`, `/rerank`, `/worklog_hour`, and OpenAI-compatible `/v1/chat/completions`. The one Python piece the pipeline can't replace (outlines + mlx-lm are Python-only).
2. **Jira updater** (`agents/pm_worklog_update/`) — agno-powered synthesis workflow that generates Jira comments + worklogs from classified sessions. Runs on an office-hours slot schedule.

For the deep technical reference (classification logic, scoring formulas, recipes for tuning prompts / debugging misclassifications), see `services/agents/README.md`.

### Hard rules

- **Every `.py` file in `services/agents/` must start with a `"""…"""` module docstring** describing its purpose. The Rust/TS file-header convention does not apply — Python uses docstrings. Match the prose style of existing modules (terse, opinionated).
- **`ticket_links` and `session_dimensions` writes must be idempotent.** Both tables have UNIQUE / composite-PK constraints with explicit `ON CONFLICT … DO UPDATE` policies. New writers must use the same UPSERT pattern. Never `DELETE` then `INSERT` from the daemon path.
- **Coding-agent segment idempotency:** the `(claude_session_uuid, segment_started_at)` unique index is the key (migration 027; `day_utc` was dropped in 028). The UPSERT refreshes a LIVE row but carries `WHERE sealed_at IS NULL`, so a SEALED row is immutable — the summariser/classifier only ever read sealed rows.
- **Eval-only strategies live in `services/tests/evals/strategies.py`, NOT in `services/agents/`.** `services/agents/` is for production code (the running daemon, the MLX server, `run_task_linker_mlx.py`). The `EvalStrategy` abstraction + `DirectHttpStrategy` + future `ExtractThenClassifyStrategy` / retrieval-augmented / agentic variants belong with the eval harness. A strategy that proves out in eval is **promoted** into `services/agents/` as a deliberate, separate productionization step — it is NOT silently shared. Adding experimental strategies to `services/agents/` pollutes the production surface with code the tagger never executes. See `services/tests/evals/README.md` § "Architecture convention" for the rationale.

### Quick command reference

```bash
# coding-agent ingest — runs inside the daemon; these are the one-shot CLIs
echo '{"transcript_path":"~/.claude/projects/.../<uuid>.jsonl"}' | meridian coding-agent-hook  # SessionEnd: seal one session
meridian coding-agent-summarise [--dry-run] [--day YYYY-MM-DD] [--limit N]                     # summarise the pending queue

# Classifier eval pipeline (see TESTING.md §9, services/tests/evals/README.md)
services/.venv/bin/python services/tests/evals/render_seeds.py            # seeds → Goldens
EVAL_DATASET_PATH=services/tests/evals/data/generated/goldens_a_meridian.json \
services/.venv/bin/python services/tests/evals/eval_classifier.py         # run, emits traces to OpenObserve
```

---

## Git Hygiene

- Commit message style: `type(scope): short description` — e.g. `fix(etl): detect sleep gaps that span ETL run boundaries`
- `commit-msg` hook validates conventional commits format — fix message before retrying
- `pre-commit` hook runs `cargo fmt --check` and `cargo clippy -- -D warnings`
- `pre-push` hook runs the full suite: `cargo fmt` + `cargo clippy` + UI build + UI tests + security audit (claude CLI) + `cargo test`
- Never skip hooks with `--no-verify`
- Install hooks after cloning: `bash scripts/setup-hooks.sh`
- Never amend a commit that has already been pushed to `main`
