-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- Daily plan — the developer's declared "what I'm working on today" set. This is
-- the Tier-1 priority signal for classification: tasks the dev commits to in the
-- morning lead the candidate list and are labelled as today's focus. It is a
-- *boost*, never a filter — every non-excluded board ticket still flows through
-- as a candidate, so a mid-day pivot to an undeclared task is still matchable.
--
-- Only EXPLICIT dev intent is stored here (committed rows). Suggestions are
-- computed on the fly from board signals (in-progress / carried-over / recent /
-- due-soon) and are never persisted until the dev acts. `daily_plan_meta` records
-- whether the dev confirmed (or skipped) the ritual for a given day, which is what
-- decides "show suggestions" vs "use the committed set / evidence fallback".

CREATE TABLE IF NOT EXISTS daily_plan (
    plan_date  TEXT    NOT NULL,                 -- local calendar day, 'YYYY-MM-DD'
    task_key   TEXT    NOT NULL,                 -- references pm_tasks.task_key (loose, not FK — provider tickets churn)
    position   INTEGER NOT NULL DEFAULT 0,       -- drag order within the day (ascending)
    origin     TEXT    NOT NULL DEFAULT 'manual', -- carryover | in_progress | recent | due_soon | manual
    created_at TEXT    NOT NULL,
    updated_at TEXT    NOT NULL,
    PRIMARY KEY (plan_date, task_key)
);

CREATE INDEX IF NOT EXISTS idx_daily_plan_date ON daily_plan (plan_date);

-- One row per planned day. Presence of a row = the dev acted that day.
CREATE TABLE IF NOT EXISTS daily_plan_meta (
    plan_date    TEXT    NOT NULL PRIMARY KEY,   -- local calendar day, 'YYYY-MM-DD'
    confirmed_at TEXT,                            -- set when the dev confirms (or skips)
    skipped      INTEGER NOT NULL DEFAULT 0,      -- 1 = dev dismissed the ritual; evidence fallback drives Tier-1
    created_at   TEXT    NOT NULL,
    updated_at   TEXT    NOT NULL
);
