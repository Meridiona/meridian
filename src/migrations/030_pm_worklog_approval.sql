-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- pm-worklog UI approval flow. Worklogs are now never auto-posted: the daemon
-- only ever DRAFTS them, and a human approves (and optionally edits) each one in
-- the dashboard. The `state` column is free text, so the new 'approved' state
-- needs no DDL — these columns just carry the approval/edit audit trail and the
-- last post error so the UI can show why a post is stuck.
--
-- Flow:  drafted ──(UI edit)──▶ drafted ──(UI approve)──▶ approved
--                                              │ daemon sweep
--                                              ▼
--                                           posted   (or stays 'approved' with
--                                                     last_post_error on failure)

ALTER TABLE pm_worklogs ADD COLUMN approved_at        TEXT;
ALTER TABLE pm_worklogs ADD COLUMN edited_at          TEXT;
ALTER TABLE pm_worklogs ADD COLUMN last_post_error    TEXT;
ALTER TABLE pm_worklogs ADD COLUMN post_attempt_count INTEGER NOT NULL DEFAULT 0;
