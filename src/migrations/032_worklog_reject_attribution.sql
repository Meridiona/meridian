-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- Worklog-reject attribution capture. When a reviewer dismisses a worklog they
-- can now say where the time *should* have gone: a different ticket, or
-- untracked. That answer is the ground-truth attribution label the classifier
-- got wrong — the single highest-value eval signal — so we store it
-- structurally on the existing pm_worklog_feedback row (written per review
-- action since migration-time route changes) rather than burying it in `note`.
--
--   corrected_task_key       the ticket the time *should* have gone to, or NULL
--   corrected_to_untracked   1 when the reviewer said "untracked / personal"
--
-- Both stay NULL/0 for edit / approve / unapprove rows and for plain dismissals
-- where the reviewer did not supply a target.
ALTER TABLE pm_worklog_feedback ADD COLUMN corrected_task_key TEXT;
ALTER TABLE pm_worklog_feedback ADD COLUMN corrected_to_untracked INTEGER NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_pm_worklog_feedback_corrected
    ON pm_worklog_feedback (corrected_task_key)
    WHERE corrected_task_key IS NOT NULL;
