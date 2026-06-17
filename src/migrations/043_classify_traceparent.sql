-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- Store the W3C traceparent of each session's CLASSIFICATION trace (the
-- `classify_session` span tree emitted by the MLX server), so a downstream
-- worklog_draft trace can backtrack from a worklog to exactly how each
-- contributing session was classified — via OTel span Links in OpenObserve.
--
-- This is distinct from `app_sessions.traceparent` (migration 010), which holds
-- the session-FORMATION trace written by the ETL when it closes the session.
-- Together the two columns give a worklog full lineage: formation -> classify.
--
-- Format is the 55-byte W3C string: "00-{trace_id}-{span_id}-{flags}".
-- NULL until the session is classified by the MLX path.

ALTER TABLE app_sessions ADD COLUMN classify_traceparent TEXT;
