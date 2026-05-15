-- meridian — normalises screenpipe activity into structured app sessions

-- Merge ticket_links into app_sessions so every session row carries its task
-- classification inline. task_method IS NULL means not yet classified.
ALTER TABLE app_sessions ADD COLUMN task_key          TEXT;
ALTER TABLE app_sessions ADD COLUMN task_confidence   REAL;
ALTER TABLE app_sessions ADD COLUMN task_routing      TEXT;
ALTER TABLE app_sessions ADD COLUMN task_method       TEXT;
ALTER TABLE app_sessions ADD COLUMN task_reasoning    TEXT;
ALTER TABLE app_sessions ADD COLUMN task_session_type TEXT;

-- Migrate existing ticket_links rows.
UPDATE app_sessions
SET task_key          = tl.task_key,
    task_confidence   = tl.confidence,
    task_routing      = tl.routing,
    task_method       = tl.method,
    task_reasoning    = tl.reasoning,
    task_session_type = tl.session_type
FROM ticket_links tl
WHERE app_sessions.id = tl.session_id;

DROP INDEX IF EXISTS idx_ticket_links_task_key;
DROP INDEX IF EXISTS idx_ticket_links_routing;
DROP TABLE IF EXISTS ticket_links;

CREATE INDEX IF NOT EXISTS idx_app_sessions_task_key
    ON app_sessions (task_key) WHERE task_key IS NOT NULL;
