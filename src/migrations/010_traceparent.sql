-- meridian — normalises screenpipe activity into structured app sessions

-- W3C trace-context propagation across process boundaries.
-- Rust ETL writes the active span's `traceparent` when it closes a session;
-- Python agents read it back and use it as the parent context for downstream
-- spans (tagger -> stage2 -> stage3 -> jira_keeper).  This stitches a single
-- session's journey into one trace tree in OpenObserve.
--
-- Format is the 55-byte W3C string: "00-{trace_id}-{span_id}-{flags}".
-- NULL is allowed for rows written before the column existed.

ALTER TABLE app_sessions ADD COLUMN traceparent TEXT;
ALTER TABLE agent_runs   ADD COLUMN traceparent TEXT;
ALTER TABLE ticket_links ADD COLUMN traceparent TEXT;
