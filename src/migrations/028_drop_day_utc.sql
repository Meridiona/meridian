-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
-- Drop the redundant day_utc column from app_sessions.
-- The date is already encoded in started_at; callers use substr(started_at,1,10).
ALTER TABLE app_sessions DROP COLUMN day_utc;
