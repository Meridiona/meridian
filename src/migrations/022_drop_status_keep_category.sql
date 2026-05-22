-- meridian — normalises screenpipe activity into structured app sessions

-- Remove redundant status column; status_category is the normalized form used for all logic.
-- status_category values: 'todo', 'in_progress', 'done'
-- Note: SQLite doesn't support DROP COLUMN IF EXISTS, so we use a workaround.
-- Create a new table without the status column, copy data, and swap names.
-- pm_task_embeddings has a FK to pm_tasks so it must be dropped and recreated.

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

-- Preserve embeddings before dropping the parent table.
CREATE TABLE pm_task_embeddings_save AS SELECT * FROM pm_task_embeddings;
DROP TABLE pm_task_embeddings;

DROP TABLE pm_tasks;
ALTER TABLE pm_tasks_new RENAME TO pm_tasks;

CREATE INDEX IF NOT EXISTS idx_pm_tasks_provider   ON pm_tasks (provider);
CREATE INDEX IF NOT EXISTS idx_pm_tasks_expires_at ON pm_tasks (expires_at);

-- Recreate pm_task_embeddings with FK pointing at the renamed table.
CREATE TABLE pm_task_embeddings (
    task_key       TEXT    NOT NULL REFERENCES pm_tasks(task_key),
    model          TEXT    NOT NULL,
    dim            INTEGER NOT NULL,
    embedding      BLOB    NOT NULL,
    text_hash      TEXT    NOT NULL,
    pm_updated_at  TEXT    NOT NULL,
    expected_dims  TEXT,
    created_at     TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    PRIMARY KEY (task_key, model)
);

CREATE INDEX IF NOT EXISTS idx_pm_task_embeddings_model ON pm_task_embeddings (model);

INSERT INTO pm_task_embeddings SELECT * FROM pm_task_embeddings_save;
DROP TABLE pm_task_embeddings_save;
