-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- capture_ui_events — in-process input-recorder output (Gap-2 Bucket 2, slice 3c).
--
-- Mirrors the read-subset of screenpipe's `ui_events` table that meridian
-- consumes: src/db/screenpipe.rs get_signals (clipboard/app_switch) +
-- get_last_ui_event_for_app (click/key/text MAX(timestamp)), and the health
-- coverage cross-check in src/health/capture.rs (app_switch/window_focus app
-- names). The slice-4b repoint of those readers is then a mechanical
-- `FROM ui_events` → `FROM capture_ui_events` change.
--
-- OWNERSHIP IS INVERTED (like capture_frames): WRITTEN BY THE TRAY (the
-- in-process input recorder, via meridian_core::insert_capture_ui_event) and
-- READ BY THE DAEMON. Append-only; rows are never updated after insert.
--
-- Privacy: text_content is populated ONLY for 'clipboard' events (the one place
-- a reader needs it). click/key/text/app_switch/window_focus rows carry no
-- typed text — the daemon uses only their timestamp / app_name.

CREATE TABLE IF NOT EXISTS capture_ui_events (
    -- Monotonic id (AUTOINCREMENT → no rowid reuse after retention pruning).
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    -- RFC3339 UTC, microsecond precision (matches capture_frames).
    timestamp    TEXT NOT NULL,
    -- 'click' | 'key' | 'text' | 'app_switch' | 'window_focus' | 'clipboard'.
    event_type   TEXT NOT NULL,
    -- App the event belongs to (for app_switch: the activated app).
    app_name     TEXT,
    -- Clipboard text preview (truncated + password/PII-filtered upstream).
    -- NULL for every non-clipboard event type.
    text_content TEXT
);

-- get_signals scans a timestamp window (clipboard/app_switch).
CREATE INDEX IF NOT EXISTS idx_capture_ui_events_timestamp ON capture_ui_events(timestamp);
-- get_last_ui_event_for_app filters app_name + a timestamp range.
CREATE INDEX IF NOT EXISTS idx_capture_ui_events_app_ts ON capture_ui_events(app_name, timestamp);
