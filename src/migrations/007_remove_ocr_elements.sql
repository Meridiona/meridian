-- meridian — normalises screenpipe activity into structured app sessions
-- Remove ocr_samples and elements_samples columns; content will be replaced
-- by session_text (full_text line-union) in a follow-up migration.
ALTER TABLE app_sessions   DROP COLUMN ocr_samples;
ALTER TABLE app_sessions   DROP COLUMN elements_samples;
ALTER TABLE active_session DROP COLUMN ocr_samples;
ALTER TABLE active_session DROP COLUMN elements_samples;
