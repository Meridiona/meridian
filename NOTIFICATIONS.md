# Notifications — integration guide

How to plug a new notification (plain toast, interactive nudge with buttons /
inline reply, or a response handler) into Meridian's notification service.
The infrastructure is complete and E2E-verified; producers and response
handlers are thin integrations on top of it.

---

## Architecture in one paragraph

The `notifications` table in `meridian.db` is a **transactional outbox turned
round-trip mailbox**. Producers (daemon code) `enqueue()` a row; the tray
drains the `native` channel into macOS toasts (via the community
`tauri-plugin-notifications` — real `UNUserNotificationCenter`, so toasts can
carry buttons and an inline text reply); the dashboard renders the `banner`
channel. When the user answers a toast (button, reply, tap, ✕), the answer is
stamped **onto the same row**; the daemon's response consumer acts on it and
stamps it consumed. Every leg is idempotent, so crashes and retries collapse
into no-ops:

```
enqueue ──▶ deliver ──▶ respond ──▶ consume
(daemon)     (tray)      (tray)      (daemon)
   │            │           │            │
dedup_key   delivered_   responded_at  response_
UNIQUE      native_at    + response_   consumed_at
            (IS NULL     action/text   (IS NULL
             guard)      (first answer  guard)
                          wins)
                └── expires_at passed & unanswered → toast retracted,
                    stamped response_action='expired'
```

## File map

| Piece | File |
|---|---|
| Producer API (`NewNotification`, `enqueue`, `retract`) | `src/notifications.rs` |
| Policy + reads/writes (pending, banners, responses, categories) | `meridian-core/src/notifications.rs` |
| Response consumer (acts on answers) | `src/notification_responses.rs` |
| Tray delivery + retraction facade | `tray/src-tauri/src/sys.rs` |
| Tray drain + expiry sweep (poll tick, ~30 s) | `tray/src-tauri/src/poll/notifications.rs` |
| Answer write command (`record_notification_response`) | `tray/src-tauri/src/commands/notifications.rs` |
| Toast action listener (plugin event → command) | `tray/src/app.js` (`armNotificationActions`) |
| Schema | migrations `042` (outbox) + `057` (actions/response columns) |

## Add a plain notification

One call. Dedup key encodes the once-only scope; the row fires exactly once no
matter how often the producing loop runs.

```rust
use crate::notifications::{self, NewNotification};

notifications::enqueue(pool, NewNotification::event(
    &format!("my.event:{scope}"),   // dedup: '<event_key>:<scope>'
    "my.event",                     // event_key — preference lookup
    "Title", "Body",
)
.link("/route")                     // optional dashboard deep link
.via(notifications::CHANNEL_NATIVE) // default is both channels
.expiring(&iso8601_utc)             // optional; see "Duration" below
).await?;
```

That's the whole producer. Delivery policy (master switch, per-type toggle,
quiet hours) is enforced at drain time by `meridian-core`, never by producers —
**always enqueue; the user's settings decide whether it surfaces.**

## Add an interactive notification (buttons / inline reply)

1. **Category** — add the id + button set in
   `meridian-core/src/notifications.rs::categories` (one source of truth: the
   tray registers it with macOS at startup, producers stamp it on rows).
   Action JSON shape: `[{id, title, input?, destructive?, foreground?,
   inputButtonTitle?, inputPlaceholder?}]`. macOS budget: ≤4 actions per
   category; `foreground: true` for buttons that should open the app;
   `input: true` gives an inline text field. Add the id to `categories::ALL`.

2. **Producer** — same `enqueue`, plus:

   ```rust
   .category(categories::MY_CATEGORY)
   .actions(categories::actions_json(categories::MY_CATEGORY).unwrap_or("[]"))
   .expiring(&expiry)   // interactive questions should die when stale
   ```

3. **Response handler** — add a match arm in
   `src/notification_responses.rs::consume_responses`:

   ```rust
   ("my.event", "some_action_id") => my_handler(pool, &r).await,
   ```

   `r` is a `NotificationResponse`: the full row + `response_action`
   (button id | `tap` | `dismiss` | `expired`) + `response_text` (inline
   reply). Handlers MUST be idempotent — a crash before the consume-stamp
   re-runs them next tick (see the snooze handler: its re-enqueue dedup key is
   derived from `responded_at`, so a retry is a no-op). Rows not matched by any
   arm are just stamped consumed.

4. **Per-type toggle** — add a field to `RuntimeSettings`
   (`meridian-core/src/settings.rs`), a match arm in
   `meridian-core/src/notifications.rs::type_enabled`, and the checkbox in
   `ui/components/timeline/settings/NotificationsSection.tsx`. Unknown
   event keys default to enabled, so this step gates opt-out, not delivery.

No tray changes are needed — delivery, buttons, response capture, deep-link
navigation, and expiry all key off the row's columns.

## Response semantics

- `response_action` values: an action id from the category (`open`, `snooze`,
  `yes`, …), `tap` (user clicked the toast body), `dismiss` (user pressed ✕ —
  captured because categories set `customDismissAction`), or `expired` (nobody
  answered before `expires_at`; the tray stamped it during retraction).
- `response_text` carries the inline reply for `input: true` actions.
- **First answer wins**: the write guards on `responded_at IS NULL`.
- `tap` / `open` / `view` on a row with a `deep_link` opens the dashboard
  (resolved from the DB row — never from the toast payload, see gotcha 3).
- The daemon stamps `response_consumed_at` after acting; funnel queries can
  distinguish delivered / answered / expired / consumed per event type.

## Duration & persistence ("how long does it stay?")

macOS has **no per-notification duration API**. What we have:

- App-wide style: **Alerts** (persists until acted on) vs **Banners** (~5 s).
  `tray/src-tauri/Info.plist` sets `NSUserNotificationAlertStyle=alert`, so
  new installs default to persistent. The user's System Settings choice always
  wins; installs granted before this key shipped keep Banners until flipped.
- **Per-notification lifetime is ours via `expires_at`**: an undelivered row
  past expiry never fires; a delivered-but-ignored row past expiry is
  retracted from the screen/Notification Center on the next tray tick and
  stamped `expired`. Net effect under Alerts style: `expires_at` ≈ how long
  the toast stays up, with poll-tick (~30 s) granularity. No expiry = stays
  until answered.

## Platform constraints & plugin gotchas (hard-won — do not rediscover)

1. `tauri-plugin-notifications` is pinned `=0.4.6` with
   **`default-features = false`** — the default `notify-rust` feature silently
   replaces the Swift layer (no buttons, no events).
2. The Swift layer needs `-Wl,-rpath,/usr/lib/swift` (tray `build.rs`) or the
   packaged app dies at launch on `libswift_Concurrency.dylib`.
3. **Extras don't round-trip**: the toast's notification identifier (= outbox
   row id) is the only correlation that survives `actionPerformed`. Resolve
   everything else from the DB row.
4. The plugin's init **hard-fails outside a `.app` bundle** — it is registered
   only when bundled; `tauri dev` runs have no toasts at all. Interactive
   behaviour is testable only in packaged builds.
5. Action events only fire for toasts shown by the **current tray process**
   (in-memory map) — an answer after a tray restart is dropped; expiry cleans
   those up.
6. Banners collapse buttons behind a hover "Options" affordance; that's macOS.

## Testing a new notification

Unit: core reads/writes have in-module tests (`meridian-core/src/notifications.rs`);
consumer handlers get tests beside the snooze pattern. End-to-end needs a
packaged build:

```bash
cd tray && npm run build         # signs with scripts/dev-signing.sh identity
# install target/release/bundle/macos/Meridian.app, then seed:
sqlite3 ~/.meridian/meridian.db "INSERT INTO notifications
  (dedup_key,event_key,title,body,channels,category,deep_link,expires_at)
  VALUES ('test:1','my.event','T','B','native','verify_switch','/plan',
          datetime('now','+2 minutes'))"
# within one tray tick (~30s): toast with buttons; answer it, then:
sqlite3 ~/.meridian/meridian.db "SELECT response_action,response_text,
  responded_at,response_consumed_at FROM notifications WHERE dedup_key='test:1'"
```

Debugging: launch the installed app from a shell —
`RUST_LOG=info /Applications/Meridian.app/Contents/MacOS/meridian-tray` — the
popover forwards listener/payload traces via `tray_debug`, so the whole
Swift → Rust → JS chain is visible in stderr. Look for
`notification categories registered`, `notification action listener
registered`, `outbox toast delivered`, `notification action event: {...}`,
`notification response recorded`, `notification responses consumed`.

## Current producers & categories

| event_key | category | buttons | consumer arm |
|---|---|---|---|
| `plan.nudge` | `plan_nudge` | Open Plan · Snooze 1h | snooze → re-enqueue +1h |
| `worklog.ready` | `worklog_ready` | Open Worklogs · Snooze 1h | snooze → re-enqueue +1h |
| `system.fault` | `system_fault` | View | — (stamp only) |
| *(reserved)* | `verify_switch` | Yes · No · Reply… | — (PR 2: task-switch verification) |
| *(generic)* | `generic_link` | Open | — (stamp only) |

Deferred (same rails, thin slices when needed): task-switch verify producer +
rate cap + `notify_task_verify` toggle (PR 2), goal check-in / distraction
producers, LLM-generated copy (needs an MLX runtime republish), APNs/phone
delivery, folding the direct `sys::notify` bypass callers (pause/update/health
toasts) into the outbox.
