-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

-- Onboarding board-cleanup state. Kept in its OWN table, never on pm_tasks,
-- because pm_tasks is a pure provider mirror: every sync runs INSERT ... ON
-- CONFLICT DO UPDATE over all its columns and prune() deletes departed rows, so a
-- decision stored there would be clobbered. Here the machine writes `bucket` +
-- `reasons_json` on every triage pass, while the human `decision` is written once
-- and never overwritten by re-triage.
--
-- task_key is the join key to pm_tasks. No hard FK (SQLite FKs are off by default
-- in the daemon's pools); the triage hook prunes curation rows whose task_key no
-- longer exists in pm_tasks.

CREATE TABLE IF NOT EXISTS pm_task_curation (
    task_key             TEXT PRIMARY KEY,
    provider             TEXT NOT NULL DEFAULT '',
    -- machine proposal, re-written every triage pass:
    bucket               TEXT NOT NULL,            -- ready | needs_detail | looks_stale | not_sure
    reasons_json         TEXT NOT NULL DEFAULT '[]',
    triaged_at           TEXT NOT NULL,
    -- human verdict, written once, preserved across re-triage:
    decision             TEXT,                     -- keep | excluded | snoozed (NULL = undecided)
    decided_at           TEXT,
    snoozed_until        TEXT,
    -- optional locally-drafted enrichment (LLM step, added later):
    enriched_description TEXT
);

-- The onboarding UI lists undecided, non-ready tickets worst-first; index the
-- fields that query filters on.
CREATE INDEX IF NOT EXISTS idx_pm_task_curation_bucket   ON pm_task_curation (bucket);
CREATE INDEX IF NOT EXISTS idx_pm_task_curation_decision ON pm_task_curation (decision);
