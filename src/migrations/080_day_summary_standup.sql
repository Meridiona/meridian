-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- The daily summary's standup block: the day, written as the three or four lines
-- you would actually say tomorrow morning.
--
-- Stored as a JSON array of strings rather than one blob of text, because the
-- screen renders it as bullets and the Copy button joins it back with newlines -
-- splitting a stored paragraph on newlines in the UI would make the bullet list a
-- guess about the model's formatting rather than a fact about its answer.
--
-- Defaults to '[]', so every row written before this migration reads back as "no
-- standup" and the block simply does not render. Nothing recomposes on upgrade;
-- the next generate for that day fills it in.

ALTER TABLE day_summaries ADD COLUMN standup_json TEXT NOT NULL DEFAULT '[]';
