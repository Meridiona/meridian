-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- Tier-3 of the worklog pipeline: when an hour of work matches no existing PM
-- task, the matcher drafts a PROPOSED task here for the developer to approve.
-- Drafts only — nothing is ever auto-created in the tracker. On approval the UI
-- creates the real ticket via the existing write-back path and stamps
-- created_task_key + approved_at.
--
-- Idempotent per source hour: re-running the worklog pipeline for the same hour
-- refreshes a still-`proposed` row but never disturbs one already approved or
-- dismissed (mirrors the pm_worklogs draft-immutability rule).

CREATE TABLE IF NOT EXISTS pm_proposed_tasks (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    day_utc          TEXT    NOT NULL,
    source_hour      TEXT    NOT NULL,            -- 'YYYY-MM-DDTHH' the work came from
    title            TEXT    NOT NULL,
    description      TEXT    NOT NULL DEFAULT '',
    reasoning        TEXT    NOT NULL DEFAULT '',
    state            TEXT    NOT NULL DEFAULT 'proposed',  -- proposed | approved | dismissed
    workflow_run_id  TEXT,
    created_task_key TEXT,                          -- set when approved → real ticket key
    created_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    resolved_at      TEXT,                          -- approved/dismissed timestamp
    UNIQUE (day_utc, source_hour)
);

CREATE INDEX IF NOT EXISTS idx_pm_proposed_tasks_day_state
    ON pm_proposed_tasks (day_utc, state);
