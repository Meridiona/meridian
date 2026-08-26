-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
--
-- Single-owner PM sync: the request side of the outbox.
--
-- WHY THIS EXISTS
-- An Atlassian OAuth refresh token is single-use and rotating: the old token dies
-- the instant the new one is issued, so a lost response leaves the grant
-- recoverable only inside a 10-minute window and unrecoverable after it. That
-- makes the token a resource with exactly one safe writer.
--
-- It had several. The tray refreshed in-process, the daemon refreshed on its poll
-- loop, and the tray spawned `meridian pm-sync` / `tasks-sync` as fresh processes
-- that each refreshed too. Mutual exclusion was left to an advisory file lock that
-- could not actually deliver it: its 10 s timeout is shorter than the ~26 s a
-- refresh can take (3 attempts x 8 s + backoff), and on failure the code proceeded
-- WITHOUT the lock rather than backing off. Two processes could therefore spend the
-- same token, and the only thing preventing corruption was Atlassian's grace window
-- handing the loser the current pair. Correctness by vendor accident.
--
-- So sync becomes a REQUEST rather than an action. Producers (tray window opens,
-- tracker connect, "Sync now", the CLI) write a row here; the daemon is the sole
-- consumer and the sole holder of the credential.
--
-- COALESCING, NOT A QUEUE
-- `provider` is the PRIMARY KEY so repeated requests collapse into one pending row
-- (`ON CONFLICT DO UPDATE`). Opening the dashboard ten times must not queue ten
-- syncs - it must mean "a sync is wanted", once. `'*'` is the all-providers request
-- that every current producer writes; per-provider rows are reserved for a future
-- caller that needs to refresh just one board.
--
-- `mode` escalates and never de-escalates while a row is pending: a 'force' request
-- landing on a pending 'gated' one upgrades it, because a user who just connected a
-- tracker or pressed "Sync now" must not have their explicit request downgraded by a
-- passing window focus. The reverse is silently ignored by the producer's UPSERT.
--
-- COMPLETION IS REPORTED IN PLACE, NOT DELETED
-- The daemon stamps `completed_at` plus `error` / `synced_count` rather than removing
-- the row, so "Sync now" can show a real outcome without the tray needing to hold the
-- credential or shell out. A serviced row is retained until the next request replaces
-- it, which also gives `meridian health` a cheap, content-free view of the last sync
-- attempt.

CREATE TABLE IF NOT EXISTS pm_sync_requests (
    -- '*' = all configured providers. A specific provider name scopes the request.
    provider      TEXT    NOT NULL PRIMARY KEY,
    -- 'gated'  - honour the per-provider staleness window (the cheap common case).
    -- 'force'  - bypass it; the user explicitly asked (connect, "Sync now", CLI).
    mode          TEXT    NOT NULL DEFAULT 'gated',
    -- Free-text producer tag for tracing only (e.g. 'dashboard_open', 'token_connected').
    -- Never a user-content value: this is read back into logs.
    reason        TEXT    NOT NULL DEFAULT '',
    requested_at  TEXT    NOT NULL,
    -- Set when the daemon starts servicing, so a request in flight is distinguishable
    -- from one still waiting. Cleared on the next request.
    claimed_at    TEXT,
    -- Set when the daemon finishes, success or failure. NULL while pending/in-flight.
    completed_at  TEXT,
    -- NULL on success. The failure detail otherwise, for the tray to surface.
    error         TEXT,
    -- Tasks refreshed on the last completed pass, for "Sync now" feedback.
    synced_count  INTEGER
);

-- No secondary index on purpose. Every hot query (`claim`, `complete`, `outcome`,
-- `request`) filters on `provider`, which is the PRIMARY KEY, so an index on
-- (completed_at, claimed_at) would never be chosen. The only query that does not
-- name a provider is `reset_stale_claims`, which runs once per daemon boot over a
-- table holding one row per provider - a scan there is free.
