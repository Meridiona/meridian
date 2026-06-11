-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
-- https://github.com/meridiona/meridian

-- Completed app sessions (append-only, never updated after insert)
CREATE TABLE IF NOT EXISTS app_sessions (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    app_name      TEXT    NOT NULL,
    started_at    TEXT    NOT NULL,  -- ISO8601 UTC
    ended_at      TEXT    NOT NULL,  -- ISO8601 UTC
    duration_s    INTEGER NOT NULL,  -- ended_at - started_at in seconds
    window_titles TEXT    NOT NULL,  -- JSON: [{"title":"auth.rs","count":12}, ...]
    ocr_samples   TEXT,              -- JSON: [{"text":"fn validate","window":"auth.rs","ts":"..."}, ...]
    elements_samples TEXT,           -- JSON: same shape as ocr_samples but from accessibility tree
    audio_snippets TEXT,             -- JSON: [{"text":"fix the null check","ts":"...","speaker_id":1}, ...]
    signals       TEXT,              -- JSON: [{"type":"clipboard","value":"feat/KAN-7","ts":"..."}, ...]
    min_frame_id  INTEGER NOT NULL,  -- first screenpipe frame_id in this block
    max_frame_id  INTEGER NOT NULL,  -- last screenpipe frame_id in this block
    frame_count   INTEGER NOT NULL,
    etl_run_id    INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_app_sessions_started_at  ON app_sessions (started_at);
CREATE INDEX IF NOT EXISTS idx_app_sessions_app_name    ON app_sessions (app_name);
CREATE INDEX IF NOT EXISTS idx_app_sessions_etl_run_id  ON app_sessions (etl_run_id);

-- The currently open/active block (single row, upserted every 60s)
CREATE TABLE IF NOT EXISTS active_session (
    id               INTEGER PRIMARY KEY CHECK (id = 1),
    app_name         TEXT    NOT NULL,
    started_at       TEXT    NOT NULL,
    last_seen_at     TEXT    NOT NULL,  -- updated every 60s
    window_titles    TEXT    NOT NULL,  -- JSON, grows as new windows appear
    ocr_samples      TEXT,
    elements_samples TEXT,
    audio_snippets   TEXT,
    signals          TEXT,
    min_frame_id     INTEGER NOT NULL,
    max_frame_id     INTEGER NOT NULL,  -- updated every 60s
    frame_count      INTEGER NOT NULL
);

-- ETL audit log
CREATE TABLE IF NOT EXISTS etl_runs (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    started_at      TEXT    NOT NULL,
    completed_at    TEXT,
    from_frame_id   INTEGER NOT NULL,   -- cursor start
    to_frame_id     INTEGER NOT NULL,   -- cursor end
    sessions_closed INTEGER DEFAULT 0,
    status          TEXT    NOT NULL DEFAULT 'running',  -- 'running'|'success'|'failed'|'skipped'
    error           TEXT
);

-- Cursor (single row)
CREATE TABLE IF NOT EXISTS etl_cursor (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    last_frame_id INTEGER NOT NULL DEFAULT 0,
    last_run_at   TEXT,
    last_run_id   INTEGER
);
