-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

-- High-water mark for the Foundation Models category settler. Prevents the
-- settler from re-processing sessions that existed before the daemon started.
ALTER TABLE agent_cursor ADD COLUMN last_settled_session_id INTEGER NOT NULL DEFAULT 0;
