-- meridian — normalises screenpipe activity into structured app sessions
-- Remove ocr_samples and elements_samples via table recreation.
-- SQLite has no DROP COLUMN IF EXISTS; this approach is safe whether the
-- columns still exist (fresh DB from 001_initial) or were already removed
-- (databases that ran the earlier 007_remove_ocr_elements migration).

-- ── app_sessions ──────────────────────────────────────────────────────────────

CREATE TABLE app_sessions_new (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    app_name             TEXT    NOT NULL,
    started_at           TEXT    NOT NULL,
    ended_at             TEXT    NOT NULL,
    duration_s           INTEGER NOT NULL,
    window_titles        TEXT    NOT NULL,
    audio_snippets       TEXT,
    signals              TEXT,
    min_frame_id         INTEGER NOT NULL,
    max_frame_id         INTEGER NOT NULL,
    frame_count          INTEGER NOT NULL,
    etl_run_id           INTEGER NOT NULL,
    idle_frame_count     INTEGER NOT NULL DEFAULT 0,
    category             TEXT    NOT NULL DEFAULT 'idle_personal',
    confidence           REAL    NOT NULL DEFAULT 0.0,
    category_method      TEXT    NOT NULL DEFAULT 'rule_based',
    traceparent          TEXT,
    session_text         TEXT,
    category_explanation TEXT
);

INSERT INTO app_sessions_new
    SELECT id, app_name, started_at, ended_at, duration_s, window_titles,
           audio_snippets, signals, min_frame_id, max_frame_id, frame_count,
           etl_run_id, idle_frame_count, category, confidence, category_method,
           traceparent, session_text, category_explanation
    FROM app_sessions;

DROP TABLE app_sessions;
ALTER TABLE app_sessions_new RENAME TO app_sessions;

CREATE INDEX IF NOT EXISTS idx_app_sessions_started_at ON app_sessions (started_at);
CREATE INDEX IF NOT EXISTS idx_app_sessions_app_name   ON app_sessions (app_name);
CREATE INDEX IF NOT EXISTS idx_app_sessions_etl_run_id ON app_sessions (etl_run_id);

-- ── active_session ────────────────────────────────────────────────────────────

CREATE TABLE active_session_new (
    id               INTEGER PRIMARY KEY CHECK (id = 1),
    app_name         TEXT    NOT NULL,
    started_at       TEXT    NOT NULL,
    last_seen_at     TEXT    NOT NULL,
    window_titles    TEXT    NOT NULL,
    audio_snippets   TEXT,
    signals          TEXT,
    min_frame_id     INTEGER NOT NULL,
    max_frame_id     INTEGER NOT NULL,
    frame_count      INTEGER NOT NULL,
    idle_frame_count INTEGER NOT NULL DEFAULT 0,
    category         TEXT    NOT NULL DEFAULT 'idle_personal',
    confidence       REAL    NOT NULL DEFAULT 0.0,
    session_text     TEXT
);

INSERT INTO active_session_new
    SELECT id, app_name, started_at, last_seen_at, window_titles,
           audio_snippets, signals, min_frame_id, max_frame_id, frame_count,
           idle_frame_count, category, confidence, session_text
    FROM active_session;

DROP TABLE active_session;
ALTER TABLE active_session_new RENAME TO active_session;
