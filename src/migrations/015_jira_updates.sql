-- meridian — normalises screenpipe activity into structured app sessions
CREATE TABLE IF NOT EXISTS jira_update_log (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    task_key      TEXT    NOT NULL,
    period_start  TEXT    NOT NULL,
    period_end    TEXT    NOT NULL,
    session_count INTEGER DEFAULT 0,
    duration_s    INTEGER DEFAULT 0,
    had_activity  INTEGER DEFAULT 0,
    comment_body  TEXT,
    comment_id    TEXT,
    state         TEXT    DEFAULT 'pending',
    error         TEXT,
    posted_at     TEXT,
    created_at    TEXT    DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_jira_update_log_dedup
    ON jira_update_log(task_key, period_start, period_end);
