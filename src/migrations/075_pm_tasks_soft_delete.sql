-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- "Delete" a personal (provider = 'local') task the user created by mistake,
-- without actually removing the row (see meridian-core/src/readers/task_create.rs
-- for why keys are never reused). Setting `deleted_at` hides it from every place
-- a task is listed (the Tasks board, the day's plan, the worklog matcher's
-- candidate set); NULL means live. Never set for a tracker-synced row - that
-- ticket isn't Meridian's to hide.
ALTER TABLE pm_tasks ADD COLUMN deleted_at TEXT;
