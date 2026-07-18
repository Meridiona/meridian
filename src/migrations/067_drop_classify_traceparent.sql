-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- Drop `app_sessions.classify_traceparent` (added in migration 043 for a
-- planned session-CLASSIFICATION trace link). Nothing ever wrote to it: the
-- Python MLX classify path it was meant to serve was retired in the Rust
-- port and nothing replaced the write. Dead column, no readers, no writers —
-- removed rather than left as unused schema. `app_sessions.traceparent`
-- (migration 010, the session-FORMATION trace) is unaffected and remains the
-- one traceparent column in active use (see `worklog_pipeline/hour.rs`'s
-- `link_session_formation_traces`).

ALTER TABLE app_sessions DROP COLUMN classify_traceparent;
