-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- pm-worklog stage (Stage 4). Adds the hour ledger (new) and brings the worklog
-- tables under Rust's DDL ownership. The pm_worklogs / pm_worklog_evidence /
-- pm_worklog_feedback tables may already exist on a live DB (created at runtime
-- by the now-retired Python `init_schema`); every statement here is
-- `IF NOT EXISTS`, so this is a no-op on existing data and only creates what's
-- missing (notably `pm_worklog_hours`).

-- One row per workflow run. UNIQUE (task_key, day_utc, cycle_index) lets a
-- re-run replace a DRAFTED row without double-inserting.
CREATE TABLE IF NOT EXISTS pm_worklogs (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    task_key             TEXT    NOT NULL,
    day_utc              TEXT    NOT NULL,
    cycle_index          INTEGER NOT NULL DEFAULT 0,
    window_start         TEXT    NOT NULL,
    window_end           TEXT    NOT NULL,
    state                TEXT    NOT NULL,
    confidence           REAL    NOT NULL DEFAULT 0.0,
    coverage             REAL    NOT NULL DEFAULT 0.0,
    time_spent_seconds   INTEGER NOT NULL DEFAULT 0,
    payload_json         TEXT    NOT NULL,
    session_id_min       INTEGER,
    session_id_max       INTEGER,
    workflow_run_id      TEXT,
    posted_comment_id    TEXT,
    posted_worklog_id    TEXT,
    created_at           TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    posted_at            TEXT,
    UNIQUE (task_key, day_utc, cycle_index)
);

CREATE INDEX IF NOT EXISTS idx_pm_worklogs_task_day
    ON pm_worklogs (task_key, day_utc);

CREATE INDEX IF NOT EXISTS idx_pm_worklogs_state
    ON pm_worklogs (state);

-- Idempotency backstop: at most one POSTED worklog per (task, window).
CREATE UNIQUE INDEX IF NOT EXISTS uq_pm_worklogs_worklog_window
    ON pm_worklogs (task_key, window_start, window_end)
    WHERE posted_worklog_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS pm_worklog_evidence (
    pm_worklog_id  INTEGER NOT NULL REFERENCES pm_worklogs(id) ON DELETE CASCADE,
    bullet_kind    TEXT    NOT NULL,
    bullet_index   INTEGER NOT NULL,
    session_id     INTEGER NOT NULL REFERENCES app_sessions(id),
    excerpt        TEXT    NOT NULL DEFAULT '',
    PRIMARY KEY (pm_worklog_id, bullet_kind, bullet_index, session_id)
);

CREATE INDEX IF NOT EXISTS idx_pm_worklog_evidence_session
    ON pm_worklog_evidence (session_id);

CREATE TABLE IF NOT EXISTS pm_worklog_feedback (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    pm_worklog_id  INTEGER NOT NULL REFERENCES pm_worklogs(id) ON DELETE CASCADE,
    feedback_kind  TEXT    NOT NULL,
    original_text  TEXT,
    edited_text    TEXT,
    note           TEXT,
    created_at     TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_pm_worklog_feedback_task
    ON pm_worklog_feedback (pm_worklog_id);

-- NEW: the hour ledger. One row per hour the driver has seen. `hour_start` is a
-- globally-unique absolute UTC instant (`+00:00`), so it is the natural key.
-- `status` is 'pending' until the hour is processed, then 'done' (including
-- 0-task hours, so they are never re-scanned).
CREATE TABLE IF NOT EXISTS pm_worklog_hours (
    hour_start    TEXT    NOT NULL PRIMARY KEY,
    day_utc       TEXT    NOT NULL,
    hour_end      TEXT    NOT NULL,
    status        TEXT    NOT NULL DEFAULT 'pending',
    task_count    INTEGER NOT NULL DEFAULT 0,
    processed_at  TEXT
);

CREATE INDEX IF NOT EXISTS idx_pm_worklog_hours_day
    ON pm_worklog_hours (day_utc, status);
