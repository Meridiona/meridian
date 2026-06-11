-- meridian — normalises screenpipe activity into structured app sessions

-- Remove gap rows that are exact duplicates of an earlier row for the same
-- time window. Duplicates arise when an ETL run is aborted mid-way and
-- cleanup_incomplete_runs did not previously delete its gap rows, causing the
-- same gaps to be re-inserted on subsequent runs. Keep the lowest id (earliest
-- inserted) for each (started_at, ended_at) pair.
DELETE FROM gaps
WHERE id NOT IN (
    SELECT MIN(id)
    FROM gaps
    GROUP BY started_at, ended_at
);
