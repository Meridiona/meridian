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
- NEVER push directly to `main` or `pre-main` — always create a separate feature branch, commit there, and raise a PR to `pre-main`. **All features, fixes, and other changes target `pre-main`** (the staging branch), not `main` — only a maintainer opens the `pre-main → main` release PR, and only after everything on `pre-main` has been tested end-to-end on staging
- ALWAYS use a separate branch per feature/fix — branch name format: `type/short-description` (e.g. `feat/trello-oauth`, `fix/ui-disconnect`)
- In all **user-facing app text** — window titles, wizard/UI copy, button and menu labels, notification bodies, tray tooltips, any string the user reads — use a plain hyphen `-` only. NEVER an em-dash (`—`), en-dash (`–`), or double hyphen (`--`). Use it spaced (` - `) where a dash separates clauses. (This rule is about displayed strings; code comments and docs are exempt.)

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
    notifications.rs     # notification outbox producer API — enqueue/retract (see NOTIFICATIONS.md)
    notification_responses.rs # consumer for interactive-toast answers (snooze, …)
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
> ABI dance) was retired, and `backend_install.rs` boots out any leftover `com.meridiona.ui` agent on
> update. The tray calls `meridian::observability::init("meridian-tray")`
> **unconditionally** in every build — the old dev-only `otel` feature was
> removed, because the tray is the process the user actually clicks and it was
> otherwise dark in release. Rationale + full scope: Obsidian
> `Decisions/Dashboard frontend - keep Next in Tauri.md`, `~/.claude/plans/meridian-next-fold.md`.

### Per-OS Tauri config — `tauri.windows.conf.json` (read before editing `bundle.resources`)

Tauri auto-merges `tauri.<platform>.conf.json` into `tauri.conf.json` when
building for that platform (no flag; detected from the target). Meridian uses
`tray/src-tauri/tauri.windows.conf.json` for everything Windows-specific —
`bundle.targets`, the daemon resource, NSIS options — so **`tauri.conf.json`
stays the macOS source of truth and is never edited for Windows**.

**The gotcha:** the merge is JSON Merge Patch (RFC 7396), which merges objects
**key by key** rather than replacing them. `bundle.resources` is a map, so the
macOS entries (`target/release/meridian`, `com.meridiona.daemon.plist`)
would otherwise survive into the Windows bundle and fail the build — the file
names differ (`.exe`) and a plist is meaningless there. They are explicitly set
to `null`, which is how RFC 7396 spells "delete this key".

So: **adding a resource to `tauri.conf.json` means also deciding what Windows
does with it.** If it is macOS-only, null it out in `tauri.windows.conf.json`.
Arrays and scalars (e.g. `bundle.targets`) replace wholesale and need no such
care.

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
| `MERIDIAN_DB` | `~/.meridian/meridian.db` | Path to meridian's output SQLite file |
| `MERIDIAN_DB_KEY` | (unset → unencrypted) | 64-hex-char SQLCipher raw key for `meridian.db`. Generated once by the tray (`tray/src-tauri/src/db_key.rs`, stored in the OS keychain) and mirrored into `.env` for the daemon to read — never set this by hand except for local testing (see `meridian-core/src/db_crypto.rs`). |
| `POLL_INTERVAL_SECS` | `60` | ETL poll cadence in seconds |
| `RUST_LOG` | `meridian=info` | Tracing filter |
| `SQLX_OFFLINE` | `true` (via `.cargo/config.toml`) | Prevents sqlx from hitting the DB at compile time |
| `MERIDIAN_OTLP_ENDPOINT` | (unset → default `http://localhost:5080/api/default/v1/traces`) | OpenObserve OTLP/HTTP traces endpoint override — consulted only on the DEV/bare shipping path (a packaged install ships to the release-baked central gateway instead; see the Observability section below) |
| `MERIDIAN_OO_AUTH` | DEPRECATED — ignored everywhere | OO credentials live in `settings.json` (`oo_email`/`oo_password`) |
| `MERIDIAN_TELEMETRY_DISABLED` | (unset → capture always on) | Hard kill switch for OTel span/log capture to the local spool — the sole log/trace sink; disabling it leaves only launchd's raw stdout/stderr crash-safety-net files. |
| `MERIDIAN_LAUNCHD_LOG_MAX_MB` | `10` | Size cap for each launchd-redirected raw log file (`daemon.log`, etc.); capped via copytruncate on the telemetry shipper's tick (`src/telemetry_spool/shipper.rs`). |
| `MERIDIAN_EMBEDDER_DIR` | `~/.meridian/models/<repo-basename>/` | Override the on-disk directory the session-distiller embedding weights live in (`src/embedder/provision.rs`). |
| `MERIDIAN_EMBEDDER_REPO` | `BAAI/bge-small-en-v1.5` | Override the HuggingFace repo the embedder weights are fetched from. |
| `DISTILLER_SEM_DEDUP_THR` | `0.86` | Cosine threshold for the distiller's semantic dedup (`src/worklog_pipeline/distiller/`). |
| `MERIDIAN_CAPTURE_RETENTION_DAYS` | `30` | Age floor for the capture_frames/capture_ui_events/capture_secondary_screens retention sweep (`src/etl/capture_retention.rs`) — only prunes rows both older than this AND already consumed by the ETL cursor. |

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

> **The daemon no longer reads screenpipe.** Since the slice-4b cutover, capture runs **in-process in the tray** (behind the `capture` feature): the forked `screenpipe-screen` + `screenpipe-a11y` crates produce a11y-tree/OCR frames + input events, which `meridian_core::capture` writes into **`capture_frames` / `capture_ui_events`** in `meridian.db`. The daemon ETL reads *those* tables (via `src/db/screenpipe.rs`, name unchanged for now) from the meridian pool — there is no screenpipe DB/process/pool anymore. **Implication:** a build with the `capture` feature OFF has **no data source** (the daemon produces no sessions); the shipping DMG must enable it. **Audio is dropped** (`get_audio_snippets` stubbed empty) and **gaps all classify `system_sleep`** (no in-process idle detection yet — `capture_trigger` is NULL); both are accepted v1 degradations with idle-detection/audio as future slices. The `SCREENPIPE_DB` env var + `Config::screenpipe_db` field have been removed (the daemon never reads a screenpipe DB anymore).
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

- The MCP server uses `sql.js` (pure WASM, no native compile step) — deliberately chosen over `better-sqlite3` after native-module ABI mismatches across Node versions/platforms caused real distribution pain (see `packages/meridian-mcp/src/db-cache.ts`'s header comment). Do not swap it back.
- UI API routes live in `ui/app/api/`; keep them thin — query, transform, return JSON
- No `any` types unless unavoidable and justified with a comment
- **Spawning the `meridian` binary from a UI route: ALWAYS use `selectMeridianBinary(meridianCandidates())` from `@/lib/meridian-bin`.** Never spawn a bare `'meridian'` (relies on `$PATH`), and never hand-roll a candidate list. The dashboard runs under **launchd**, whose PATH lacks Homebrew's `node`, so the `#!/usr/bin/env node` wrapper at `~/.local/bin/meridian` dies with `env: node: No such file or directory`. The helper probes the **native binary first** (`~/.meridian/app/bin/meridian`, no runtime deps → works under launchd), so it behaves identically in dev and installed. This bug is invisible in `dev-start` (dev installs a bash wrapper, not a node one) — it only surfaces on bundle/npm installs. `__tests__/meridian-bin.test.ts` guards the ordering. The one sanctioned exception is launching `meridian` in a user Terminal (`open -a Terminal …`, e.g. `api/update`), where an interactive login shell *does* have node/PATH.

### SQL migrations

- Add a new numbered file in `src/migrations/` — never modify an existing migration
- Include the file header comment on line 1
- The integration test helper `make_meridian_db()` runs all migrations; new migrations are covered automatically by `cargo test`

### Observability (logs & traces — local-only capture; dev ships full-fidelity to its own OO, packaged ships redacted error-only to central)

Any new or changed code path that does real work (daemon stages, the distiller/embedder, the worklog pipeline, coding-agent ingest) **must emit structured logs and traces** — not just `println!` to a terminal. Add proper logs and traces as you write the code, not as an afterthought.

**One pipeline, no duplicates.** The local OTel telemetry spool
(`~/.meridian/telemetry/{pending,sent,quarantine}/`, raw OTLP protobuf) is the
**sole** log/trace sink — there is no separate JSONL file and no
`tracing`-driven stdout/stderr mirror. Every `tracing::*!` call goes through this
one pipeline, unconditionally
(a local disk write, not a network call, so it never depends on `otlp_enabled`
or OpenObserve being reachable — the only escape hatch is
`MERIDIAN_TELEMETRY_DISABLED`, a dev/test kill switch). `meridian logs
[--service <name>] [-n N] [-f]` (`src/telemetry_spool/render.rs`) decodes this
same spool back into human-readable lines on demand — this is the one
supported way to read logs locally, replacing the old JSONL-tailing UI and the
old bash `meridian logs` (which used to tail launchd-redirected stdout/stderr
text).

The only thing this pipeline structurally can't capture is a hard crash
(panic before `init` runs, segfault, OOM kill) — for that, launchd's own
stdout/stderr redirect (`~/.meridian/logs/<service>.log` /
`<service>-error.log`, unrelated to `tracing`/`logging`) is the OS-level
safety net. It has no log volume during normal operation (nothing mirrors
into it deliberately) and is size-capped (`MERIDIAN_LAUNCHD_LOG_MAX_MB`,
copytruncate, `telemetry_spool::shipper`) and folded into diagnostics export
bundles alongside the spool.

**Shipping has TWO modes, split by install type**
(`src/observability/otlp_target.rs::resolve_otlp_target`). The daemon's
`telemetry_spool::shipper` background task is still the ONLY thing that ever
ships spooled files anywhere:

- **Dev / Bare checkout** → the engineer's OWN local OpenObserve, gated on
  `otlp_enabled` + `oo_email`/`oo_password`, `Basic` auth, **full fidelity**
  (no redaction). Unchanged.
- **Canonical (packaged DMG)** → Meridian's **central** OpenObserve via the
  ingest gateway (`ops/central-observability/`), gated on the
  `error_reporting_enabled` setting (**opt-out, default true**), `Bearer` auth
  with a release-baked write-only token, and **redacted + error-only**
  (`telemetry_spool::redact`). The endpoint and token are release-injected
  `option_env!` constants, so a source build is inert and can never ship to
  prod by accident.

Redaction applies to the **ship leg only** — it produces a separate stripped
copy to POST, so local capture and `meridian logs` stay full-fidelity. Two
things it guarantees that are easy to regress: only WARN+ logs / ERROR-status
spans egress at all, and `host.name` is replaced by a stable **pseudonym**
(`redact::pseudonymize_host`) rather than shipping the raw hostname, which on
macOS is routinely the account holder's real name. The tray's Sentry
`before_send` applies the identical pseudonym to `event.server_name`.

**Export Diagnostics remains the manual path**, unchanged and independent of
consent: tray Settings → Account → **Export Diagnostics** (or `meridian
telemetry export`) bundles the spool + the launchd crash-safety-net logs into
a `.tar.gz` the user hands to support, imported by hand with `meridian
telemetry import <bundle> --endpoint <url> --auth <base64>`. Retention
(default 7 days, `MERIDIAN_TELEMETRY_RETENTION_DAYS`) applies to both
`pending/` and `sent/`, regardless of shipping status.

- **Rust**: `tracing::info!/warn!/error!/debug!` with **structured fields** — never format data values into the message string (already enforced).
- **Wrap discrete operations in spans** (`tracing::info_span!` / `debug_span!`) and put the meaningful inputs, outputs, and metrics as **span attributes**, not buried in log lines. For an LLM/model call, capture the EXACT input as sent and output as received (post-cap/post-template — reflect any truncation that actually happened), plus real token counts/latency. See `src/llm/resolver.rs`'s `llm.call` span tree (request → infer → response) and `src/worklog_pipeline/distiller`'s `distil.run` span for the reference shape.
- **No duplication, no truncation of debug data**: emit each fact once, on the span that owns it; don't truncate the values you'd actually need to debug a misclassification. Keep static/identical-every-call blobs (e.g. the full system prompt) out of every trace where a size + a single archived copy suffices.
- **Set span status `ERROR`** (with a message) on failures, and log a `warning`/`error` with `.context`/`extra` at the failure boundary.
- **Shipping degrades silently**: code must never crash because OpenObserve is unreachable or export is disabled — capture (the only thing every install can rely on) is unaffected either way.

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

### Add a notification (plain toast or interactive nudge)

Read `NOTIFICATIONS.md` first — it is the integration guide for the
notification service (outbox → deliver → respond → consume lifecycle, category
registry, response handlers, expiry/persistence semantics, plugin gotchas, and
the packaged-build test recipe). In short:

1. Plain: one `notifications::enqueue(...)` call with a scoped `dedup_key`
   (`src/notifications.rs`). Never gate on settings in the producer — policy
   lives in `meridian-core` at drain time.
2. Interactive: add the category (id + buttons JSON) to
   `meridian-core/src/notifications.rs::categories`, stamp it on the row via
   `.category()/.actions()`, and handle the answer with a match arm in
   `src/notification_responses.rs` (handlers must be idempotent).
3. Interactive toasts only work in packaged builds (`UNUserNotificationCenter`
   needs a `.app` bundle) — test per the recipe in `NOTIFICATIONS.md`.

### Add a new MCP tool

1. Read `packages/meridian-mcp/dist/index.js` to understand existing tool structure
2. Edit the TypeScript source in `packages/meridian-mcp/src/`
3. Run `npm run build` in `packages/meridian-mcp/` and verify `dist/index.js` is updated

### Add a What's New entry per release

The dashboard's "What's New" modal (`ui/components/timeline/WhatsNewModal.tsx`, opened via the toolbar nav pill or auto-opened once per app version by the tray's `poll::whats_new_auto_open`) is **hand-curated**, deliberately separate from the auto-generated `CHANGELOG.md` — that file is commit-level and too internal to show end users (e.g. `hf-proxy: bake MERIDIAN_HF_ENDPOINT into the staging channel`).

1. Edit `tray/src-tauri/resources/whats-new.json` (compiled into the tray binary via `include_str!`, not Tauri resource-bundling — a rebuild always picks up the change).
2. Add a new object to the front of `releases` (newest-first): `version`, `date`, `highlights` (features, user-facing language), `fixes`. Rewrite each bullet in plain user terms — never paste a commit message verbatim.
3. Update `roadmap` if upcoming plans changed — `status` is `in-progress` | `planned` | `considering`.
4. Every string in this file is user-facing app text — plain hyphen `-` only, no em-dash, per the Hard Rules at the top of this file.
5. `cargo test -p meridian-tray` (from `tray/src-tauri/`) covers `whats_new_json_parses`, which fails the build if the JSON doesn't match the expected shape.

### Make a DMG release mandatory (force-install on old versions)

DMG auto-updates are consent-based (in-app banner + click) with one exception: when the update manifest declares a **minimum supported version** and the running app is below it, the app installs the update and relaunches automatically. Use this as a kill-switch for releases old versions must not keep running against (broken update path, data-corrupting bug) — not for routine releases.

1. Commit `tray/minimum-version` containing the floor as plain `X.Y.Z` (usually the new release's own version, forcing everyone older) **before cutting the release**. File absent or empty = consent-based, the default; a malformed value fails the release loudly (`scripts/package-updater.sh`).
2. The release ships it as a `Minimum-Version: X.Y.Z` line inside `latest.json`'s `notes` — the notes body is the transport because `tauri-plugin-updater` drops unknown manifest fields. The staging channel inherits it via `mirror-staging-release.sh`'s verbatim `latest.json` copy.
3. Installed apps below the floor force-update via `update::enforce_minimum_version` (`tray/src-tauri/src/update.rs`) — checked 30 s after launch and every 6 h thereafter, so long-running trays catch it without a relaunch. A failed forced install falls back to the consent banner and retries next cycle.
4. Empty or remove `tray/minimum-version` afterwards if later releases should go back to consent-based — the floor ships with **every** release while the file has content.

Only affects the DMG channel; npm/CLI installs still update via `meridian update`.

---

## Coding-agent pipeline (`src/coding_agent_session_ingest/`)

The coding-agent indexer + summariser run **inside the Rust daemon** (`src/coding_agent_session_ingest/`), spawned as gated tokio tasks from `main.rs`. They turn coding-agent conversations into segmented `app_sessions` rows, summarise sealed segments **with each agent's own CLI** (no cross-engine fallback), and write the summary for the worklog pipeline to pick up. Lifecycle is the `task_method` column: `coding_agent_live → pending_summariser → summarised`.

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
- **Summariser** (`summariser/`): routes each row to its own agent CLI — `claude.rs` / `codex.rs` / `copilot.rs` / `cursor_agent.rs` (2 attempts, no cross-engine fallback; a failed row is left pending for a later drain); writes `session_summary` + `summary_source`, flips `task_method` to `summarised`. cursor-agent is auth-probed lazily on first use, and auto-installed only behind the `CURSOR_AGENT_AUTO_INSTALL=1` opt-in (`cursor_agent_init.rs`). CLI: `meridian coding-agent-summarise`. See `summariser/README.md`.
- **Self-ingest guard**: copilot/cursor-agent persist their own summary runs into stores we ingest; `sources::sweep()` drops any conversation whose first user prompt carries `SUMMARY_PROMPT_MARKER` (log: `skipping summariser-artifact session`). This is the loop cut — do not remove it.
- **Worklog trigger**: `summarised` rows are picked up by the worklog pipeline (`src/worklog_pipeline/`) via `session_summary IS NOT NULL` — folded verbatim into the hour's activity summary alongside the distilled OCR sessions, then matched to tasks and drafted.

Source-adapter env overrides: `COPILOT_SESSION_STATE_DIR`, `VSCODE_USER_DIR`, `CURSOR_STATE_VSCDB`, `CURSOR_CLI_CHATS_DIR`, `ANTIGRAVITY_APP_DIR`.

> **Daemon config gotcha:** the daemon loads env via `dotenvy::dotenv_override()`, which walks UP from its launchd `WorkingDirectory` and stops at the first `.env`. Both install types converge on the **canonical `~/.meridian/.env`** (the same file the tray writes tracker creds to): the **`.app` DMG** (the tray stages the daemon via `tray/src-tauri/src/backend_install.rs`) sets `WorkingDirectory` to `~/.meridian` and reads it directly; **source/dev** reads the repo `.env`. (The old npm bundle install was retired - the DMG is the only packaged distribution now.) Edit `~/.meridian/.env` (then `meridian restart`) to tune daemon env on an installed system.

The pipeline is fully in Rust. The former Python `coding_agent_indexer` +
`coding_agent_summariser` packages **and the entire Python `services/` tree (the MLX
server, agents, eval harness, runtime packaging) have been removed.** Generation runs
only through the user's chosen third-party CLI provider (`src/llm/`), and the session
distiller's embedder now runs in-process via candle (`src/embedder/`,
`src/worklog_pipeline/distiller/`). There is no Python and no on-device generative model
anymore; a failing/rate-limited provider leaves work pending for the next cycle.

## DB write invariants (enforced in the Rust path)

- **`ticket_links` and `session_dimensions` writes must be idempotent.** Both tables have
  UNIQUE / composite-PK constraints with explicit `ON CONFLICT … DO UPDATE` policies. New
  writers must use the same UPSERT pattern. Never `DELETE` then `INSERT`.
- **Coding-agent segment idempotency:** the `(claude_session_uuid, segment_started_at)`
  unique index is the key (migration 027; `day_utc` was dropped in 028). The UPSERT refreshes
  a LIVE row but carries `WHERE sealed_at IS NULL`, so a SEALED row is immutable — the
  summariser only ever reads sealed rows.

### Quick command reference

```bash
# coding-agent ingest — runs inside the daemon; these are the one-shot CLIs
echo '{"transcript_path":"~/.claude/projects/.../<uuid>.jsonl"}' | meridian coding-agent-hook  # SessionEnd: seal one session
meridian coding-agent-summarise [--dry-run] [--day YYYY-MM-DD] [--limit N]                     # summarise the pending queue
```

---

## Git Hygiene

- Commit message style: `type(scope): short description` — e.g. `fix(etl): detect sleep gaps that span ETL run boundaries`
- `commit-msg` hook validates conventional commits format — fix message before retrying
- `pre-commit` hook runs `cargo fmt --check` and `cargo clippy -- -D warnings`
- `pre-push` hook runs the full suite: `cargo fmt` + `cargo clippy` + UI build + UI tests + security audit (claude CLI) + `cargo test`
- Never skip hooks with `--no-verify`
- Install hooks after cloning: `bash scripts/setup-hooks.sh`
- Never amend a commit that has already been pushed to `main` or `pre-main`

### PR target branch — `pre-main`, not `main`

- **Every feature/fix PR targets `pre-main`** (`gh pr create --base pre-main`), regardless of what any older doc or habit says — `pre-main` is the staging branch and is where all day-to-day work lands.
- `pre-main` is deployed to staging and gets exercised end-to-end there (including the staging DMG auto-update channel) before anything reaches production.
- **Only a maintainer** opens the `pre-main → main` release PR, and only once everything currently on `pre-main` has been verified working end-to-end on staging. Contributors should not open `main`-targeted PRs.
- `pre-main` is the staging/test channel, `main` is production.
