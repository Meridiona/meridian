-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

-- Replace per-row expires_at TTL with a single last_synced_at timestamp per
-- provider. This decouples "when to re-fetch" from task data and prevents a
-- single out-of-band upsert from holding the refresh gate open.

CREATE TABLE IF NOT EXISTS pm_sync_state (
    provider       TEXT PRIMARY KEY,
    last_synced_at TEXT NOT NULL
);

-- expires_at had a dedicated index; drop it before dropping the column.
DROP INDEX IF EXISTS idx_pm_tasks_expires_at;

ALTER TABLE pm_tasks DROP COLUMN expires_at;
