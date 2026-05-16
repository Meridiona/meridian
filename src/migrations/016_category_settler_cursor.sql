-- meridian — normalises screenpipe activity into structured app sessions

-- High-water mark for the Foundation Models category settler. Prevents the
-- settler from re-processing sessions that existed before the daemon started.
ALTER TABLE agent_cursor ADD COLUMN last_settled_session_id INTEGER NOT NULL DEFAULT 0;
