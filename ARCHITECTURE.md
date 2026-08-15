# Architecture

A map of how Meridian is put together, aimed at someone reading the codebase for the
first time. It covers the shape of the system and the reasoning behind the parts that
are not obvious.

For per-task recipes, conventions, and the accumulated gotchas, see
[CLAUDE.md](CLAUDE.md) - it is written for AI coding agents but is the most complete
reference in the repo, and this document links into it throughout.

---

## The idea in one paragraph

Meridian watches what you do, works out which task it belongs to, and drafts the
project-management update for you to approve. Everything upstream of that approval is
mechanical: capture raw screen activity, cut it into clean per-app work sessions,
enrich those sessions with what the coding agents in your terminal were doing, group
them into something worth reporting, match that to a ticket, and write a draft. The
interesting engineering is in making each of those steps correct across sleep, idle,
app switches, crashes, and restarts - because a confidently wrong task assignment is
worse than no feature at all.

---

## The pieces

Meridian is two long-running processes plus two libraries, all on your machine. There
is no server.

```
┌──────────────────────────────┐        ┌──────────────────────────────┐
│  Tray  (Tauri, Rust + web)   │        │  Daemon  (Rust, headless)    │
│                              │        │                              │
│  • screen + a11y capture     │ writes │  • ETL: frames → sessions    │
│  • hosts the dashboard UI    │───────▶│  • coding-agent ingest       │
│  • tray menu, notifications  │        │  • worklog pipeline          │
│  • the process you click     │◀───────│  • tracker sync              │
└──────────────────────────────┘  reads └──────────────────────────────┘
                │                              │
                └──────────┬───────────────────┘
                           ▼
                 ~/.meridian/meridian.db
              (SQLite + SQLCipher, WAL mode)
                           ▲
                           │ read-only
                 ┌─────────┴──────────┐
                 │  MCP server (TS)   │  exposes your data to Claude, Cursor, etc.
                 └────────────────────┘
```

| Component | Path | What it is |
|---|---|---|
| **Daemon** | `src/` | Headless Rust process. Runs the ETL loop, the coding-agent indexer and summariser, the worklog pipeline, and tracker sync. Started by launchd (macOS) or the equivalent on Windows. |
| **Tray** | `tray/src-tauri/` | The Tauri app. Owns screen capture, hosts the dashboard webview, and is the only part with a UI. |
| **Shared core** | `meridian-core/` | The data layer both processes depend on - DB access, settings, and the read queries that back the dashboard. |
| **Dashboard** | `ui/` | Next.js, built as a **static export** and embedded in the tray binary. No Node server, no HTTP API. |
| **MCP server** | `packages/meridian-mcp/` | TypeScript. Exposes the same data to AI clients over the Model Context Protocol. |
| **OAuth** | `meridian-oauth/` | Browser-based OAuth flows for Jira and Trello. |

The Rust workspace is `[".", "meridian-core", "meridian-oauth", "tray/src-tauri"]`.
Because the repo root is itself a package, **`cargo test` and `cargo clippy` must be
run with `--workspace`** or they silently test only the daemon. This is the single
most common way to get a green run that proves nothing.

---

## The database is the interface

`~/.meridian/meridian.db` is how the two processes communicate. There is no IPC
protocol, no message bus, and no HTTP between them - they open the same SQLite file in
WAL mode and coordinate through tables.

This keeps each process simple and independently restartable, and it means every stage
of the pipeline leaves an inspectable audit trail on disk. The costs are real and worth
knowing before you write to it:

- **Ownership is split, and inverted from what you would guess.** The tray *writes*
  the capture tables (`capture_frames`, `capture_ui_events`); the daemon *reads* them.
  Everything downstream is the other way around.
- **Two writers means write discipline matters.** Writes must be idempotent - see the
  UPSERT invariants in [CLAUDE.md](CLAUDE.md#db-write-invariants-enforced-in-the-rust-path).
  Never `DELETE` then `INSERT`.
- **The file is encrypted** (SQLCipher). The key is generated on first run by the tray
  and held in the OS keychain, mirrored into `~/.meridian/.env` because the daemon is
  headless and cannot prompt for keychain access.
- **Corruption is handled explicitly**, not shrugged off: detection latches the ETL
  loop and recovery is a deliberate operator action (`meridian db repair`). The
  reasoning is in `src/db/integrity.rs` and `src/db/repair/`.

Schema changes are append-only numbered migrations in `src/migrations/` (79 and
counting). **Never edit an existing migration** - installed databases have already
applied it, and sqlx verifies checksums on startup.

---

## How activity becomes a ticket update

### 1. Capture (tray, in-process)

Forked [screenpipe](https://screenpi.pe) crates produce accessibility-tree and OCR
frames plus input events, written straight into `meridian.db`. This runs *inside* the
tray rather than as a child process so macOS asks for screen-recording permission once,
for the app you actually launched.

### 2. ETL - frames become sessions (`src/etl/`)

`run_etl()` runs every poll interval and is the heart of the system. It walks new
frames in batches, maintaining a state machine keyed on the active app: while the app
stays the same the block stays open, and when it changes the block is closed into
`app_sessions` with its real duration.

The subtlety is everything that is not a clean app switch. Sleep, idle, and daemon
restarts all produce gaps in the frame stream, and a session whose duration silently
includes eight hours of sleep is worse than useless. So gaps beyond a threshold are
detected - both within a batch and across runs - classified, recorded in `gaps`, and
excluded from session duration. The in-progress block lives in a single-row
`active_session` table until something closes it.

These rules are pinned by integration tests in `tests/integration_etl.rs`, which run
against in-memory SQLite. **If you change ETL logic, read [docs/testing.md](docs/testing.md)
first** - the invariants listed there are load-bearing.

### 3. Coding-agent ingest (`src/coding_agent_session_ingest/`)

Screen capture tells you *that* you were in a terminal. It does not tell you what you
were building. So Meridian reads the on-disk transcripts the coding agents already
keep - Claude Code, Codex, Copilot CLI and VS Code chat, Cursor, cursor-agent - and
turns them into sessions of their own.

Each agent stores conversations differently (JSONL files, VS Code op-logs, SQLite blob
stores), so a per-agent adapter under `sources/` normalises them into a common record
type. Everything downstream is agent-blind.

Sealed sessions are summarised **by each agent's own CLI** - the tool is already
installed and authenticated, so there is no separate model to provision. Lifecycle is
tracked in the `task_method` column: `coding_agent_live → pending_summariser →
summarised`.

> One non-obvious guard: these agents persist Meridian's own summarisation runs into
> the same stores Meridian ingests. A marker in the summary prompt lets the sweeper
> drop them. Removing that check creates a feedback loop.

### 4. Worklog pipeline (`src/worklog_pipeline/`)

Summarised coding-agent rows and distilled screen sessions are folded into an hourly
activity summary, matched to a specific task, and drafted into a worklog. A small local
embedding model (via [candle](https://github.com/huggingface/candle)) de-duplicates
near-identical activity before summarising; it is the only model that runs fully
on-device.

Generation runs through whichever provider you configured (`src/llm/`). A failing or
rate-limited provider leaves work pending for the next cycle rather than dropping it.

### 5. Approval and sync (`src/pm_worklog/`)

Drafts appear in the dashboard. **Nothing reaches your tracker until you approve it** -
that gate is the product's core promise, and it is enforced in one place rather than
per-integration. On approval the update is posted directly from your machine to Jira,
GitHub, Linear, Trello, or Azure DevOps.

---

## The dashboard runs inside the tray

The Next.js app is a static export (`output: 'export'`) bundled into the tray binary.
There is no Node server and there are no `/api` routes.

- **DB-backed reads** live in `meridian-core/src/readers/` - one source of truth the
  daemon and tray both use.
- **File, environment, and process operations** are Tauri commands in
  `tray/src-tauri/src/commands/`.
- **The frontend reaches Rust only through `ui/lib/bridge.ts`** - `load`/`mutate` call
  `invoke`, and `subscribe` listens on the Tauri event bus. Live streams that used to
  be SSE are now events emitted by the tray's poll loop.

Adding a route means following the playbook in
[CLAUDE.md](CLAUDE.md#porting-a-dashboard-route-to-rust-next-fold-playbook). Two traps
worth knowing up front: a new window label must be added to `capabilities/default.json`
or its `invoke` calls are silently denied, and **`window.confirm`/`alert`/`prompt` do
nothing in the packaged app** - `confirm()` always returns `false`, which is
indistinguishable from the user clicking Cancel. Use `@/components/ConfirmDialog`.

---

## Observability

Every `tracing` call is captured to a local OTLP spool at `~/.meridian/telemetry/`,
full fidelity, unconditionally. `meridian logs` decodes it back into readable lines -
that is the supported way to read logs locally.

Shipping is a separate question from capture, and splits by install type:

- **Source builds** ship to your own OpenObserve if you configure one, unredacted. The
  central endpoint and token are injected only at release time, so a source build can
  never ship to production by accident.
- **Packaged builds** ship redacted, error-only diagnostics to Meridiana - on by
  default, switchable off in Settings. `src/telemetry_spool/redact.rs` is the privacy
  boundary and its module header explains the two fail-closed rules.

New code that does real work is expected to emit structured logs and spans as it is
written. The two ways instrumentation silently disappears - the `EnvFilter` prefix rule
and the attribute allowlist - are documented in
[CLAUDE.md](CLAUDE.md#observability-logs--traces--local-only-capture-dev-ships-full-fidelity-to-its-own-oo-packaged-ships-redacted-error-only-to-central).

---

## Design decisions worth knowing

| Decision | Why |
|---|---|
| **Local-first, no server** | Screen content is the most sensitive data a dev tool can hold. Not having a server is a stronger guarantee than promising not to look. |
| **SQLite as the process boundary** | Two simple, independently restartable processes and a durable audit trail, instead of an IPC protocol to maintain. |
| **Capture in-process in the tray** | One TCC permission entry for the app the user launched, rather than a helper process the OS asks about separately. |
| **Summarise with the user's own agent CLI** | No model to provision, no inference cost, no extra credentials - and it is already authenticated. |
| **Approval as a hard gate** | A wrong ticket update is worse than a missing one. The gate makes the failure mode recoverable. |
| **`sql.js` in the MCP server** | Native SQLite bindings caused real ABI-mismatch pain across Node versions and platforms. Pure WASM has no compile step. Do not swap it back. |

---

## Where to go next

- **[CONTRIBUTING.md](CONTRIBUTING.md)** - dev environment, build, test, PR workflow.
- **[CLAUDE.md](CLAUDE.md)** - the deep reference: conventions, per-task recipes, and
  the failure modes that have already bitten someone.
- **[docs/testing.md](docs/testing.md)** - required reading before touching ETL or migrations.
- **[SETUP.md](SETUP.md)** - installing and configuring a real deployment.
- **[docs/privacy.md](docs/privacy.md)** - exactly what does and does not leave the
  machine.
- **[docs/vision.md](docs/vision.md)** - what the product is trying to be, for product decisions.
