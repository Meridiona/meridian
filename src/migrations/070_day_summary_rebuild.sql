-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- The daily summary, rebuilt around the question it should have been asking.
--
-- WHAT CHANGED AND WHY
--
-- 066 stored a narrative, some insight strings, and `panels_json` — 0-2 Vega-Lite
-- specs the model authored itself. The specs are gone, along with the vendored
-- 1.8 MB Vega-Lite schema, the two-layer spec validator and the chart runtime that
-- rendered them. They produced the one thing that screen must never be: a
-- monitoring dashboard. A person at the end of a day does not want a pie chart of
-- their categories.
--
-- What they do want is an answer to "did the day go the way I meant it to". So the
-- row now also carries:
--
--   headline        one warm line above everything.
--   plan_json       one verdict per PLANNED ticket - done or not_touched. The
--                   outcome is BINARY: a ticket the day's work touched is done,
--                   otherwise not_touched. (`partial` is retained in the wire enum
--                   for back-compat but is never emitted.) Carries the evidence for
--                   the verdict and the measured minutes.
--   adherence_json  the arithmetic: planned, done, partial (always 0), not_touched,
--                   the percentage, and the minutes of substantial work no planned
--                   ticket accounts for.
--   themes_json     UNUSED for now - kept only for wire back-compat. The generator
--                   writes `[]` unconditionally and the LLM schema offers the model
--                   no themes field, so nothing populates it. (It was intended to
--                   hold, for a no-plan day, what the day turned out to be about.)
--   evidence_at     the newest tracked activity the summary was composed from, so
--                   reopening it hours later can tell the day has moved on and
--                   quietly recompose rather than showing a summary of a morning.
--
-- WHY THE SCORE IS NOT STORED AS THE MODEL SAID IT
--
-- `adherence_json` is computed in Rust from `plan_json`, never parsed out of the
-- model's prose, and the outcomes in `plan_json` are locked wherever the DATABASE
-- already settles them (a worklog was posted against the ticket, the workstream
-- carries it as its linked ticket, or the ticket went terminal). The model only
-- fills the gaps. That is what makes the ring and the list beside it two views of
-- one array instead of two opinions that can disagree.
--
-- INSIGHTS CHANGED SHAPE
--
-- `insights_json` held bare strings; it now holds `{title, text}` objects.
-- Deliberately NOT migrated: the reader decodes tolerantly and an old row degrades
-- to an empty insight list, which the next generate replaces. Rewriting historical
-- prose into a shape the model never wrote it in would be inventing data, and a
-- summary is a convenience - losing the insight lines on days composed before this
-- release costs nothing worth a data migration.
--
-- Note the columns are added rather than the table recreated: a day's narrative is
-- worth keeping across the change.

ALTER TABLE day_summaries DROP COLUMN panels_json;

ALTER TABLE day_summaries ADD COLUMN headline TEXT NOT NULL DEFAULT '';
ALTER TABLE day_summaries ADD COLUMN plan_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE day_summaries ADD COLUMN adherence_json TEXT NOT NULL DEFAULT '{}';
ALTER TABLE day_summaries ADD COLUMN themes_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE day_summaries ADD COLUMN evidence_at TEXT;
