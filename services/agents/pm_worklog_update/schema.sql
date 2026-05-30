-- meridian — pm_worklog_update tables
--
-- These tables live alongside the rest of meridian.db. They are owned by
-- the Python pm_worklog_update package — Rust does not write to them. When this
-- module is eventually stitched into the daemon, the DDL here will be
-- migrated into src/migrations/02X_pm_worklogs.sql; until then,
-- `db.init_schema()` applies it idempotently.

-- One row per workflow run. Uniqueness over (task_key, day_utc, cycle_index)
-- prevents the daemon from double-posting after a restart. Backfill mode
-- writes rows with cycle_index = -1 so it never collides with live cycles.
CREATE TABLE IF NOT EXISTS pm_worklogs (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    task_key             TEXT    NOT NULL,
    day_utc              TEXT    NOT NULL,                       -- 'YYYY-MM-DD'
    cycle_index          INTEGER NOT NULL DEFAULT 0,             -- nth cycle that day; -1 = backfill
    window_start         TEXT    NOT NULL,                       -- ISO-8601 UTC
    window_end           TEXT    NOT NULL,
    state                TEXT    NOT NULL,                       -- UpdateState enum value
    confidence           REAL    NOT NULL DEFAULT 0.0,
    coverage             REAL    NOT NULL DEFAULT 0.0,
    time_spent_seconds   INTEGER NOT NULL DEFAULT 0,
    payload_json         TEXT    NOT NULL,                       -- serialised JiraUpdate
    session_id_min       INTEGER,
    session_id_max       INTEGER,
    workflow_run_id      TEXT,                                   -- agno workflow run id
    posted_comment_id    TEXT,                                   -- Jira comment id (phase 2)
    posted_worklog_id    TEXT,                                   -- Jira worklog id (phase 1)
    created_at           TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    posted_at            TEXT,
    UNIQUE (task_key, day_utc, cycle_index)
);

-- NOTE: the worklog idempotency index `uq_pm_worklogs_worklog_window`
-- is created in db.init_schema() so it can run AFTER the additive
-- column migration adds posted_worklog_id to legacy tables.

CREATE INDEX IF NOT EXISTS idx_pm_worklogs_task_day
    ON pm_worklogs (task_key, day_utc);

CREATE INDEX IF NOT EXISTS idx_pm_worklogs_state
    ON pm_worklogs (state);

-- One row per bullet — flat shape lets the UI render an evidence panel
-- without parsing the payload JSON.
CREATE TABLE IF NOT EXISTS pm_worklog_evidence (
    pm_worklog_id  INTEGER NOT NULL REFERENCES pm_worklogs(id) ON DELETE CASCADE,
    bullet_kind   TEXT    NOT NULL,                              -- 'shipped'|'in_progress'|'blocker'|'decision'
    bullet_index  INTEGER NOT NULL,
    session_id    INTEGER NOT NULL REFERENCES app_sessions(id),
    excerpt       TEXT    NOT NULL DEFAULT '',
    PRIMARY KEY (pm_worklog_id, bullet_kind, bullet_index, session_id)
);

CREATE INDEX IF NOT EXISTS idx_pm_worklog_evidence_session
    ON pm_worklog_evidence (session_id);

-- Self-learning signal: every admin edit / rejection becomes a feedback
-- row. The Synth agent's pre_hook injects recent feedback as few-shot
-- guidance, so the system improves without us manually retuning prompts.
CREATE TABLE IF NOT EXISTS pm_worklog_feedback (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    pm_worklog_id   INTEGER NOT NULL REFERENCES pm_worklogs(id) ON DELETE CASCADE,
    feedback_kind  TEXT    NOT NULL,                             -- 'edit'|'reject'|'approve'
    original_text  TEXT,                                         -- the synth-generated comment
    edited_text    TEXT,                                         -- the human-edited final, if any
    note           TEXT,                                         -- optional admin note
    created_at     TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_pm_worklog_feedback_task
    ON pm_worklog_feedback (pm_worklog_id);
