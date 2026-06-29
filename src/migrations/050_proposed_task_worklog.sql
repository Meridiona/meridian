-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- Carry a DRAFTED worklog alongside each tier-3 proposed task so the approval
-- surface can show the proposed ticket AND an editable worklog together (the
-- WorklogsView renders proposals inline, as a continuation of the day's real
-- worklogs). On approval the tray creates the real ticket via the provider
-- write-back path, then posts this worklog against the new task_key.
--
-- All columns are nullable / defaulted so existing `proposed` rows (written
-- before this migration) keep working; the worklog pipeline backfills them on
-- its next run for the same source hour (the upsert is state-guarded).

ALTER TABLE pm_proposed_tasks ADD COLUMN worklog_payload_json TEXT;
ALTER TABLE pm_proposed_tasks ADD COLUMN time_spent_seconds   INTEGER NOT NULL DEFAULT 3600;
ALTER TABLE pm_proposed_tasks ADD COLUMN confidence           REAL    NOT NULL DEFAULT 0.0;
ALTER TABLE pm_proposed_tasks ADD COLUMN window_start         TEXT;
ALTER TABLE pm_proposed_tasks ADD COLUMN window_end           TEXT;
-- The pm_worklogs row created when this proposal is approved (so the existing
-- ~60s approved-sweep posts it). NULL until approved.
ALTER TABLE pm_proposed_tasks ADD COLUMN created_worklog_id   INTEGER;
