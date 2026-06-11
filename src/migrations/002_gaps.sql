-- meridian — normalises screenpipe activity into structured app sessions
-- https://github.com/meridiona/meridian

CREATE TABLE IF NOT EXISTS gaps (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    started_at  TEXT    NOT NULL,
    ended_at    TEXT    NOT NULL,
    duration_s  INTEGER NOT NULL,
    kind        TEXT    NOT NULL CHECK(kind IN ('user_idle', 'system_sleep')),
    etl_run_id  INTEGER NOT NULL,
    FOREIGN KEY (etl_run_id) REFERENCES etl_runs(id)
);

CREATE INDEX IF NOT EXISTS idx_gaps_started_at ON gaps(started_at);
CREATE INDEX IF NOT EXISTS idx_gaps_kind       ON gaps(kind);

-- Track how many screenpipe idle frames fell within each app session.
-- Enables UI to show "you were at the computer but not actively working".
ALTER TABLE app_sessions ADD COLUMN idle_frame_count INTEGER NOT NULL DEFAULT 0;

-- Same counter on the in-progress active_session row.
ALTER TABLE active_session ADD COLUMN idle_frame_count INTEGER NOT NULL DEFAULT 0;
