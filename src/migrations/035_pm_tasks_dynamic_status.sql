-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

-- Make PM task status dynamic. Trackers (and even individual boards on the same
-- tracker) use arbitrary, user-defined column / state names: "In Review", "QA",
-- "Ready for Deploy", "Shipped". Collapsing every name into three hardcoded
-- buckets ('todo' / 'in_progress' / 'done') silently mismaps any column the
-- substring heuristic didn't anticipate — which is what breaks board sync.
--
-- Replace the bucketed `status_category` with two columns:
--   status_raw  -- the verbatim provider status/column name, shown to the user as-is
--   is_terminal -- the one normalized signal logic needs: is this ticket done/closed?
--
-- Backfill from the old buckets so existing rows keep working until the next sync
-- re-resolves them through the shared status resolver.

ALTER TABLE pm_tasks ADD COLUMN status_raw  TEXT    NOT NULL DEFAULT '';
ALTER TABLE pm_tasks ADD COLUMN is_terminal INTEGER NOT NULL DEFAULT 0;

UPDATE pm_tasks
   SET status_raw  = status_category,
       is_terminal = CASE WHEN status_category = 'done' THEN 1 ELSE 0 END;

ALTER TABLE pm_tasks DROP COLUMN status_category;
