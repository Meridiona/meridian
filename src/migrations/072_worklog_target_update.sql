-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
-- Per-ticket worklog body. When one day-task's work advances several tickets, each
-- matched ticket now gets its OWN generated status update rather than the same
-- shared body posted verbatim to all of them. That per-ticket update is stored here.
--
-- NULL means "use the draft-level update" (day_task_worklogs.update_json): the
-- propose branch (a new ticket gets the one workstream update), rows written before
-- this column existed, and any match the model did not hand a per-ticket update.
-- The poster falls back accordingly, so an un-migrated or partial row still posts.
ALTER TABLE day_task_worklog_targets ADD COLUMN update_json TEXT;
