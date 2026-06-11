-- meridian — normalises screenpipe activity into structured app sessions
--
-- Multi-provider worklog support. A worklog must remember WHICH tracker it
-- belongs to so the approved-poster can route it to the right backend (Jira's
-- native worklog API, a Linear comment, or a GitHub issue comment) even after
-- the originating pm_tasks row has been pruned (a closed ticket is removed on
-- the next sync). We snapshot the provider onto the worklog row at draft time.
--
-- Default 'jira' keeps every pre-existing row valid — before this migration the
-- poster only ever spoke to Jira, so that is the correct historical value.

ALTER TABLE pm_worklogs ADD COLUMN provider TEXT NOT NULL DEFAULT 'jira';

CREATE INDEX IF NOT EXISTS idx_pm_worklogs_provider ON pm_worklogs (provider);
