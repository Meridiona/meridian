# Architecture Overview

Meridian is a single-process Rust daemon. No network, no auth, no HTTP server — local-only SQLite, running 24/7 alongside screenpipe.

## System diagram

```
screenpipe.db (read-only)
       │
       ▼
  ETL runner  ──────────────────────────────────────────────────┐
  (every 60s)                                                   │
       │                                                         │
       ├─ get_frames_since(cursor)                              │
       ├─ detect app-switch boundaries                          │
       ├─ extract_block_context (OCR, audio, signals)           │
       ├─ upsert active_session (open block)                    │
       └─ close completed sessions → app_sessions               │
                                                                 ▼
                                                        meridian.db
                                                                 │
                                                                 ▼
  Classification ──────── POST /classify_sessions ─────► MLX server
  (per tick)              http://127.0.0.1:7823          (Qwen3.5-9B,
       │                                                  model in memory)
       └─ writes ticket_links, session_dimensions ────────────► meridian.db
```

## Components

### Rust daemon (`src/`)

The core ETL process. Two SQLite connection pools:
- `screenpipe` — read-only WAL connection to screenpipe's database
- `meridian` — read-write WAL connection to Meridian's output database

On startup: removes partial sessions left by a previous crash (`cleanup_incomplete_runs`), then immediately runs the first ETL pass before entering the poll loop.

Poll loop: `tokio::select!` over `SIGINT`/`SIGTERM` and a sleep timer. Calls `run_etl()` on each tick. Graceful shutdown closes both pools.

### Python classification service (`services/`)

Runs alongside the Rust daemon as a separate launchd agent. Calls the MLX inference server to classify completed sessions to specific Jira tasks, then writes the result back to `meridian.db`.

### MLX inference server

A persistent FastAPI process (port 7823) that loads Qwen3.5-9B once at startup. The Rust daemon HTTP-calls it for each session. No per-request cold load.

### Next.js dashboard (`ui/`)

Local dashboard at `http://localhost:3000`. Reads from `meridian.db` via `better-sqlite3` API routes.

### MCP server (`packages/meridian-mcp/`)

TypeScript server exposing session data to MCP-compatible AI tools. Uses `sql.js` (pure WebAssembly SQLite, no native deps). See [MCP Server →](/mcp-server).

## Source layout

```
src/
  main.rs              # tokio::main, signal handling, poll loop
  lib.rs               # public crate root
  config.rs            # Config::from_env() — reads env vars, expands ~
  db/
    meridian.rs        # writes app_sessions, active_session, etl_runs, etl_cursor, gaps
    screenpipe.rs      # read-only queries against screenpipe's frames/ocr/audio/ui_events
  etl/
    runner.rs          # run_etl() — batch loop, gap detection, block state machine
    extractor.rs       # extract_block_context() — OCR, audio, signals, window titles
  migrations/
    001_initial.sql    # app_sessions, active_session, etl_runs, etl_cursor
    002_gaps.sql       # gaps table, idle_frame_count columns
```

## Design principles

- **Correctness over features.** A wrong session boundary is worse than no feature.
- **Minimal footprint.** Target: <1% CPU idle, <50 MB RAM.
- **Local-first always.** No network calls, no telemetry, no remote dependencies.
- **Idempotent ETL.** Re-running on the same data produces the same result. No duplicates, ever.
- **Read-only on screenpipe.** Meridian never writes to screenpipe's database.

## Deep dives

- [ETL Pipeline →](/architecture/etl-pipeline)
- [Database Schema →](/architecture/database)
