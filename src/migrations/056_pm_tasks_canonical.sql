-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

-- Stage 3 of the CDM migration: bring `pm_tasks` up to the CanonicalTask shape
-- (meridian-core `canonical_task`). ADDITIVE ONLY — every column is nullable, so
-- existing rows are untouched and older code keeps working. The columns are
-- populated by the per-provider adapters once ingestion is cut over to them
-- (Stage 3b); this migration lands the schema + a best-effort backfill so the
-- read path is ready first.
--
-- Column ↔ CanonicalTask field:
--   canonical_id    -> canonical_id     "{provider}:{provider_id}", deterministic
--   status_category -> status_category  canonical 6-value lifecycle phase
--                                       (backlog/todo/in_progress/in_review/done/cancelled)
--   raw_payload     -> raw_payload      verbatim tracker JSON (lossless escape hatch)
--   reporter_name   -> reporter         creator/reporter display name
--   completed_at    -> completed_at     ISO-8601 when it reached a terminal state
--   ancestor_path   -> ancestor_path    JSON array of canonical ids, root-first
--   project_ids     -> project_ids      JSON array of canonical ids
--
-- (`issue_type` already carries the verbatim type = CanonicalTask.kind_raw;
--  `is_terminal` stays as the fast terminal flag, now derivable from
--  status_category once the adapter populates it.)

ALTER TABLE pm_tasks ADD COLUMN canonical_id    TEXT;
ALTER TABLE pm_tasks ADD COLUMN status_category TEXT;
ALTER TABLE pm_tasks ADD COLUMN raw_payload     TEXT;
ALTER TABLE pm_tasks ADD COLUMN reporter_name   TEXT;
ALTER TABLE pm_tasks ADD COLUMN completed_at    TEXT;
ALTER TABLE pm_tasks ADD COLUMN ancestor_path   TEXT;
ALTER TABLE pm_tasks ADD COLUMN project_ids     TEXT;

-- Best-effort backfill for existing rows. The only canonical signal derivable
-- from today's columns is terminal-ness; Done vs Cancelled can't be told apart
-- from `is_terminal` alone, so terminal rows seed 'done' and everything else is
-- left NULL (unknown) until the next sync resolves it through the adapter.
UPDATE pm_tasks SET status_category = 'done' WHERE is_terminal = 1;
