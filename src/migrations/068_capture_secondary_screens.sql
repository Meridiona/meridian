-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- capture_secondary_screens — OCR text from monitors OTHER than the one
-- holding the currently-focused window (multi-screen capture, context-only).
--
-- OWNERSHIP IS INVERTED vs every other meridian.db table, same as
-- capture_frames: this one is WRITTEN BY THE TRAY (the in-process capture
-- engine's secondary-monitor sweep, via meridian_core::insert_capture_secondary_screen)
-- and READ BY THE DAEMON's ETL (src/db/screenpipe.rs::get_secondary_screen_events).
-- Append-only; rows are never updated after insert.
--
-- Unlike capture_frames, rows here never define a session boundary — the ETL
-- state machine in src/etl/runner.rs never reads this table. It is folded in
-- at session-close/upsert time as extra context on whichever app_sessions /
-- active_session row is open (see BlockContext::secondary_screens), attached
-- to the session the user was actually working in, not tracked as separate
-- activity of its own.

CREATE TABLE IF NOT EXISTS capture_secondary_screens (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    -- RFC3339 UTC, microsecond precision — matches capture_frames.timestamp.
    timestamp    TEXT NOT NULL,
    -- Display name (SafeMonitor::name(), falling back to stable_id() when empty).
    monitor_name TEXT NOT NULL,
    -- Foreground app on that monitor's topmost window, if known.
    app_name     TEXT,
    window_name  TEXT,
    -- OCR text of that window. NULL/empty rows are never inserted (writer skips them).
    text         TEXT
);

-- get_secondary_screen_events scans a timestamp window, mirroring capture_frames' index.
CREATE INDEX IF NOT EXISTS idx_capture_secondary_screens_timestamp ON capture_secondary_screens(timestamp);
