-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- Give each tier-3 proposed ticket an issue type (Task vs Bug). The worklog
-- pipeline's proposer now decides the type from the captured work — a Bug when
-- the developer was fixing broken/defective behaviour, a Task otherwise — so the
-- approval surface can show it and the provider write-back path can create the
-- ticket with the right issue type (Jira `issuetype`, Azure `$<type>`).
--
-- Defaulted to 'Task' so existing `proposed` rows (written before this migration)
-- keep working; the pipeline backfills the real type on its next run for the same
-- source hour (the upsert is state-guarded).

ALTER TABLE pm_proposed_tasks ADD COLUMN issue_type TEXT NOT NULL DEFAULT 'Task';
