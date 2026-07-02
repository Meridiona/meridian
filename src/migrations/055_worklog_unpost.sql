-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- Editing/re-matching a worklog that's already been POSTED to the tracker
-- needs to delete the stale entry there before the corrected content can be
-- reposted, so a human never sees two comments/worklogs for the same window.
-- edit_worklog/rematch_worklog stash the posted entry's (provider, id) here
-- and clear posted_worklog_id/posted_at, pulling the row back to 'drafted';
-- the daemon's unpost sweep (src/pm_worklog/post.rs) then deletes it via the
-- provider API and clears these two columns. Nullable: NULL means "nothing
-- pending cleanup."

ALTER TABLE pm_worklogs ADD COLUMN unpost_provider TEXT;
ALTER TABLE pm_worklogs ADD COLUMN unpost_worklog_id TEXT;
