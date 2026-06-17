-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- Per-session screenpipe frame provenance.
--
-- `frame_contributions` is a JSON array of the screenpipe frames whose OCR /
-- accessibility text actually fed this session's `session_text` (i.e. the frames
-- `get_frame_full_texts` returned with non-empty text), captured at ETL time:
--
--   [{"frame_id":126759,"timestamp":"2026-06-17T07:40:01Z","text_source":"ocr","chars":842}, ...]
--
-- Recorded at formation time on purpose: screenpipe prunes old frames, so by the
-- time the classifier runs the raw frames may be gone. Persisting the list on the
-- session row keeps the provenance durable, and lets the classifier emit a
-- `contributing_frames` span inside the classification trace without needing
-- screenpipe access. Capped (see FRAME_CONTRIBUTION_CAP) so a long session can't
-- bloat the row; `min_frame_id`/`max_frame_id`/`frame_count` still bound the full
-- window. NULL for sessions formed before this migration and for coding-agent
-- sessions (which have no screenpipe frames).

ALTER TABLE app_sessions   ADD COLUMN frame_contributions TEXT;
ALTER TABLE active_session ADD COLUMN frame_contributions TEXT;
