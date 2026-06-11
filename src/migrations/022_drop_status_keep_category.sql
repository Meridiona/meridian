-- meridian — normalises screenpipe activity into structured app sessions

-- Remove redundant status column; status_category is the normalized form used for all logic.
-- status_category values: 'todo', 'in_progress', 'done'
-- Note: SQLite doesn't support DROP COLUMN IF EXISTS, so we use a workaround.
-- Create a new table without the status column, copy data, and swap names.

CREATE TABLE pm_tasks_new (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    task_key         TEXT    NOT NULL UNIQUE,
    provider         TEXT    NOT NULL,
    title            TEXT    NOT NULL,
    description_text TEXT    NOT NULL DEFAULT '',
    status_category  TEXT    NOT NULL DEFAULT 'todo',
    issue_type       TEXT    NOT NULL DEFAULT '',
    project_key      TEXT    NOT NULL DEFAULT '',
    url              TEXT    NOT NULL DEFAULT '',
    updated_at       TEXT    NOT NULL,
    fetched_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    expires_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    parent_key       TEXT,
    epic_title       TEXT,
    sprint_name      TEXT,
    assignee_name    TEXT
);

INSERT INTO pm_tasks_new
SELECT id, task_key, provider, title, description_text, status_category,
       issue_type, project_key, url, updated_at, fetched_at, expires_at,
       parent_key, epic_title, sprint_name, assignee_name
FROM pm_tasks;

DROP TABLE pm_tasks;
ALTER TABLE pm_tasks_new RENAME TO pm_tasks;

CREATE INDEX IF NOT EXISTS idx_pm_tasks_provider   ON pm_tasks (provider);
CREATE INDEX IF NOT EXISTS idx_pm_tasks_expires_at ON pm_tasks (expires_at);
