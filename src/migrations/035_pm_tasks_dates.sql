-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
ALTER TABLE pm_tasks ADD COLUMN due_date TEXT;
ALTER TABLE pm_tasks ADD COLUMN start_date TEXT;
