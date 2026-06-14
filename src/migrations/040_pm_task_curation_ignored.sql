-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

-- Per-issue "ignore" for board hygiene. A dev can dismiss an OPTIONAL hygiene
-- defect (no labels, no epic, no priority, …) on a ticket so it stops being
-- surfaced — must-fix defects (due date / description / title) cannot be ignored.
-- Stored as a JSON array of reason codes the dev chose to ignore for this ticket.
-- The triage UPSERT preserves this column (only bucket/reasons/triaged_at are
-- rewritten each pass), so an ignore survives re-triage.

ALTER TABLE pm_task_curation ADD COLUMN ignored_codes TEXT NOT NULL DEFAULT '[]';
