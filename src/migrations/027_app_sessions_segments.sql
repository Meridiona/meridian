-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- Segment-based chunking + sealing for Claude / Codex coding-agent rows.
--
-- WHY: the previous scheme (migration 026) keyed coding-agent rows on
-- (claude_session_uuid, day_utc) and live-tracked them — every poll tick
-- re-UPSERTed the same row with fresh ended_at / duration / transcript.
-- That makes the row a moving target: a downstream summary taken mid-flight
-- goes stale the moment more turns append. It also violated the
-- app_sessions invariant ("append-only, never updated after insert").
--
-- WHAT: we now slice a session into SEGMENTS split on >1h idle gaps between
-- messages (config.SEGMENT_GAP_SECONDS), and SEAL each segment once it is
-- settled. A sealed row is immutable forever; downstream only ever reads
-- sealed rows, so it never sees a moving target.
--
--   * `segment_started_at` — the first message timestamp of the segment
--     (ISO-8601 UTC). This is the segment's stable identity. A single
--     session that the user resumes after a >1h break produces a SECOND
--     row with a later segment_started_at, same claude_session_uuid.
--
--   * `sealed_at` — NULL while the segment is live (still being appended
--     to / last activity <1h ago). Set to an ISO-8601 UTC timestamp once
--     the segment settles (last message >1h ago, OR the SessionEnd hook
--     fired). The indexer's UPSERT carries `WHERE sealed_at IS NULL`, so a
--     sealed row is never mutated again.
--
-- The summariser queue is exactly: sealed rows with task_method =
-- 'pending_summariser'. Live (unsealed) rows carry task_method =
-- 'coding_agent_live' — non-NULL either way, so the MLX classifier (which
-- selects `WHERE task_method IS NULL`) skips them in both states.

ALTER TABLE app_sessions ADD COLUMN segment_started_at TEXT;
ALTER TABLE app_sessions ADD COLUMN sealed_at TEXT;

-- Backfill existing coding-agent rows so they remain valid under the new
-- unique key without waiting for a reseed. Each legacy (uuid, day) row
-- becomes a single sealed segment keyed on its own started_at.
UPDATE app_sessions
SET    segment_started_at = started_at
WHERE  claude_session_uuid IS NOT NULL
  AND  segment_started_at IS NULL;

-- Legacy rows are historical (their sessions ended long ago) — mark them
-- sealed so they are immutable and immediately eligible for the summariser.
UPDATE app_sessions
SET    sealed_at = ended_at
WHERE  claude_session_uuid IS NOT NULL
  AND  sealed_at IS NULL;

-- Replace per-day uniqueness with per-segment uniqueness.
DROP INDEX IF EXISTS uq_app_sessions_claude_day;

CREATE UNIQUE INDEX IF NOT EXISTS uq_app_sessions_claude_segment
    ON app_sessions (claude_session_uuid, segment_started_at)
    WHERE claude_session_uuid IS NOT NULL;

-- Partial index for the seal sweep + summariser queue (open / sealed scans).
CREATE INDEX IF NOT EXISTS idx_app_sessions_seal
    ON app_sessions (sealed_at, ended_at)
    WHERE claude_session_uuid IS NOT NULL;

-- After applying this migration, re-split legacy day-rows into true
-- gap-based segments by re-registering from the JSONLs on disk:
--
--     cd services
--     .venv313/bin/python -m coding_agent_indexer.cli --reseed
--
-- Reseed is safe: nothing is summarised yet, so deleting + recreating the
-- indexer-owned rows loses no downstream work.
