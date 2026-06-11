-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

-- Add hierarchy and context columns to pm_tasks so the classifier can
-- reason about epic context, sprint membership, and task ownership.
ALTER TABLE pm_tasks ADD COLUMN parent_key    TEXT;
ALTER TABLE pm_tasks ADD COLUMN epic_title    TEXT;
ALTER TABLE pm_tasks ADD COLUMN sprint_name   TEXT;
ALTER TABLE pm_tasks ADD COLUMN assignee_name TEXT;
