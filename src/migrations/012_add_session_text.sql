-- meridian — normalises screenpipe activity into structured app sessions
ALTER TABLE app_sessions   ADD COLUMN session_text TEXT;
ALTER TABLE active_session ADD COLUMN session_text TEXT;
