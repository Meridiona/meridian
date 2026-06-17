-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- Daily-plan task snapshot. `daily_plan` references board tickets by key, joined
-- against `pm_tasks` at read time for their live title/status/etc. But `pm_tasks`
-- is the *active* board: when a planned ticket goes Done it is pruned from the
-- provider sync, so the join finds nothing and the /plan card collapses to a bare
-- key. We don't want completed tickets back on the active board (they'd pollute
-- the Tasks page, triage/cleanup, and the classifier candidate set), so instead we
-- keep a small denormalised snapshot of the ticket ON the plan row.
--
-- `task_snapshot` holds a JSON blob captured from `pm_tasks` whenever the dev adds
-- or confirms the task (while it is still on the board). When the live `pm_tasks`
-- row later disappears, the /plan reader falls back to this snapshot so a
-- planned-then-completed ticket stays visible in that day's plan with its real
-- title / description / epic. NULL until the first write that has board data.

ALTER TABLE daily_plan ADD COLUMN task_snapshot TEXT;
