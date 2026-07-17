-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- The per-(day, day-task) generated-worklog ledger backing the "Generate worklog"
-- action on a day-task workstream card. Each row is one AI-drafted status update:
-- either a MATCH to an existing pm_task or a PROPOSE of a new one, plus the rich
-- status update to post as a comment. Keyed (day_local, task_id) — the existing
-- pm_worklogs / pm_proposed_tasks ledgers are HOUR-keyed and cannot key this.
--
-- Lifecycle via `state`: 'drafted' (regenerate overwrites) -> 'approved'
-- (approve started) -> 'posted' (comment posted, immutable). `created_task_key`
-- is persisted BEFORE the comment posts so an approve retry never re-creates a
-- ticket. `last_error` records the last failure so a retry-safe row surfaces it.

CREATE TABLE IF NOT EXISTS day_task_worklogs (
    day_local           TEXT NOT NULL,
    task_id             TEXT NOT NULL,
    -- The tracker the update will post to (matched task's provider, or the first
    -- configured provider for a proposal).
    provider            TEXT NOT NULL,
    -- MATCH branch (mutually exclusive with the PROPOSE branch).
    match_task_key      TEXT,
    match_confidence    REAL,
    -- PROPOSE branch (mutually exclusive with the MATCH branch).
    propose_issue_type  TEXT,
    propose_title       TEXT,
    propose_description TEXT,
    -- The status update itself: a denormalised summary for quick reads + the full
    -- {summary,decisions,architecture,status} object as JSON.
    update_summary      TEXT NOT NULL DEFAULT '',
    update_json         TEXT NOT NULL DEFAULT '{}',
    reasoning           TEXT NOT NULL DEFAULT '',
    state               TEXT NOT NULL DEFAULT 'drafted',
    -- Resolved target key: the matched key at draft time, or the created key once
    -- a proposal is created on approve.
    target_key          TEXT,
    created_task_key    TEXT,
    posted_comment_id   TEXT,
    browse_url          TEXT,
    last_error          TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    PRIMARY KEY (day_local, task_id)
);
