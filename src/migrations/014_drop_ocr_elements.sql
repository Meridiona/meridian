-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
-- Remove ocr_samples and elements_samples columns.
-- Uses ALTER TABLE DROP COLUMN (SQLite 3.35+) to avoid recreating the table,
-- which would require disabling FK constraints (sqlx enables them by default).

ALTER TABLE app_sessions   DROP COLUMN ocr_samples;
ALTER TABLE app_sessions   DROP COLUMN elements_samples;

ALTER TABLE active_session DROP COLUMN ocr_samples;
ALTER TABLE active_session DROP COLUMN elements_samples;
