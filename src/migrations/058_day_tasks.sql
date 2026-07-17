-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
-- Rolling day-task accumulation — Meridian's own inferred day-level tasks.
--
-- Each local hour's activity report is folded into a small running set of TASKS
-- (workstreams, 1-5/day). `day_tasks` is the whole day's state, overwritten in
-- full on every fold — it IS the state store the next hour reads back. Minutes
-- are recomputed deterministically each fold from `day_hour_spans` (the measured
-- active minutes per local hour), never taken from the model.
--
-- `linked_ticket` is the only PM seam — always NULL for now; PM-provider linking
-- is deliberately deferred.

CREATE TABLE IF NOT EXISTS day_tasks (
    day_local     TEXT NOT NULL,               -- 'YYYY-MM-DD' local
    task_id       TEXT NOT NULL,               -- 'T1','T2',… stable within the day
    title         TEXT NOT NULL,
    summary       TEXT NOT NULL DEFAULT '',     -- running log: absorbed items, newline-joined
    hours_json    TEXT NOT NULL DEFAULT '[]',   -- ['YYYY-MM-DDTHH', …] local hour labels
    minutes       INTEGER NOT NULL DEFAULT 0,   -- deterministic, recomputed each fold
    status        TEXT NOT NULL DEFAULT 'active',
    linked_ticket TEXT,                          -- PM seam; always NULL for now
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    PRIMARY KEY (day_local, task_id)
);

CREATE INDEX IF NOT EXISTS idx_day_tasks_day ON day_tasks (day_local);

-- Measured active minutes per local hour — the deterministic time source the
-- minute recompute reads. Local-native (keyed by the same 'YYYY-MM-DDTHH' local
-- label the tasks store) so the recompute needs zero UTC<->local conversion.
CREATE TABLE IF NOT EXISTS day_hour_spans (
    day_local  TEXT NOT NULL,
    hour_label TEXT NOT NULL,   -- 'YYYY-MM-DDTHH' local
    span_min   INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (day_local, hour_label)
);
