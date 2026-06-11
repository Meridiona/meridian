-- meridian — normalises screenpipe activity into structured app sessions
--
-- Add `claude_session_uuid` to `app_sessions` so the coding_agent_indexer
-- can register ended Claude Code / Codex sessions as first-class
-- app_sessions rows pointing at the source JSONL on disk.
--
-- The partial unique index keeps NULL rows (normal screen-frame
-- sessions) unconstrained while preventing double-register of the same
-- (jsonl uuid, started_at) chunk. `started_at` is part of the key so
-- a resumed session (same JSONL, new activity after a long break) can
-- register multiple rows — one per work chunk.

ALTER TABLE app_sessions ADD COLUMN claude_session_uuid TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS uq_app_sessions_claude_chunk
    ON app_sessions (claude_session_uuid, started_at)
    WHERE claude_session_uuid IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_app_sessions_claude_uuid
    ON app_sessions (claude_session_uuid)
    WHERE claude_session_uuid IS NOT NULL;
