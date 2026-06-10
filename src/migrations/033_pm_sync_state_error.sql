-- meridian — normalises screenpipe activity into structured app sessions

-- Surface sync errors (e.g. 401/403 from provider APIs) so the UI can flag them.
ALTER TABLE pm_sync_state ADD COLUMN last_error TEXT;
