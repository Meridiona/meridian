-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- One day-task's status update can now land on SEVERAL tickets. A day's strand of
-- work routinely advances more than one planned task, and 060 could only record
-- one: a single `match_task_key`, a single `posted_comment_id`, a single
-- `browse_url`. This promotes the match branch out of `day_task_worklogs` into a
-- child table, one row per ticket the update posts to.
--
-- WHY A TABLE AND NOT A JSON ARRAY. The posted state is per-ticket, and it has to
-- be, because posting is not atomic: a comment cannot be un-posted. Approving to
-- three tickets can succeed on two and fail on the third, and the retry must post
-- ONLY the third. A JSON blob makes that a read-modify-write race; a row per
-- ticket with its own `posted_comment_id` makes "already posted" a fact the retry
-- reads directly. `posted_comment_id IS NOT NULL` is the idempotency key, exactly
-- as `created_task_key` is for the create-a-proposed-ticket step.
--
-- The PROPOSE branch stays on the parent — a draft proposes at most one new
-- ticket, and once approve creates it, it is inserted here as an ordinary target.
--
-- `manual` marks a ticket the USER picked over the model (was 061's
-- `target_manual`). It is a column and not a `confidence = 1.0` sentinel because
-- the generator clamps model confidence to 1.0, so the sentinel could not tell a
-- human's choice from a confident model's, and the UI must never label the user's
-- own pick "100% match".

CREATE TABLE IF NOT EXISTS day_task_worklog_targets (
    day_local         TEXT NOT NULL,
    task_id           TEXT NOT NULL,
    task_key          TEXT NOT NULL,
    -- The tracker this ticket lives on. Per-target, not per-draft: nothing stops a
    -- day's plan from spanning two trackers.
    provider          TEXT NOT NULL,
    -- The model's honest confidence, or 1.0 and `manual = 1` when the user picked it.
    confidence        REAL NOT NULL DEFAULT 0,
    manual            INTEGER NOT NULL DEFAULT 0,
    position          INTEGER NOT NULL DEFAULT 0,
    -- Per-target delivery state. `posted_comment_id IS NOT NULL` means the comment
    -- is live on the tracker and this target must never be posted to again.
    posted_comment_id TEXT,
    browse_url        TEXT,
    posted_at         TEXT,
    last_error        TEXT,
    created_at        TEXT NOT NULL,
    PRIMARY KEY (day_local, task_id, task_key)
);

CREATE INDEX IF NOT EXISTS idx_worklog_targets_row
    ON day_task_worklog_targets (day_local, task_id, position);

-- Carry every existing draft's single target across. COALESCE(match_task_key,
-- target_key) covers both branches: a matched row names its ticket in
-- `match_task_key`; a proposal that was already approved has only the created key
-- in `target_key`. A proposal still awaiting approval has neither, and correctly
-- lands no target row.
INSERT OR IGNORE INTO day_task_worklog_targets
    (day_local, task_id, task_key, provider, confidence, manual, position,
     posted_comment_id, browse_url, posted_at, last_error, created_at)
SELECT day_local,
       task_id,
       COALESCE(match_task_key, target_key),
       provider,
       COALESCE(match_confidence, 0),
       COALESCE(target_manual, 0),
       0,
       posted_comment_id,
       browse_url,
       CASE WHEN state = 'posted' THEN updated_at END,
       NULL,
       created_at
FROM day_task_worklogs
WHERE COALESCE(match_task_key, target_key) IS NOT NULL;

-- The parent's single-target columns are now superseded by the child table.
-- Dropped rather than left in place: a dead `match_task_key` sitting next to the
-- real answer is precisely how a second, silently-wrong source of truth gets read
-- by the next person. `created_task_key` STAYS — it is the create-idempotency key
-- for the proposal branch and belongs to the draft, not to any one target.
ALTER TABLE day_task_worklogs DROP COLUMN match_task_key;
ALTER TABLE day_task_worklogs DROP COLUMN match_confidence;
ALTER TABLE day_task_worklogs DROP COLUMN target_key;
ALTER TABLE day_task_worklogs DROP COLUMN target_manual;
ALTER TABLE day_task_worklogs DROP COLUMN posted_comment_id;
ALTER TABLE day_task_worklogs DROP COLUMN browse_url;
