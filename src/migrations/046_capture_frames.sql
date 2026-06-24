-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- capture_frames — in-process capture output (Gap-2 Bucket 2, slice 4a).
--
-- Mirrors the read-subset of screenpipe's `frames` table that meridian's ETL
-- consumes (see src/db/screenpipe.rs: get_frames_since / count_frames_in_window
-- / get_window_titles / get_frame_full_texts), so the slice-4b reader repoint is
-- a mechanical FROM-clause change rather than a reshape.
--
-- OWNERSHIP IS INVERTED vs every other meridian.db table: this one is WRITTEN BY
-- THE TRAY (the in-process capture engine, via meridian_core::insert_capture_frame)
-- and READ BY THE DAEMON's ETL. Append-only; rows are never updated after insert.
--
-- text_source mirrors screenpipe exactly: each row populates exactly ONE of
-- full_text (OCR) / accessibility_text (a11y-tree), and the reader resolves them
-- with COALESCE(full_text, accessibility_text) + COALESCE(text_source, 'ocr').

CREATE TABLE IF NOT EXISTS capture_frames (
    -- Monotonic cursor key. AUTOINCREMENT (not bare rowid) so ids are never
    -- reused after retention pruning — a reused id would make the ETL cursor
    -- skip or reprocess frames.
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    -- RFC3339 UTC, microsecond precision, 'Z' offset (chrono
    -- to_rfc3339_opts(Micros, true)). Constant width so the reader's string
    -- range comparisons (timestamp > ?, BETWEEN ? AND ?) are unambiguous.
    timestamp          TEXT NOT NULL,
    -- Focused app. NULL allowed — the reader already filters NULL/'' itself,
    -- and CapturedFrame.app_name is Option.
    app_name           TEXT,
    window_name        TEXT,
    browser_url        TEXT,
    -- OCR text (text_source='ocr'); NULL when the row is a11y-sourced.
    full_text          TEXT,
    -- a11y-tree text (text_source='accessibility'); NULL when OCR-sourced.
    accessibility_text TEXT,
    -- 'ocr' | 'accessibility'.
    text_source        TEXT,
    -- Reserved: 'idle' etc for gap (user-idle vs system-sleep) classification
    -- via count_frames_in_window. NULL until in-process idle detection lands.
    capture_trigger    TEXT
);

-- Supports the reader's timestamp-window scans (gap classification, audio/signal
-- windows). Primary scans are by id (the cursor), so this is the secondary path.
CREATE INDEX IF NOT EXISTS idx_capture_frames_timestamp ON capture_frames(timestamp);
