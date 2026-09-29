-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

-- Give each PM sync request a monotonic sequence number, so a producer can tell
-- WHICH request an outcome belongs to.
--
-- Migration 082 modelled the outbox as one row per provider whose completion was
-- signalled by `completed_at` going non-NULL, and `request()` cleared it so the row
-- "unambiguously represents work still to do". With one waiter that is fine. With
-- two it loses answers, and the tracker-connect flow always produces at least two:
-- `oauth_connected`, `token_connected` and the user's own "Sync now" all fire inside
-- a few seconds.
--
-- Measured on 1.91.0-staging.2: request A is claimed and a real Jira sync starts;
-- request B lands mid-flight and nulls `claimed_at`/`completed_at`; the daemon
-- finishes and calls `complete()`, whose guard was `claimed_at IS NOT NULL` - now
-- NULL - so the outcome was DISCARDED and the sync re-run from scratch. Every waiter
-- then polled for its full 30 s budget and reported failure for a sync that had
-- actually succeeded, repeatedly.
--
-- With a sequence, "done" is a watermark rather than a flag: a producer holding seq
-- N is satisfied by any `completed_seq >= N`, so overlapping requests coalesce
-- instead of cannibalising each other, and no completion can be misattributed to a
-- request that was never serviced.
--
-- `seq` starts at 1 and `completed_seq` is NULL-means-nothing-completed, so
-- "pending" is `seq > COALESCE(completed_seq, 0)` everywhere.
ALTER TABLE pm_sync_requests ADD COLUMN seq INTEGER NOT NULL DEFAULT 1;
ALTER TABLE pm_sync_requests ADD COLUMN completed_seq INTEGER;

-- Carry the existing row's state across rather than resetting it. A row already
-- completed under 082 must NOT read as pending after this migration (that would fire
-- a spurious provider sync on the first daemon start after an update, for every
-- installed user at once); a row mid-flight must stay pending so it is still
-- serviced.
UPDATE pm_sync_requests SET completed_seq = seq WHERE completed_at IS NOT NULL;
