-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
-- Extend the gaps table to support tray-inserted tracking pauses.
--
-- Two changes:
--   1. Expand the kind CHECK to accept 'tracking_paused' and 'schedule_paused'.
--   2. Make etl_run_id nullable — tray-inserted pause gaps have no ETL run.
-- SQLite does not support ALTER TABLE … MODIFY COLUMN, so we rebuild the table.

CREATE TABLE gaps_new (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    started_at  TEXT    NOT NULL,
    ended_at    TEXT    NOT NULL,
    duration_s  INTEGER NOT NULL,
    kind        TEXT    NOT NULL CHECK(kind IN (
                    'user_idle', 'system_sleep',
                    'tracking_paused', 'schedule_paused'
                )),
    etl_run_id  INTEGER,
    FOREIGN KEY (etl_run_id) REFERENCES etl_runs(id)
);

INSERT INTO gaps_new SELECT id, started_at, ended_at, duration_s, kind, etl_run_id FROM gaps;
DROP TABLE gaps;
ALTER TABLE gaps_new RENAME TO gaps;

CREATE INDEX IF NOT EXISTS idx_gaps_started_at ON gaps(started_at);
CREATE INDEX IF NOT EXISTS idx_gaps_kind       ON gaps(kind);
