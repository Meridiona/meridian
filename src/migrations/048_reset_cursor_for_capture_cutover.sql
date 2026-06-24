-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- Reset the ETL cursor for the in-process-capture cutover (Gap-2 Bucket 2, slice 4b).
--
-- Before this migration the daemon read frames from screenpipe's DB, so
-- etl_cursor.last_frame_id held a *screenpipe* frame id (typically in the
-- millions). From slice 4b the daemon reads `capture_frames` instead, whose
-- ids restart at 1 (AUTOINCREMENT). Left unchanged, `WHERE id > last_frame_id`
-- would skip every capture frame forever. Resetting to 0 makes the first ETL
-- run after the cutover process the capture table from the beginning.
--
-- Safe because: capture_frames is freshly populated (nothing processed yet),
-- AUTOINCREMENT guarantees every row has id > 0, and only the cutover daemon
-- (which reads capture_frames) ever applies this migration. Already-processed
-- screenpipe sessions remain in app_sessions as history; the only effect is a
-- one-time boundary gap between the last screenpipe session and the first
-- capture session.

UPDATE etl_cursor SET last_frame_id = 0;
