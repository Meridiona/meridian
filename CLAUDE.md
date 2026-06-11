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
        lib.rs           # Tauri library root
        poll.rs          # polling loop for /api/health, /api/today
        commands.rs      # Tauri commands (get_status, open_dashboard, etc.)
        format.rs        # duration formatting helpers (with unit tests)
        state.rs         # app state and health tracking
      Cargo.toml         # Tauri dependencies
    src/
      index.html         # popover UI template
      app.js             # event listeners, UI rendering
      style.css          # popover styling
    package.json         # npm/Node build config
    create-icons.sh      # icon generation script
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
| `SCREENPIPE_DB` | `~/.screenpipe/db.sqlite` | Path to screenpipe's SQLite file (read-only) |
| `MERIDIAN_DB` | `~/.meridian/meridian.db` | Path to meridian's output SQLite file |
| `POLL_INTERVAL_SECS` | `60` | ETL poll cadence in seconds |
| `RUST_LOG` | `meridian=info` | Tracing filter |
| `SQLX_OFFLINE` | `true` (via `.cargo/config.toml`) | Prevents sqlx from hitting the DB at compile time |
| `MERIDIAN_OTLP_ENDPOINT` | (unset → no export) | OpenObserve OTLP/HTTP traces endpoint (loaded from `.env`) |
| `MERIDIAN_OO_AUTH` | (unset → no auth) | Base64 `user:password` for OpenObserve OTLP auth |
| `MLX_SERVER_URL` | (unset → in-process load) | URL of a running MLX classifier server (eval pipeline) |
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

The coding-agent indexer + summariser run **inside the Rust daemon** (`src/coding_agent_session_ingest/`), spawned as gated tokio tasks from `main.rs`. They turn coding-agent conversations into segmented `app_sessions` rows, summarise sealed segments **with each agent's own CLI** (MLX as the shared fallback), and hand them to the classifier on their summary. Lifecycle is the `task_method` column: `coding_agent_live → pending_summariser → pending_classifier → mlx_direct`.

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
- **Summariser** (`summariser/`): routes each row to its own agent CLI — `claude.rs` / `codex.rs` / `copilot.rs` / `cursor_agent.rs` (2 attempts) → `mlx.rs` fallback (`/summarise`); writes `session_summary` + `summary_source`, flips `task_method` to `pending_classifier`. cursor-agent is auth-probed lazily on first use, and auto-installed only behind the `CURSOR_AGENT_AUTO_INSTALL=1` opt-in (`cursor_agent_init.rs`). CLI: `meridian coding-agent-summarise`. See `summariser/README.md`.
- **Self-ingest guard**: copilot/cursor-agent persist their own summary runs into stores we ingest; `sources::sweep()` drops any conversation whose first user prompt carries `SUMMARY_PROMPT_MARKER` (log: `skipping summariser-artifact session`). This is the loop cut — do not remove it.
- **Classify trigger** (`src/intelligence/task_linker/`): a non-cursor branch classifies `pending_classifier` rows on the **summary** (not the transcript), preserving the summariser's summary. CLI: `meridian coding-agent-classify`.

Source-adapter env overrides: `COPILOT_SESSION_STATE_DIR`, `VSCODE_USER_DIR`, `CURSOR_STATE_VSCDB`, `CURSOR_CLI_CHATS_DIR`, `ANTIGRAVITY_APP_DIR`.

> **Daemon config gotcha:** on a bundle install the daemon's `WorkingDirectory` is `~/.meridian/app`, and dotenvy stops at the FIRST `.env` walking up — so the daemon reads **`~/.meridian/app/.env`**, not the repo `.env`. Edit that file (then `meridian restart`) when tuning daemon env on an installed system.

The pipeline is fully ported to Rust; the former Python `coding_agent_indexer` + `coding_agent_summariser` packages have been removed. The MLX server (`agents/server.py`) is the only remaining Python hop (it serves `/summarise` + `/classify_sessions`).

## Python agent service (`services/`)

These Python services still run alongside the Rust daemon:

1. **MLX classifier + summariser** (`agents/server.py`, `run_task_linker_mlx.py`) — the persistent FastAPI model server (`com.meridiona.mlx-server.plist`). Exposes `/classify_sessions` (Rust calls it to classify) and `/summarise` (the coding-agent summariser's MLX fallback). The one Python piece the pipeline can't replace (outlines + mlx-lm are Python-only).
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
meridian coding-agent-classify                                                                  # classify summarised rows

# MLX classifier — call the running server directly
curl -s -X POST http://127.0.0.1:7823/classify_sessions \
  -H "Content-Type: application/json" \
  -d '{"session_ids": [<ID>]}' | jq .

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
