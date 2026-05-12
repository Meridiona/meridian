-- meridian — normalises screenpipe activity into structured app sessions

-- ---------------------------------------------------------------------------
-- session_dimensions — multi-label tagging table.
--
-- A single row in `ticket_links` records which Jira task a session belongs to.
-- That's only one slice of the picture. session_dimensions is the parallel
-- multi-label store — for any (session_id, dimension), we may write zero,
-- one, or many values, each with its own confidence and source attribution.
--
-- The taxonomy (allowed dimensions and, for closed dimensions, allowed values)
-- lives in services/agents/taxonomy.py — the DB doesn't enforce it so the
-- taxonomy can evolve without migrations.
--
-- Common dimensions (see taxonomy.py):
--   activity       single  coding | code_review | learning | meeting | …
--   intent         single  implementation | refactor | exploration | …
--   engagement     single  deep_work | focused | context_switching | …
--   collaboration  single  solo | ai_assisted | pair_programming | team_review
--   tool           multi   open vocab — vscode, cursor, claude.ai, cargo, …
--   topic          multi   open vocab — rust, async, sqlite, embeddings, …
--   practice       multi   tests_written | type_checking | documentation_updated …
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS session_dimensions (
    session_id  INTEGER NOT NULL REFERENCES app_sessions(id),
    dimension   TEXT    NOT NULL,
    value       TEXT    NOT NULL,
    confidence  REAL    NOT NULL DEFAULT 0.5,
    source      TEXT    NOT NULL,                                 -- 'rule:<name>' | 'embedding' | 'llm' | 'manual'
    created_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    PRIMARY KEY (session_id, dimension, value)
);

CREATE INDEX IF NOT EXISTS idx_session_dimensions_dim_val ON session_dimensions (dimension, value);
CREATE INDEX IF NOT EXISTS idx_session_dimensions_session ON session_dimensions (session_id);
