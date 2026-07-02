-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- Persist the per-hour distilled activity text on the hour ledger. The worklog
-- pipeline's distill stage produces a compact body for every hour it processes;
-- storing it here (independent of whether the hour yields a worklog draft) lets
-- the dashboard's hour-detail panel show "here's what happened this hour" even
-- when no ticket matched. All three columns are nullable — an un-distilled or
-- pre-053 hour row simply carries NULLs (distinguishable from "processed, empty").

ALTER TABLE pm_worklog_hours ADD COLUMN hour_text TEXT;
ALTER TABLE pm_worklog_hours ADD COLUMN hour_text_chars INTEGER;
ALTER TABLE pm_worklog_hours ADD COLUMN hour_text_reduction_pct REAL;
