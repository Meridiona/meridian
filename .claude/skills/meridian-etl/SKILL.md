---
name: meridian-etl
description: "Debug and work with Meridian's ETL pipeline. Covers session boundary detection, cursor management, DB queries, and common failure modes."
allowed-tools: Bash, Read, Edit, Grep
---

# Meridian ETL Skill

## How the ETL Works

```
screenpipe.db (read-only)
       │
       ▼
 runner.rs  ← src/etl/runner.rs
 polls every POLL_INTERVAL_SECS
       │
       ├─ get_frames_since(cursor)      ← src/db/screenpipe.rs
       │   returns new frames since last processed frame_id
       │
       ├─ detect_boundaries()
       │   splits frames into blocks by focused_app change
       │
       ├─ extract_block_context()       ← src/etl/extractor.rs
       │   OCR samples, window titles, audio snippets, signals
       │
       ├─ upsert active_session         ← src/db/meridian.rs
       │   (current open block, updated each poll)
       │
       └─ insert completed app_sessions
           (previous blocks, now closed)
```

## Key Concepts

| Concept | What it is |
|---------|-----------|
| **cursor** | `last_processed_frame_id` — marks where ETL left off |
| **app-switch boundary** | frame where `focused_app` differs from the previous frame |
| **active_session** | the currently-open, in-progress session (one row, upserted) |
| **app_sessions** | completed, closed sessions with final timestamps |

## Running with Verbose Logging
```bash
RUST_LOG=debug ./target/release/meridian
RUST_LOG=meridian=trace ./target/release/meridian   # trace-level for ETL internals
```

## Useful Debug Queries

```bash
# Open the meridian DB
sqlite3 ~/.meridian/meridian.db

# Top apps by time today
SELECT app_name, ROUND(SUM(duration_s)/60.0,1) AS minutes, COUNT(*) AS sessions
FROM app_sessions
WHERE started_at >= date('now')
GROUP BY app_name ORDER BY minutes DESC LIMIT 10;

# Check cursor (last processed frame)
SELECT * FROM etl_state;

# Inspect active session
SELECT * FROM active_session;

# Find gaps between sessions (potential missed frames)
SELECT
  a.ended_at,
  b.started_at,
  ROUND((julianday(b.started_at) - julianday(a.ended_at)) * 86400) AS gap_secs
FROM app_sessions a
JOIN app_sessions b ON b.rowid = a.rowid + 1
WHERE gap_secs > 120
ORDER BY gap_secs DESC LIMIT 20;

# Sessions with zero duration (regression check)
SELECT * FROM app_sessions WHERE duration_s = 0;

# Count sessions per day
SELECT date(started_at) AS day, COUNT(*) AS n, ROUND(SUM(duration_s)/3600.0,2) AS hours
FROM app_sessions
GROUP BY day ORDER BY day DESC;
```

## Common Issues

### Zero-duration sessions
Single-frame sessions returned `duration_s = 0`. Fixed in commit `317ceb2` (Option D).
Verify with: `SELECT * FROM app_sessions WHERE duration_s = 0;`

### Phantom sessions spanning sleep gaps
Machine sleep between two ETL runs created a session covering the sleep period.
Fixed in commit `a8f2280` (sleep gap detection at ETL run boundary).
Check: `SELECT * FROM app_sessions WHERE duration_s > 3600 ORDER BY duration_s DESC;`

### Duplicate sessions
Cursor not advancing correctly caused re-processing. Check cursor value:
```bash
sqlite3 ~/.meridian/meridian.db "SELECT * FROM etl_state;"
```

### Screenpipe DB locked
Meridian must open screenpipe DB with read-only flag. If you see `SQLITE_BUSY`:
```bash
lsof ~/.screenpipe/db.sqlite
```

## Reset and Re-run ETL from Scratch

```bash
# Stop the daemon
pkill meridian

# Delete meridian DB (this resets all sessions and cursor)
rm ~/.meridian/meridian.db

# Restart — ETL will re-process all screenpipe frames from scratch
./target/release/meridian
```

## Running Integration Tests

```bash
cargo test                         # all tests
cargo test integration             # integration tests only
RUST_LOG=debug cargo test -- --nocapture  # with log output
```
