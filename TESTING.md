# Meridian regression testing checklist

> **purpose**: prevent regressions. test core ETL paths rigorously any time you touch session boundaries, DB schema, migrations, or the MCP server.

## critical edge cases (sorted by regression frequency)

### 1. ETL session boundary detection

these break whenever `runner.rs` or `extractor.rs` changes. test ALL of these after any ETL modification.

- [ ] **app-switch detection** — change focused app in screenpipe fixture; verify previous session closes and new one opens.
- [ ] **single-frame session duration** — a session containing exactly one frame must have `duration_s > 0`. was broken: single-frame sessions returned 0s (fixed: Option D, commit `317ceb2`).
- [ ] **sleep gap spanning ETL run boundary** — machine sleeps between two ETL runs; gap frames must not create a phantom session or extend the last real session. was broken (fixed: commit `a8f2280`).
- [ ] **cursor advances correctly** — after a successful run, `last_processed_frame_id` matches the highest frame_id in the batch; next run picks up only new frames.
- [ ] **idempotency** — run ETL twice on the same screenpipe data; verify no duplicate rows appear in `app_sessions`.
- [ ] **active session upsert** — open/in-progress session must update its `ended_at` and `duration_s` on each poll, not insert a new row.
- [ ] **active session closes on completion** — when app changes, the open `active_session` row moves to `app_sessions` with final timestamps.
- [ ] **back-to-back same-app frames** — multiple consecutive frames in the same app stay in one session (no spurious split).
- [ ] **empty frame batch** — zero new frames since last cursor; ETL exits the poll cycle cleanly, cursor unchanged.

### 2. database schema and migrations

- [ ] **fresh install** — delete `~/.meridian/` entirely; start daemon; verify DB created, migrations applied, all tables present.
- [ ] **migration idempotency** — run daemon a second time against already-migrated DB; no error, no duplicate migration.
- [ ] **existing rows survive migration** — add a new column via migration; pre-existing `app_sessions` rows must still be readable with correct values in old columns.
- [ ] **`~/.meridian/` auto-created** — parent directory does not exist at startup; daemon creates it without error.
- [ ] **meridian DB path override** — `MERIDIAN_DB=/tmp/test.db ./meridian`; DB created at the custom path, not the default.
- [ ] **tilde expansion in env vars** — `MERIDIAN_DB=~/custom/meridian.db`; tilde expanded correctly to home directory.

### 3. screenpipe DB compatibility

- [ ] **missing screenpipe DB** — `SCREENPIPE_DB=/nonexistent.db ./meridian`; daemon exits with a clear, readable error message (not a panic).
- [ ] **empty screenpipe DB** — screenpipe DB exists but contains zero frames; ETL runs without crash, no sessions written, cursor remains at 0.
- [ ] **screenpipe DB opened read-only** — verify no write lock is held on screenpipe's DB while meridian is running (screenpipe must continue writing frames unimpeded).
- [ ] **WAL mode** — both DBs operating in WAL mode; no `SQLITE_BUSY` or `SQLITE_LOCKED` errors under concurrent read/write.
- [ ] **screenpipe DB path override** — `SCREENPIPE_DB=/custom/path.db`; daemon reads from the custom path.

### 4. session data extraction

- [ ] **OCR samples capped at 20 and deduplicated** — a block with 50+ frames, some repeating the same text; `ocr_samples` JSON array contains at most 20 entries, each unique, valid JSON.
- [ ] **window titles aggregated** — multiple windows seen in one session; `window_titles` is a JSON array of `{title, count}` sorted by count descending.
- [ ] **audio snippets deduplicated** — repeated transcription chunks (same text) stored once in `audio_snippets`; only the earliest timestamp kept.
- [ ] **signals deduplicated** — same clipboard value copied multiple times appears once in `signals`.
- [ ] **elements deduplicated** — same accessibility element (text + role) seen across frames stored once in `elements_samples`.
- [ ] **signals captured** — clipboard copy events during a session appear in `signals` JSON array.
- [ ] **`min_frame_id` / `max_frame_id`** — values span exactly the frames in the session block; no off-by-one.
- [ ] **`frame_count`** — equals the actual number of frames in the block, not an estimate.
- [ ] **null-safe extraction** — frames with null OCR, null audio, or null accessibility data do not crash the extractor.
- [ ] **category and confidence assigned** — completed session has non-empty `category` string and `confidence` between 0.0 and 1.0.
- [ ] **gaps recorded** — a gap > 300s between sessions produces a row in the `gaps` table with correct `kind` (`user_idle` or `system_sleep`) and `duration_s`.

### 5. UI dashboard

- [ ] **sessions page loads** — `npm run dev` in `ui/`; sessions page renders without JS errors.
- [ ] **load more pagination** — scroll to bottom; "load more" fetches the next page of sessions without duplicating visible rows (commit `15d0fd7`).
- [ ] **active session card** — while meridian daemon is running, active session card shows current app and updates duration.
- [ ] **category badge** — session cards and active session card show a CategoryBadge with emoji, label, and hex color matching the `category` field.
- [ ] **category breakdown chart** — dashboard "By Category" section renders a horizontal bar chart for the top non-idle categories; bars use the correct colors.
- [ ] **timeline category colors** — day timeline segment colors come from `getCategoryMeta(category).color`, not app-name hashing.
- [ ] **focus donut chart** — donut reflects actual session durations; percentages sum to 100%.
- [ ] **stats row totals** — total active time and session count match what is in the DB.
- [ ] **app name branding** — UI shows "Meridian" / "Meridiona", not "Meridiana" (typo fix commit `331f456`).

### 6. MCP server

- [ ] **starts without meridian DB** — `node dist/index.js` when `~/.meridian/meridian.db` does not exist; server returns a clear error on tool call, not an unhandled exception.
- [ ] **query today's sessions** — call sessions tool for today's date; returns correct JSON matching DB contents.
- [ ] **date boundary respects local timezone** — sessions straddling midnight are attributed to the correct local day.
- [ ] **focus time aggregation** — focus time query sums `duration_s` correctly per app; no double-counting.
- [ ] **`MERIDIAN_DB` env var** — MCP server reads from custom path when env var is set.
- [ ] **read-only connection** — MCP server opens DB with `readonly: true`; no accidental writes.

### 7. task linker / hermes classification

These are real integration tests — they call the actual `services/agents/run_task_linker.py`, which fetches session data from a temp DB, calls hermes, and returns a classification. Rust writes the result back to the DB.

**Prerequisites**

```bash
# 1. python3 in PATH (check)
python3 --version

# 2. hermes configured (must exist)
ls services/.hermes/config.yaml

# 3. venv with dependencies installed
cd services && python3 -m venv .venv && .venv/bin/pip install -r requirements.txt
```

**Classify a single real session (writes to DB)**

```bash
# dry run — see what hermes would decide without writing
cargo run --bin backfill_task_classification -- --session <ID> --dry-run

# real run — calls hermes, writes task_key + dimensions to ~/.meridian/meridian.db
cargo run --bin backfill_task_classification -- --session <ID>

# check what was written
sqlite3 ~/.meridian/meridian.db \
  "SELECT id, task_key, task_method, task_routing, task_confidence FROM app_sessions WHERE id = <ID>"
```

**Watch logs while it runs**

```bash
# in a second terminal — tail Python logs (hermes output goes to stderr)
RUST_LOG=meridian=debug cargo run --bin backfill_task_classification -- --session <ID> 2>&1 | tee /tmp/classify.log

# or tail the tagger daemon log if the daemon is running
tail -f ~/.meridian/logs/tagger-daemon.log
```

**Run the automated smoke tests**

```bash
# All four tests (two require hermes, two are always-on prefilter tests)
cargo test --test task_linker_smoke

# Verbose output — see which tests ran vs. skipped
cargo test --test task_linker_smoke -- --nocapture

# One specific test
cargo test --test task_linker_smoke real_classification_writes_task_and_advances_cursor -- --nocapture
```

**What each test does**

| Test | Needs hermes | What it verifies |
|---|---|---|
| `real_classification_writes_task_and_advances_cursor` | Yes | Full round-trip: hermes classifies, `task_method` written to DB, cursor advanced, `agent_run` recorded |
| `real_classification_does_not_reprocess_classified_session` | Yes | Running twice classifies the session exactly once |
| `short_session_is_not_classified` | No | Sessions under `min_classification_duration_s` never reach Python |
| `trivial_session_is_marked_overhead_without_python` | No | Empty `session_text` → `prefilter_trivial/skip` without LLM |

**Skip vs. fail**

If python3 is missing, `services/agents/run_task_linker.py` is not found, or `services/.hermes/config.yaml` does not exist, the hermes-dependent tests print `SKIP:` and exit cleanly. The test suite still passes. Run with `--nocapture` to see skip reasons.

**What the tests do NOT assert**

They do not assert a specific `task_key` — that is LLM output and varies by model and session content. They assert that `task_method IS NOT NULL` (something was written) and that cursor + audit rows are consistent.

### 8. configuration and startup

- [ ] **`POLL_INTERVAL_SECS` override** — daemon polls at the configured cadence (verify via log timestamps).
- [ ] **`RUST_LOG=debug`** — debug logging produces frame-level detail without crashing.
- [ ] **graceful shutdown on SIGTERM** — `kill <pid>`; daemon finishes the current ETL pass and exits cleanly.
- [ ] **graceful shutdown on Ctrl-C** — same as SIGTERM.
