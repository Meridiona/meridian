-- meridian — normalises screenpipe activity into structured app sessions

-- Track whether the category was assigned by the rule-based ETL or re-classified
-- by Foundation Models post-ETL.  'rule_based' is the default for all existing rows.
ALTER TABLE app_sessions ADD COLUMN category_method TEXT NOT NULL DEFAULT 'rule_based';
