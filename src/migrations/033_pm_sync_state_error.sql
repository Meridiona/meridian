-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

-- Surface sync errors (e.g. 401/403 from provider APIs) so the UI can flag them.
ALTER TABLE pm_sync_state ADD COLUMN last_error TEXT;
