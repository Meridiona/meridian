-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- Dev-only LLM comparison harness ("LLM Lab"): replay one prose stage (hour report /
-- workstream fold / worklog generate) from stored inputs across several provider/model
-- variants and persist every outcome side by side. NEVER feeds production tables --
-- pm_worklog_hours / day_tasks / day_task_worklogs are read as inputs only.

CREATE TABLE IF NOT EXISTS llm_experiments (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    -- 'hour_report' | 'workstream_fold' | 'worklog_generate'
    process     TEXT NOT NULL,
    -- Human key of the replayed input: 'YYYY-MM-DDTHH' or 'YYYY-MM-DD/<task_id>'.
    input_ref   TEXT NOT NULL,
    -- Snapshot of the EXACT variant-independent request: {"user": "...", "label": "...",
    -- "render_ctx": {...}}. The system prompt and schema are re-derived from `process`,
    -- so a run is reproducible and the UI can show precisely what was sent.
    input_json  TEXT NOT NULL DEFAULT '{}',
    -- 'running' | 'done' | 'failed' (failed = input assembly failed or every variant errored)
    status      TEXT NOT NULL DEFAULT 'running',
    n_variants  INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL,
    finished_at TEXT
);

CREATE TABLE IF NOT EXISTS llm_experiment_results (
    experiment_id INTEGER NOT NULL,
    variant_idx   INTEGER NOT NULL,
    -- LlmProvider wire form: 'claude' | 'codex' | 'cursor' | 'copilot' | 'local'
    provider      TEXT NOT NULL,
    -- '' = the provider's default model
    model         TEXT NOT NULL DEFAULT '',
    -- Future variant dimensions (prompt version, temperature, ...) ride here.
    params_json   TEXT NOT NULL DEFAULT '{}',
    -- 'pending' | 'running' | 'ok' | 'failed' | 'rate_limited'
    status        TEXT NOT NULL DEFAULT 'pending',
    -- The model's raw answer.
    output_text     TEXT,
    -- What the pipeline would have made of it (assembled report / parsed placements /
    -- parsed draft JSON) -- rendered, never persisted to production tables.
    output_rendered TEXT,
    error           TEXT,
    input_tokens  INTEGER,
    output_tokens INTEGER,
    elapsed_s     REAL,
    started_at    TEXT,
    finished_at   TEXT,
    PRIMARY KEY (experiment_id, variant_idx)
);

CREATE INDEX IF NOT EXISTS idx_llm_experiments_created ON llm_experiments(created_at DESC);
