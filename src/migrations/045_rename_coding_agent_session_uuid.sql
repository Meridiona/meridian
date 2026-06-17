-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

-- Rename claude_session_uuid → coding_agent_session_uuid.
--
-- The column is agent-neutral: it holds the conversation/session UUID of ANY
-- ingested coding agent (Claude Code, Codex, GitHub Copilot, Cursor, …), not
-- just Claude. The old name mislabelled it.
--
-- SQLite 3.25+ RENAME COLUMN auto-rewrites dependent index/trigger/view
-- definitions, so every partial-index `WHERE … IS NOT NULL` clause follows the
-- rename automatically. We still DROP + CREATE the two "claude"-named indexes so
-- their NAMES become neutral too; idx_app_sessions_seal already has a neutral
-- name and only its WHERE clause needs the (automatic) rewrite.

DROP INDEX IF EXISTS idx_app_sessions_claude_uuid;
DROP INDEX IF EXISTS uq_app_sessions_claude_segment;

ALTER TABLE app_sessions RENAME COLUMN claude_session_uuid TO coding_agent_session_uuid;

CREATE INDEX IF NOT EXISTS idx_app_sessions_agent_uuid
    ON app_sessions (coding_agent_session_uuid)
    WHERE coding_agent_session_uuid IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS uq_app_sessions_agent_segment
    ON app_sessions (coding_agent_session_uuid, segment_started_at)
    WHERE coding_agent_session_uuid IS NOT NULL;
