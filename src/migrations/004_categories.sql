-- meridian — normalises screenpipe activity into structured app sessions

-- Add activity category and confidence score to every closed session.
-- Populated inline during ETL at the moment each session closes.
ALTER TABLE app_sessions   ADD COLUMN category   TEXT NOT NULL DEFAULT 'idle_personal';
ALTER TABLE app_sessions   ADD COLUMN confidence REAL NOT NULL DEFAULT 0.0;

-- Also stored on the in-progress session so the dashboard can show a live estimate.
ALTER TABLE active_session ADD COLUMN category   TEXT NOT NULL DEFAULT 'idle_personal';
ALTER TABLE active_session ADD COLUMN confidence REAL NOT NULL DEFAULT 0.0;
