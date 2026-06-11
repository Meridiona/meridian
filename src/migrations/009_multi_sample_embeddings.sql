-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

-- ---------------------------------------------------------------------------
-- Multi-sample (multi-vector) session embeddings.
--
-- The single-vector representation in 008 averaged each session's OCR + titles
-- into one 384-d point, which collapsed when an early OCR sample was app
-- chrome (Claude.ai settings, VS Code extension banner) — the actual signal
-- got drowned out. Stage 2 now treats each OCR sample (plus the title block
-- and audio block) as its own vector and scores against pm_tasks via
--
--     cosine(session, task) = max over session_samples of cos(sample, task)
--
-- which is the standard ColBERT-style "MaxSim" trick. We also use the same
-- max-pool over session-pair samples for the `past_vote` retrieval.
--
-- Schema change: widen the primary key with `sample_idx` and add a
-- `sample_label` so logs/tests are readable. We DROP the old single-row
-- table — vectors are reproducible from app_sessions, so re-embedding on the
-- next cycle costs nothing.
-- ---------------------------------------------------------------------------

DROP INDEX IF EXISTS idx_session_embeddings_model;
DROP TABLE IF EXISTS session_embeddings;

CREATE TABLE session_embeddings (
    session_id    INTEGER NOT NULL REFERENCES app_sessions(id),
    model         TEXT    NOT NULL,
    sample_idx    INTEGER NOT NULL DEFAULT 0,
    sample_label  TEXT    NOT NULL DEFAULT 'titles',  -- 'titles' | 'audio' | 'ocr_0' | 'ocr_1' | ...
    dim           INTEGER NOT NULL,
    embedding     BLOB    NOT NULL,
    text_hash     TEXT    NOT NULL,                   -- hash of the *combined* sample texts for change-detection
    created_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    PRIMARY KEY (session_id, model, sample_idx)
);

CREATE INDEX IF NOT EXISTS idx_session_embeddings_model      ON session_embeddings (model);
CREATE INDEX IF NOT EXISTS idx_session_embeddings_session_id ON session_embeddings (session_id);
