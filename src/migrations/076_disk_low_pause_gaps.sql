-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
-- Extend the gaps table to accept a 'disk_low_paused' kind, written by the
-- tray's disk-space guard (poll::check_disk_space) when capture is
-- auto-paused because free disk space fell below the low-disk threshold.
-- SQLite does not support ALTER TABLE … MODIFY COLUMN, so we rebuild the
-- table, same as migration 051.

CREATE TABLE gaps_new (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    started_at  TEXT    NOT NULL,
    ended_at    TEXT    NOT NULL,
    duration_s  INTEGER NOT NULL,
    kind        TEXT    NOT NULL CHECK(kind IN (
                    'user_idle', 'system_sleep',
                    'tracking_paused', 'schedule_paused', 'disk_low_paused'
                )),
    etl_run_id  INTEGER,
    FOREIGN KEY (etl_run_id) REFERENCES etl_runs(id)
);

INSERT INTO gaps_new SELECT id, started_at, ended_at, duration_s, kind, etl_run_id FROM gaps;
DROP TABLE gaps;
ALTER TABLE gaps_new RENAME TO gaps;

CREATE INDEX IF NOT EXISTS idx_gaps_started_at ON gaps(started_at);
CREATE INDEX IF NOT EXISTS idx_gaps_kind       ON gaps(kind);
