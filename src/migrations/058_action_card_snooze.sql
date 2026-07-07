-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- Per-kind dismiss for the standardized action-card stack (must-fix / cleanup
-- / drafts — see ui/components/timeline/ActionCard.tsx + useActionItems.ts).
-- Dismissing a card upserts snoozed_until so it stops rendering until that
-- moment passes. Mirrors pm_task_curation.snoozed_until's semantics (a NULL
-- or already-past snoozed_until means "show it").

CREATE TABLE IF NOT EXISTS action_card_snooze (
    card_kind     TEXT PRIMARY KEY,
    snoozed_until TEXT NOT NULL
);
