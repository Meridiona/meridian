-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

-- ---------------------------------------------------------------------------
-- Schema for the meridian-agents Python service.
--
-- The Rust daemon owns all DDL; the Python service only does SELECT/INSERT/
-- UPDATE on the tables defined here. New tables only — no ALTERs to existing
-- tables. Future agent-side changes should land in a new numbered migration.
--
-- Tables introduced in this migration:
--   - agent_runs           audit log of synthesizer ticks
--   - agent_cursor         single-row high-water mark of analysed sessions
--   - dispatch_queue       queue of external write-backs (Jira / GitHub / Linear / log)
--   - session_summaries    one LLM-derived summary per analysed app_session
--   - context_graph_nodes  persistent knowledge graph (project / task / tool / pattern / ticket)
--   - activity_context     single-row "current focus" snapshot, drives jira-keeper
-- ---------------------------------------------------------------------------


-- One row per orchestrator tick. Mirrors etl_runs in spirit.
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
CREATE TABLE IF NOT EXISTS agent_cursor (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    last_session_id INTEGER NOT NULL DEFAULT 0,
    updated_at      TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

INSERT OR IGNORE INTO agent_cursor (id, last_session_id) VALUES (1, 0);


-- Pending external write-backs (Jira worklog/comment, GitHub comment,
-- Linear update, or the LogSink no-op).
CREATE TABLE IF NOT EXISTS dispatch_queue (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id    INTEGER NOT NULL REFERENCES app_sessions(id),
    agent_run_id  INTEGER NOT NULL REFERENCES agent_runs(id),
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

CREATE INDEX IF NOT EXISTS idx_dispatch_queue_state        ON dispatch_queue (state);
CREATE INDEX IF NOT EXISTS idx_dispatch_queue_session_id   ON dispatch_queue (session_id);
CREATE INDEX IF NOT EXISTS idx_dispatch_queue_agent_run_id ON dispatch_queue (agent_run_id);


-- One LLM-derived summary per analysed session. UNIQUE on session_id —
-- a session is summarised exactly once.
CREATE TABLE IF NOT EXISTS session_summaries (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id    INTEGER NOT NULL UNIQUE REFERENCES app_sessions(id),
    agent_run_id  INTEGER NOT NULL REFERENCES agent_runs(id),
    summary_json  TEXT    NOT NULL,
    generated_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE INDEX IF NOT EXISTS idx_session_summaries_agent_run_id ON session_summaries (agent_run_id);


-- Persistent knowledge graph the synthesizer maintains across runs.
-- Equivalent of hermes' context_map.json. Edges deferred to a future
-- migration if the synthesizer prompt grows that capability.
CREATE TABLE IF NOT EXISTS context_graph_nodes (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    node_id         TEXT    NOT NULL UNIQUE,
    node_type       TEXT    NOT NULL
                            CHECK (node_type IN ('project', 'task', 'tool', 'pattern', 'ticket')),
    label           TEXT    NOT NULL,
    last_seen       TEXT    NOT NULL,
    frequency       INTEGER NOT NULL DEFAULT 1,
    confidence_avg  REAL    NOT NULL DEFAULT 0.7
);

CREATE INDEX IF NOT EXISTS idx_context_graph_nodes_type      ON context_graph_nodes (node_type);
CREATE INDEX IF NOT EXISTS idx_context_graph_nodes_last_seen ON context_graph_nodes (last_seen);


-- Single-row "what is the user working on right now" snapshot. Read by
-- jira-keeper to decide whether to dispatch. Equivalent of hermes'
-- current_context.json.
CREATE TABLE IF NOT EXISTS activity_context (
    id                INTEGER PRIMARY KEY CHECK (id = 1),
    updated_at        TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    active_project    TEXT,
    jira_key          TEXT,
    inferred_task     TEXT    NOT NULL DEFAULT '',
    confidence        REAL    NOT NULL DEFAULT 0.0,
    trigger_jira_sync INTEGER NOT NULL DEFAULT 0 CHECK (trigger_jira_sync IN (0, 1)),
    tags              TEXT,
    last_synced       TEXT
);

INSERT OR IGNORE INTO activity_context (id, inferred_task, confidence)
VALUES (1, '', 0.0);
