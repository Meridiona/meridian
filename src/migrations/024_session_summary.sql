-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- Add a `session_summary` column to `app_sessions` so the classifier can
-- emit a 10-40 sentence factual summary of the user's activity in the
-- session. The PM-update workflow reads this column instead of the raw
-- 2KB session_text excerpt — it's pre-digested, much smaller, and
-- preserves SDLC-relevant signal (files touched, commands run, errors
-- hit, decisions, tests, blockers, validations, commits, research).
--
-- Nullable: old rows stay NULL until re-classified. The PM-update layer
-- falls back to session_text excerpts for rows where this is NULL.

ALTER TABLE app_sessions ADD COLUMN session_summary TEXT;
