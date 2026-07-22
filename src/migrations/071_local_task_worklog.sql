-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- Where a "Generate worklog" match/post lands when it resolves to a personal
-- (`provider = 'local'`) task instead of a real tracker ticket: there is no
-- external comment thread to post to, so the update text is stored directly on
-- the task row itself, so the task's own detail view can show it.
ALTER TABLE pm_tasks ADD COLUMN local_worklog_text TEXT;
ALTER TABLE pm_tasks ADD COLUMN local_worklog_posted_at TEXT;
