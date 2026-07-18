-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- The end-of-day review: one AI-composed summary per local day.
--
-- WHY THIS EXISTS. Meridian already knows what you did - `day_tasks` folds each
-- hour into a handful of workstreams with minute-precise segments, and
-- `pm_worklog_hours` carries the per-hour activity reports. Nothing ever told the
-- developer how their day WENT. The timeline shows data and leaves the
-- interpreting to the reader. This table stores the interpretation.
--
-- WHY THE PANELS STORE FORM AND NOT DATA. `panels_json` holds Vega-Lite specs
-- that bind their data by NAME (`{"data": {"name": "segments"}}`); the real rows
-- are re-read live at render and injected then. Storing the numbers alongside the
-- spec was the obvious alternative and is wrong twice over:
--
--   1. The day's data keeps moving. An hour that folds in after a summary is
--      generated changes `day_tasks`. A frozen copy of the numbers would render a
--      chart that disagrees with the timeline beside it - silently, and looking
--      entirely authoritative. A spec re-bound to live rows cannot drift.
--   2. It is the house rule. `worklog_pipeline::workstream` already says the model
--      owns judgement and code owns plumbing: durations are derived from segments,
--      never taken from the model. Keeping data out of the spec means a
--      hallucinated number has nowhere to live.
--
-- So the model chooses WHICH panels, HOW MANY, and WHAT FORM each takes; it never
-- supplies a value. What is persisted here is only that choice.
--
-- WHY IT IS CACHED AT ALL, given the above. Generating costs a real LLM call on
-- the user's own subscription (~a minute). Re-deriving on every open would spend
-- that quota to answer a question the user already asked. Regenerating is an
-- explicit act, so this is an UPSERT on `day_local`: one summary per day, the
-- newest wins, and each regeneration may legitimately look nothing like the last.
--
-- `fallback` records that the model produced nothing usable and the deterministic
-- panel set was substituted. It is NOT an error - the screen always renders - but
-- it is the one number that says whether this feature is actually working, so it
-- is stored rather than merely logged.

CREATE TABLE day_summaries (
    -- 'YYYY-MM-DD' local. One summary per day; regenerate overwrites.
    day_local     TEXT PRIMARY KEY,
    -- The model's prose: how the day went. Empty on the fallback path.
    narrative     TEXT NOT NULL DEFAULT '',
    -- [{title, why, spec}, ...] - validated Vega-Lite specs. Form only, no data.
    panels_json   TEXT NOT NULL DEFAULT '[]',
    -- The model's insight lines, as a JSON array of strings.
    insights_json TEXT NOT NULL DEFAULT '[]',
    -- Which provider ACTUALLY answered. May differ from the user's setting: the
    -- resolver degrades to local on failure and on a rate limit, and the whole
    -- point of recording it is to know which one produced this.
    provider      TEXT NOT NULL,
    -- The model override in force (settings.json `llm_provider_model`), or ''.
    -- 'haiku' and 'sonnet' are not the same author; a summary is only comparable
    -- against another from the same one.
    model         TEXT NOT NULL DEFAULT '',
    -- 1 = the model produced no usable panel and the deterministic set was used.
    fallback      INTEGER NOT NULL DEFAULT 0,
    generated_at  TEXT NOT NULL
);
