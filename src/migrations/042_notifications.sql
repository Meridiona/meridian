-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

-- Notification outbox. Distinct from `system_notices` (a stateful fault bus that
-- is raised/cleared): this is a transactional outbox of DISCRETE notification
-- events (plan nudge, worklog ready, a fault promoted to an OS toast). The daemon
-- enqueues a row; consumers drain it and stamp a per-channel delivered/dismissed
-- timestamp. At-least-once delivery with idempotency on `dedup_key`.
--
-- event_key  — type for preference lookup, e.g. 'plan.nudge', 'worklog.ready',
--              'system.fault'. dedup_key — the once-only key, usually
--              '<event_key>:<scope>' e.g. 'plan.nudge:2026-06-15'. Re-enqueuing
--              the same dedup_key is a no-op (the event fires exactly once).
-- channels   — CSV of delivery channels: 'native' (macOS toast via the tray),
--              'banner' (in-dashboard banner). Either or both.
CREATE TABLE notifications (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    dedup_key           TEXT NOT NULL UNIQUE,
    event_key           TEXT NOT NULL,
    severity            TEXT NOT NULL DEFAULT 'info'
                          CHECK (severity IN ('info', 'warning', 'error')),
    title               TEXT NOT NULL,
    body                TEXT NOT NULL DEFAULT '',
    deep_link           TEXT,
    channels            TEXT NOT NULL DEFAULT 'native',
    scheduled_for       TEXT,
    expires_at          TEXT,
    delivered_native_at TEXT,
    banner_dismissed_at TEXT,
    attempts            INTEGER NOT NULL DEFAULT 0,
    created_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

-- Drains the native channel: undelivered, due, unexpired rows whose channel set
-- includes 'native'. ORDER BY id keeps delivery FIFO.
CREATE INDEX idx_notifications_native_pending
    ON notifications (delivered_native_at, scheduled_for);

-- Drains the banner channel: undismissed, unexpired rows.
CREATE INDEX idx_notifications_banner_active
    ON notifications (banner_dismissed_at, expires_at);
