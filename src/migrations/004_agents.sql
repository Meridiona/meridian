-- meridian — normalises screenpipe activity into structured app sessions

-- ---------------------------------------------------------------------------
-- Schema for the meridian-agents Python service.
-- The Rust daemon owns all DDL; the Python service only does SELECT/INSERT/
-- UPDATE on the tables defined here. Adding new columns or tables for the
-- agent service should land in a future numbered migration.
-- ---------------------------------------------------------------------------

-- Audit log — one row per orchestrator tick. Mirrors etl_runs.
CREATE TABLE IF NOT EXISTS agent_runs (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    started_at          TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    finished_at         TEXT,
    status              TEXT    NOT NULL DEFAULT 'running'
                                CHECK (status IN ('running', 'success', 'failed', 'aborted')),
    error               TEXT,
    sessions_processed  INTEGER NOT NULL DEFAULT 0,
    summaries_written   INTEGER NOT NULL DEFAULT 0,
    links_written       INTEGER NOT NULL DEFAULT 0,
    dispatches_queued   INTEGER NOT NULL DEFAULT 0,
    dispatches_sent     INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_agent_runs_status     ON agent_runs (status);
CREATE INDEX IF NOT EXISTS idx_agent_runs_started_at ON agent_runs (started_at);

-- Single-row cursor — highest app_sessions.id already analysed.
-- Mirrors etl_cursor.
CREATE TABLE IF NOT EXISTS agent_cursor (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    last_session_id INTEGER NOT NULL DEFAULT 0,
    updated_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

INSERT OR IGNORE INTO agent_cursor (id, last_session_id) VALUES (1, 0);

-- Pending external write-backs (Jira worklog, GitHub comment, Linear update,
-- or LogSink no-op). Drained by the orchestrator each tick.
CREATE TABLE IF NOT EXISTS dispatch_queue (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id    INTEGER NOT NULL REFERENCES app_sessions(id),
    task_key      TEXT    NOT NULL,
    provider      TEXT    NOT NULL CHECK (provider IN ('jira', 'github', 'linear', 'log')),
    payload_json  TEXT    NOT NULL,
    state         TEXT    NOT NULL DEFAULT 'pending'
                          CHECK (state IN ('pending', 'sent', 'failed', 'skipped')),
    attempts      INTEGER NOT NULL DEFAULT 0,
    last_error    TEXT,
    created_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    dispatched_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_dispatch_queue_state      ON dispatch_queue (state);
CREATE INDEX IF NOT EXISTS idx_dispatch_queue_session_id ON dispatch_queue (session_id);

-- LLM-generated summary of an app_session, written by session_summarizer.
-- NULL means "not yet analysed". Stored next to the row to avoid a join.
ALTER TABLE app_sessions ADD COLUMN summary_json TEXT;

-- Deterministic activity category, populated by src/intelligence/categorizer.rs
-- once it is wired into the ETL pipeline. Stored as the lowercase enum name
-- (e.g. 'coding', 'meeting', 'idle_personal'). NULL means "not yet
-- categorised" — meridian-agents tolerates NULL by analysing every session.
ALTER TABLE app_sessions ADD COLUMN activity_kind TEXT;
