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

### 3. capture DB compatibility

- [ ] **empty capture DB** — no capture_frames rows yet; ETL runs without crash, no sessions written, cursor remains at 0.
- [ ] **WAL mode** — meridian.db operating in WAL mode; no `SQLITE_BUSY` or `SQLITE_LOCKED` errors under concurrent read/write from the tray-side capture writer.

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
- [ ] **app name branding** — UI shows "Meridian" / "Meridiona", not "meridiona" (typo fix commit `331f456`).

### 6. MCP server

- [ ] **starts without meridian DB** — `node dist/index.js` when `~/.meridian/meridian.db` does not exist; server returns a clear error on tool call, not an unhandled exception.
- [ ] **query today's sessions** — call sessions tool for today's date; returns correct JSON matching DB contents.
- [ ] **date boundary respects local timezone** — sessions straddling midnight are attributed to the correct local day.
- [ ] **focus time aggregation** — focus time query sums `duration_s` correctly per app; no double-counting.
- [ ] **`MERIDIAN_DB` env var** — MCP server reads from custom path when env var is set.
- [ ] **read-only connection** — MCP server opens DB with `readonly: true`; no accidental writes.

### 7. session categorization

Session categorization runs **fully in Rust** (`src/intelligence/session_categorizer/`) — there is no Python, no hermes, and no on-device model in this path. The `cat_smoke` binary reads real `app_sessions` rows from `meridian.db`, runs `categorize()` on each, and prints the result without writing anything back.

```bash
# Categorize every session (read-only — nothing is written)
cargo run --bin cat_smoke

# Limit the sample, or filter to one app
cargo run --bin cat_smoke -- --limit 50
cargo run --bin cat_smoke -- --app "Google Chrome"
```

- [ ] **category and confidence assigned** — each row gets a non-empty `category` and a `confidence` in `[0.0, 1.0]`.
- [ ] **trivial / empty sessions** — a row with empty `session_text` is categorized without crashing (no LLM required for the trivial path).

Ticket linking and worklog drafting that consume these categories go through the user's chosen CLI provider (`src/llm/`); those LLM hops are exercised end-to-end by running the daemon, not by a dedicated smoke test.

### 8. configuration and startup

- [ ] **`POLL_INTERVAL_SECS` override** — daemon polls at the configured cadence (verify via log timestamps).
- [ ] **`RUST_LOG=debug`** — debug logging produces frame-level detail without crashing.
- [ ] **graceful shutdown on SIGTERM** — `kill <pid>`; daemon finishes the current ETL pass and exits cleanly.
- [ ] **graceful shutdown on Ctrl-C** — same as SIGTERM.

### 9. Windows

Windows is a **shipped, maintained platform**, not a port in progress — the daemon,
tray, notifications, packaging and release channel all have real Windows
implementations, and CI runs a dedicated `rust-windows` job (clippy `-D warnings` +
`cargo test`) on every PR into `pre-main`.

This checklist did not follow it there, which is the gap these items close. **None of
the automated suites cover the Windows install path**: `tests/install/` is
launchd-shaped by construction (`plutil -lint`, plist rendering), and the local git
hooks are bash, so a Windows contributor's `pre-push` cannot catch a Windows-only
failure — only CI can. Everything below is therefore manual, on a real Windows 10/11
machine.

**Daemon lifecycle** (`tray/src-tauri/src/commands/daemon_control.rs` — named pipe +
Task Scheduler, where macOS uses a Unix socket + launchd):

- [ ] **task registered on install** — after the NSIS install, `schtasks /Query /TN "Meridian Daemon"` lists the task.
- [ ] **start / stop from the tray** — the tray's daemon toggle drives `schtasks /Run` and `/End`; tray status reflects the change.
- [ ] **policy-blocked `schtasks /Run` falls back** — the documented fallback spawns the staged `.exe` directly; the daemon still comes up.
- [ ] **named-pipe control** — `\\.\pipe\meridian-daemon-<user>` accepts the tray's commands (this is the socket's Windows counterpart; a silent failure here looks like an unresponsive daemon).
- [ ] **no console windows flash** — every spawned child uses `CREATE_NO_WINDOW` (`meridian-core/src/proc_ext.rs`); a stray black window on a poll tick is a regression.

**Paths and config** (`meridian-core/src/util/paths.rs`):

- [ ] **`%USERPROFILE%` resolution** — `~/.meridian` resolves under the user profile, with backslash separators, and the daemon creates it if absent.
- [ ] **`MERIDIAN_DB` override with a Windows path** — e.g. `C:\temp\test.db`; the DB is created there.

**Capture and permissions:**

- [ ] **`Windows.Media.Ocr` path produces frames** — sessions appear (the OCR engine is async/fallible here, unlike Apple Vision).
- [ ] **no TCC prompts** — Accessibility/Screen-Recording checks report granted without prompting; Windows Graphics Capture and UI Automation need no persistent grant.
- [ ] **notification permission** — the WinRT `ToastNotifier::Setting()` check reports correctly, and a denied state points at **Windows** notification settings, not macOS wording.

**Known macOS-only, degrade gracefully — confirm the fallback, not the feature:**

- [ ] **app icons** — `NSWorkspace` icon extraction has no Windows equivalent; rows must fall back to the letter monogram, not a broken image.
- [ ] **DRM/streaming detection** — the AppleScript detector is macOS-only, so frames are **not** skipped during protected playback on Windows. Confirm this is still the intended trade-off before a release.

**Packaging and update:**

- [ ] **NSIS installer, per-user** — installs under `currentUser` without an admin prompt.
- [ ] **auto-update** — the updater manifest covers `windows-x86_64` and is signed with the same minisign key as the macOS DMG; an update installs and relaunches.
- [ ] **uninstall** — removes the scheduled task (`schtasks /Delete /F`) and leaves no orphaned daemon.

### 10. LLM experimentation (dev-only LLM Lab)

The Python `deepeval` + MLX golden-dataset eval harness that lived under `services/tests/evals/` was removed along with the rest of the Python `services/` tree — there is no on-device model to score anymore. Generation now runs through the user's chosen third-party CLI provider (`src/llm/`).

Prompt and provider experimentation happens through the **dev-only LLM Lab** (`meridian llm-experiment run|create|exec|list|get …`), which writes only the `llm_experiment*` tables and never touches production tables. Like every other LLM call, its runs emit OTel spans to OpenObserve, so a run is inspectable as a trace tree there.

## Install-package tests

Tests for the install package (`install.sh`, `scripts/meridian-cli.sh`, the daemon installers, and the plist templates) live under `tests/install/`.

> **macOS only, by construction.** This suite lints plists with `plutil` and renders
> launchd templates, so it has no meaning on Windows and there is no Windows
> equivalent — the NSIS installer and the Task Scheduler registration have **no
> automated coverage at all**. Section 9 above is the manual substitute; treat it as
> required before a release that touches install, update, or daemon lifecycle.

Run them with:

```bash
bash tests/install/run.sh
```

Coverage:

- **Syntax** — `bash -n` and (if available) `shellcheck` on every shell script.
- **Plist linting** — `plutil -lint` on `com.meridiona.daemon.plist` and `com.meridiona.screenpipe.plist`.
- **install.sh dry-run** — `./install.sh --dry-run --skip-permissions --skip-env --no-ui --no-daemon` exits 0.
- **install.sh --help** — prints usage including all flags (`--no-ui`, `--dry-run`, `--no-daemon`, `--skip-permissions`, `--skip-env`).
- **meridian CLI** — `--help`, `status`, `doctor`, and unknown-command paths all exit cleanly.
- **Plist rendering** — runs `install-daemon.sh` against a temp HOME, verifies all `{{placeholders}}` are substituted (no leftover `{{...}}` tokens).
- **Env collection** — calls the `get_env_value` / `set_env_value` helpers in isolation; verifies idempotency (re-setting the same key replaces in place, doesn't append duplicates).

The suite does NOT actually install or load launchd agents — it stays within the cloned repo and uses temp directories. Run time is under 30 seconds.

Pre-push hook integration: not currently wired (the test suite is opt-in). If you want to gate pushes on the install tests, append `bash tests/install/run.sh` to your `.git/hooks/pre-push`.
