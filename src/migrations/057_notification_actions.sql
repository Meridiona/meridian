-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

-- Interactive notifications: action buttons + inline reply on the outbox rows,
-- and the response leg back from the user. Extends `notifications` (042) in-row,
-- matching the existing per-channel ack pattern — no new table. The outbox
-- becomes a round-trip mailbox: enqueue → deliver → respond → consume, every
-- leg idempotent.
--
--   category             — UNNotificationCategory id registered by the tray at
--                          startup (e.g. 'verify_switch'); NULL = plain toast,
--                          exactly today's behaviour.
--   actions              — JSON [{id,title,input?,destructive?,foreground?}].
--                          Informational for v1 (macOS buttons come from the
--                          registered category); carried for banner rendering
--                          and future dynamic-copy producers.
--   responded_at         — when the user answered (button press / tap / dismiss).
--   response_action      — pressed action id, or 'tap' | 'dismiss'.
--   response_text        — inline-reply text, if the action had an input field.
--   response_consumed_at — daemon-side consumer ack (it acted on the response).

ALTER TABLE notifications ADD COLUMN category TEXT;
ALTER TABLE notifications ADD COLUMN actions TEXT;
ALTER TABLE notifications ADD COLUMN responded_at TEXT;
ALTER TABLE notifications ADD COLUMN response_action TEXT;
ALTER TABLE notifications ADD COLUMN response_text TEXT;
ALTER TABLE notifications ADD COLUMN response_consumed_at TEXT;

-- The daemon's response consumer drains answered-but-unconsumed rows.
CREATE INDEX idx_notifications_responses
    ON notifications (responded_at, response_consumed_at);
