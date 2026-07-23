-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
-- User corrections to the AI-inferred day-tasks (timeline workstreams): DISMISS a
-- task, or MERGE one task into another.
--
-- WHY A SEPARATE TABLE (not a flag on day_tasks): `day_tasks` is derived, roll-
-- forward data the worklog fold rewrites every hour (src/worklog_pipeline/task_db.rs
-- `replace_day_tasks` UPSERTs on (day_local, task_id) with `status = excluded.status`
-- and PRUNES any task_id the fold no longer names). A `status = 'dismissed'` flag or
-- a hard DELETE would be clobbered / re-derived on the very next fold. This table is
-- the DURABLE record of the user's intent; `day_task_corrections::reconcile` reads it
-- and re-materializes the correction into `day_tasks` after every fold AND on each
-- command, so the correction sticks no matter how many times the day is rewritten.
CREATE TABLE day_task_corrections (
    day_local   TEXT NOT NULL,          -- 'YYYY-MM-DD' local day
    task_id     TEXT NOT NULL,          -- the corrected task ('T1','T2',…)
    action      TEXT NOT NULL,          -- 'dismissed' | 'merged'
    merged_into TEXT,                   -- for 'merged': the surviving task_id it folds into
    created_at  TEXT NOT NULL,          -- RFC3339 UTC
    PRIMARY KEY (day_local, task_id)
);
