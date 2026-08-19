-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

-- The persistent fault banner (`system_notices`, read by `NoticeBar.tsx`) had no
-- click-through target at all — only the transactional `notifications` outbox
-- (migration 042) carries `deep_link`, which only ever reaches the transient
-- native OS toast. A banner that outlives its toast (the common case: a toast
-- disappears in seconds, a banner sits until the fault clears) had nowhere to
-- send a click. `raise_typed` already threads a `deep_link` through to the paired
-- toast; this lets it persist the same value onto the banner row too.
--
-- Nullable, no default: a notice with no deep link (most of them) simply renders
-- with no button, exactly as today.
ALTER TABLE system_notices ADD COLUMN deep_link TEXT;
