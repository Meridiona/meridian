-- meridian — normalises screenpipe activity into structured app sessions

-- Track the source of session_text per session so the LLM can calibrate
-- confidence: accessibility tree text is clean/structured, OCR is noisier.
-- Values: 'accessibility' | 'ocr' | 'hybrid' | 'unknown'
ALTER TABLE app_sessions    ADD COLUMN session_text_source TEXT;
ALTER TABLE active_session  ADD COLUMN session_text_source TEXT;
