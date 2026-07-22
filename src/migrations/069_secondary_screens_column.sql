-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- Adds secondary_screens (JSON array) to app_sessions / active_session — the
-- per-session rollup of capture_secondary_screens rows (multi-screen capture,
-- context-only). Same shape/treatment as the existing audio_snippets column:
-- nullable, NULL for pre-existing rows, capped + deduplicated on write (see
-- src/etl/session_builder.rs SECONDARY_SCREEN_CAP).
--
-- Kept as its own column rather than folded into `signals`: signals is a
-- generic {event_type, value, timestamp} bucket (src/db/screenpipe.rs
-- SignalEvent) for single-value events (clipboard, app_switch);
-- secondary-screen samples carry a richer payload (monitor + app + window +
-- OCR text) that doesn't fit that shape, same reasoning window_titles and
-- audio_snippets already get their own columns.

ALTER TABLE app_sessions ADD COLUMN secondary_screens TEXT;
ALTER TABLE active_session ADD COLUMN secondary_screens TEXT;
