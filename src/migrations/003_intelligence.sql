-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

-- Cached PM tasks fetched from Jira / GitHub / Linear.
-- Re-fetched periodically; expires_at controls staleness.
CREATE TABLE IF NOT EXISTS pm_tasks (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    task_key         TEXT    NOT NULL UNIQUE,
    provider         TEXT    NOT NULL,          -- 'jira' | 'github' | 'linear'
    title            TEXT    NOT NULL,
    description_text TEXT    NOT NULL DEFAULT '',
    status           TEXT    NOT NULL DEFAULT '',
    status_category  TEXT    NOT NULL DEFAULT 'todo',
    issue_type       TEXT    NOT NULL DEFAULT '',
    project_key      TEXT    NOT NULL DEFAULT '',
    url              TEXT    NOT NULL DEFAULT '',
    updated_at       TEXT    NOT NULL,
    fetched_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    expires_at       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_pm_tasks_provider   ON pm_tasks (provider);
CREATE INDEX IF NOT EXISTS idx_pm_tasks_expires_at ON pm_tasks (expires_at);

-- One row per completed app_session that has been analysed.
-- session_id is UNIQUE — a session maps to exactly one task (or is overhead/unknown).
CREATE TABLE IF NOT EXISTS ticket_links (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id   INTEGER NOT NULL UNIQUE REFERENCES app_sessions(id),
    task_key     TEXT,
    provider     TEXT,
    method       TEXT    NOT NULL DEFAULT 'none',
    confidence   REAL    NOT NULL DEFAULT 0.0,
    session_type TEXT    NOT NULL DEFAULT 'unknown'
                         CHECK (session_type IN ('task', 'overhead', 'unknown')),
    routing      TEXT    NOT NULL DEFAULT 'skip'
                         CHECK (routing IN ('auto', 'queue', 'skip')),
    created_at   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_ticket_links_task_key  ON ticket_links (task_key);
CREATE INDEX IF NOT EXISTS idx_ticket_links_routing   ON ticket_links (routing);
