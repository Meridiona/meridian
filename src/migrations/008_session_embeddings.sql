-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

-- ---------------------------------------------------------------------------
-- Stage-2 vector index for the agent-side tagger.
--
-- We store float32 embeddings as raw BLOBs alongside the model name + dim
-- so a future model swap (e.g. MiniLM → bge-small or a 768-d encoder) can
-- coexist with old rows during a re-embed. `text_hash` is the sha1 of the
-- exact text fed to the encoder, which lets us re-embed only when the text
-- actually changes.
--
-- At meridian's scale (low-thousands of sessions, low-hundreds of pm_tasks)
-- a brute-force `M @ q` matmul in numpy is <5 ms — no vector index needed.
-- If we ever exceed ~50k rows or hit p95 latency >20 ms, layer sqlite-vec
-- on top in a separate migration; the BLOB column stays authoritative.
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS session_embeddings (
    session_id  INTEGER NOT NULL REFERENCES app_sessions(id),
    model       TEXT    NOT NULL,                                 -- e.g. 'bge-small-en-v1.5'
    dim         INTEGER NOT NULL,                                 -- 384 for bge-small
    embedding   BLOB    NOT NULL,                                 -- float32 little-endian, length = dim*4
    text_hash   TEXT    NOT NULL,                                 -- sha1 of the encoded text
    created_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    PRIMARY KEY (session_id, model)
);

CREATE INDEX IF NOT EXISTS idx_session_embeddings_model ON session_embeddings (model);


CREATE TABLE IF NOT EXISTS pm_task_embeddings (
    task_key       TEXT    NOT NULL REFERENCES pm_tasks(task_key),
    model          TEXT    NOT NULL,
    dim            INTEGER NOT NULL,
    embedding      BLOB    NOT NULL,
    text_hash      TEXT    NOT NULL,
    pm_updated_at  TEXT    NOT NULL,                              -- copied from pm_tasks.updated_at at embed time
    expected_dims  TEXT,                                          -- JSON: {"activity":[...], "topic":[...], "tool":[...]} for Path-C dim_overlap
    created_at     TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    PRIMARY KEY (task_key, model)
);

CREATE INDEX IF NOT EXISTS idx_pm_task_embeddings_model ON pm_task_embeddings (model);
