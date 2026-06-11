-- meridian — normalises screenpipe activity into structured app sessions
--
-- Per-day chunking for Claude / Codex rows.
--
-- A single coding-agent session can span many days. Storing one
-- app_sessions row per session credits all the active time to the day
-- the session STARTED, which breaks the PM-update workflow's window
-- queries (`WHERE started_at BETWEEN ?...`). The fix is to split each
-- session into one row per (uuid, calendar-day-local), each row
-- carrying only that day's slice of active time, turns, and transcript.
--
-- The previous unique index `uq_app_sessions_claude_chunk` was keyed on
-- (claude_session_uuid, started_at) — that allowed re-registration after
-- a resume but didn't enforce per-day slicing. The new index keys on
-- (claude_session_uuid, day_utc) — the indexer's UPSERT now writes one
-- row per local-day slice.
--
-- `day_utc` is misleadingly named for historical reasons; it actually
-- holds the user-local calendar day (YYYY-MM-DD) because that's what
-- PM updates and dashboards reason about. The column name is kept for
-- compatibility with the existing app_sessions schema vocabulary.

ALTER TABLE app_sessions ADD COLUMN day_utc TEXT;

-- Backfill day_utc from started_at for existing rows. This uses the
-- UTC date prefix, NOT the user-local TZ — accurate enough for
-- screen-frame rows (the PM updater queries by `started_at` ranges
-- anyway, not by day_utc).
--
-- For Claude / Codex rows (claude_session_uuid IS NOT NULL) this
-- backfill is a stop-gap: legacy installs that wrote one row per
-- session need to be re-split into per-(uuid, local-day) rows. Run
-- once after migration:
--
--     cd services
--     .venv313/bin/python -m coding_agent_indexer.cli --reseed
--
-- which calls `db.delete_claude_session_rows()` and then `--scan-once`
-- to re-register everything from the JSONLs on disk under the per-day
-- scheme.
UPDATE app_sessions
SET    day_utc = substr(started_at, 1, 10)
WHERE  day_utc IS NULL;

-- Drop the old per-(uuid,started_at) uniqueness — superseded by per-day.
DROP INDEX IF EXISTS uq_app_sessions_claude_chunk;

-- New per-day uniqueness: one row per (Claude session, local day).
CREATE UNIQUE INDEX IF NOT EXISTS uq_app_sessions_claude_day
    ON app_sessions (claude_session_uuid, day_utc)
    WHERE claude_session_uuid IS NOT NULL;
