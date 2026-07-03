-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- Persist the per-hour activity REPORT — the human-readable /activity_report
-- LLM output (stage_report), distinct from hour_text (the raw distilled INPUT
-- added in migration 053). The dashboard's hour-detail "ACTIVITY SUMMARY" box
-- must show this, not the distilled input body. Nullable: an un-reported or
-- pre-054 hour row simply carries NULL.

ALTER TABLE pm_worklog_hours ADD COLUMN hour_report TEXT;
ALTER TABLE pm_worklog_hours ADD COLUMN hour_report_chars INTEGER;
