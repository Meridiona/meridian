//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
use super::*;
use sqlx::sqlite::SqlitePoolOptions;

fn settings() -> RuntimeSettings {
    RuntimeSettings::default()
}

/// In-memory pool with the columns the delivery writes touch.
async fn notif_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE notifications (\
            id INTEGER PRIMARY KEY, delivered_native_at TEXT, banner_dismissed_at TEXT, \
            attempts INTEGER NOT NULL DEFAULT 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool
}

#[tokio::test]
async fn mark_native_delivered_is_idempotent() {
    let pool = notif_pool().await;
    sqlx::query("INSERT INTO notifications (id) VALUES (1)")
        .execute(&pool)
        .await
        .unwrap();
    mark_native_delivered(&pool, 1, "2026-06-18T10:00:00Z")
        .await
        .unwrap();
    // Second ack is a no-op (the IS NULL guard) — attempts must NOT bump again.
    mark_native_delivered(&pool, 1, "2026-06-18T11:00:00Z")
        .await
        .unwrap();
    let (delivered, attempts): (Option<String>, i64) =
        sqlx::query_as("SELECT delivered_native_at, attempts FROM notifications WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(delivered.as_deref(), Some("2026-06-18T10:00:00Z"));
    assert_eq!(attempts, 1, "duplicate ack must not re-bump attempts");
}

#[tokio::test]
async fn dismiss_banner_stamps_once() {
    let pool = notif_pool().await;
    sqlx::query("INSERT INTO notifications (id) VALUES (1)")
        .execute(&pool)
        .await
        .unwrap();
    dismiss_banner(&pool, 1, "2026-06-18T10:00:00Z")
        .await
        .unwrap();
    dismiss_banner(&pool, 1, "2026-06-18T11:00:00Z")
        .await
        .unwrap();
    let stamp: Option<String> =
        sqlx::query_scalar("SELECT banner_dismissed_at FROM notifications WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stamp.as_deref(), Some("2026-06-18T10:00:00Z"));
}

#[tokio::test]
async fn active_banners_filters_channel_dismissed_expired_and_prefs() {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE notifications (\
            id INTEGER PRIMARY KEY, event_key TEXT, severity TEXT, title TEXT, body TEXT, \
            deep_link TEXT, created_at TEXT, channels TEXT, \
            banner_dismissed_at TEXT, expires_at TEXT)",
    )
    .execute(&pool)
    .await
    .unwrap();
    let now = "2026-06-18T10:00:00Z";
    // id1: banner, live → shown.  id2: native-only → excluded.
    // id3: dismissed → excluded.  id4: expired → excluded.
    // id5: banner but newer id → must sort BEFORE id1 (id DESC).
    let rows = [
        (1, "plan.nudge", "banner", None::<&str>, None::<&str>),
        (2, "plan.nudge", "native", None, None),
        (3, "plan.nudge", "banner", Some(now), None),
        (
            4,
            "plan.nudge",
            "banner",
            None,
            Some("2026-06-18T09:00:00Z"),
        ),
        (5, "plan.nudge", "banner,native", None, None),
    ];
    for (id, ek, ch, dismissed, expires) in rows {
        sqlx::query(
            "INSERT INTO notifications (id, event_key, severity, title, body, created_at, channels, banner_dismissed_at, expires_at) \
             VALUES (?, ?, 'info', 't', 'b', '2026-06-18T08:00:00Z', ?, ?, ?)",
        )
        .bind(id)
        .bind(ek)
        .bind(ch)
        .bind(dismissed)
        .bind(expires)
        .execute(&pool)
        .await
        .unwrap();
    }

    let banners = active_banners(&pool, now, &settings()).await;
    let ids: Vec<i64> = banners.iter().map(|b| b.id).collect();
    assert_eq!(
        ids,
        vec![5, 1],
        "id DESC; native/dismissed/expired excluded"
    );

    // Master switch off → nothing surfaces.
    let mut off = settings();
    off.notifications_enabled = false;
    assert!(active_banners(&pool, now, &off).await.is_empty());
}

#[test]
fn event_allowed_respects_only_the_master_switch() {
    let mut s = settings();
    assert!(event_allowed(&s));
    s.notifications_enabled = false;
    assert!(!event_allowed(&s), "master off → nothing surfaces");
}

#[test]
fn quiet_hours_same_day_and_wraparound() {
    let mut s = settings();
    // disabled → never quiet
    assert!(!in_quiet_hours_at(&s, 23 * 60));
    s.quiet_hours_enabled = true;
    // default 22:00–08:00 wraps midnight
    assert!(in_quiet_hours_at(&s, 23 * 60)); // 23:00 inside
    assert!(in_quiet_hours_at(&s, 2 * 60)); // 02:00 inside
    assert!(!in_quiet_hours_at(&s, 12 * 60)); // noon outside
    assert!(!in_quiet_hours_at(&s, 8 * 60)); // 08:00 end-exclusive → outside
    assert!(in_quiet_hours_at(&s, 22 * 60)); // 22:00 start-inclusive → inside
                                             // same-day window 09:00–17:00
    s.quiet_hours_start = "09:00".into();
    s.quiet_hours_end = "17:00".into();
    assert!(in_quiet_hours_at(&s, 12 * 60));
    assert!(!in_quiet_hours_at(&s, 8 * 60));
    // malformed bounds → fail open (not quiet)
    s.quiet_hours_start = "nope".into();
    assert!(!in_quiet_hours_at(&s, 12 * 60));
}

/// In-memory pool with the FULL post-057 column set the interactive reads
/// touch (042 outbox columns + 057 response columns).
async fn interactive_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE notifications (\
            id INTEGER PRIMARY KEY, dedup_key TEXT NOT NULL UNIQUE, event_key TEXT, \
            severity TEXT DEFAULT 'info', title TEXT, body TEXT, deep_link TEXT, \
            channels TEXT DEFAULT 'native', scheduled_for TEXT, expires_at TEXT, \
            delivered_native_at TEXT, banner_dismissed_at TEXT, \
            attempts INTEGER NOT NULL DEFAULT 0, created_at TEXT, \
            category TEXT, actions TEXT, responded_at TEXT, response_action TEXT, \
            response_text TEXT, response_consumed_at TEXT)",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool
}

#[tokio::test]
async fn record_response_first_answer_wins() {
    let pool = interactive_pool().await;
    sqlx::query(
        "INSERT INTO notifications (id, dedup_key, event_key, title, body, category) \
         VALUES (1, 'k1', 'plan.nudge', 't', 'b', 'plan_nudge')",
    )
    .execute(&pool)
    .await
    .unwrap();
    record_response(&pool, 1, "snooze", None, "2026-07-06T10:00:00Z")
        .await
        .unwrap();
    // A second (late) answer must be a no-op — first answer wins.
    record_response(&pool, 1, "open", Some("late"), "2026-07-06T11:00:00Z")
        .await
        .unwrap();
    let (at, action, text): (Option<String>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT responded_at, response_action, response_text FROM notifications WHERE id = 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(at.as_deref(), Some("2026-07-06T10:00:00Z"));
    assert_eq!(action.as_deref(), Some("snooze"));
    assert_eq!(text, None);
}

#[tokio::test]
async fn unconsumed_responses_drain_and_consume() {
    let pool = interactive_pool().await;
    for (id, key) in [(1, "k1"), (2, "k2")] {
        sqlx::query(
            "INSERT INTO notifications (id, dedup_key, event_key, title, body) \
             VALUES (?, ?, 'worklog.ready', 't', 'b')",
        )
        .bind(id)
        .bind(key)
        .execute(&pool)
        .await
        .unwrap();
    }
    // Nothing answered yet → empty queue.
    assert!(unconsumed_responses(&pool).await.is_empty());
    record_response(&pool, 2, "reply", Some("hi"), "2026-07-06T10:00:00Z")
        .await
        .unwrap();
    let q = unconsumed_responses(&pool).await;
    assert_eq!(q.len(), 1);
    assert_eq!(q[0].id, 2);
    assert_eq!(q[0].response_action, "reply");
    assert_eq!(q[0].response_text.as_deref(), Some("hi"));
    // Consume → leaves the queue; a duplicate consume is a no-op.
    mark_response_consumed(&pool, 2, "2026-07-06T10:01:00Z")
        .await
        .unwrap();
    mark_response_consumed(&pool, 2, "2026-07-06T11:00:00Z")
        .await
        .unwrap();
    assert!(unconsumed_responses(&pool).await.is_empty());
    let stamp: Option<String> =
        sqlx::query_scalar("SELECT response_consumed_at FROM notifications WHERE id = 2")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stamp.as_deref(), Some("2026-07-06T10:01:00Z"));
}

#[tokio::test]
async fn pending_native_carries_category_and_survives_pre_057() {
    // Post-057 schema: category/actions ride along.
    let pool = interactive_pool().await;
    sqlx::query(
        "INSERT INTO notifications (id, dedup_key, event_key, title, body, category, actions) \
         VALUES (1, 'k1', 'plan.nudge', 't', 'b', 'plan_nudge', '[]')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let now = "2026-07-06T10:00:00Z";
    let pending = pending_native(&pool, now, &settings()).await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].category.as_deref(), Some("plan_nudge"));

    // Pre-057 schema (042 columns only): the read degrades to category=None
    // instead of erroring the queue empty, and the response leg no-ops.
    let old = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE notifications (\
            id INTEGER PRIMARY KEY, dedup_key TEXT NOT NULL UNIQUE, event_key TEXT, \
            severity TEXT DEFAULT 'info', title TEXT, body TEXT, deep_link TEXT, \
            channels TEXT DEFAULT 'native', scheduled_for TEXT, expires_at TEXT, \
            delivered_native_at TEXT, banner_dismissed_at TEXT, \
            attempts INTEGER NOT NULL DEFAULT 0, created_at TEXT)",
    )
    .execute(&old)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO notifications (id, dedup_key, event_key, title, body) \
         VALUES (1, 'k1', 'plan.nudge', 't', 'b')",
    )
    .execute(&old)
    .await
    .unwrap();
    let pending = pending_native(&old, now, &settings()).await;
    assert_eq!(pending.len(), 1, "pre-057 read must still deliver");
    assert_eq!(pending[0].category, None);
    record_response(&old, 1, "tap", None, now).await.unwrap(); // must not error
    assert!(unconsumed_responses(&old).await.is_empty());
}

#[tokio::test]
#[allow(clippy::type_complexity)] // test fixture: a flat (id, delivered, responded, expires) table
async fn expired_unanswered_selects_only_delivered_unanswered_past_expiry() {
    let pool = interactive_pool().await;
    let now = "2026-07-06T12:00:00Z";
    // (id, delivered, responded, expires) — only id 1 qualifies.
    let rows: [(i64, Option<&str>, Option<&str>, Option<&str>); 5] = [
        (1, Some("t"), None, Some("2026-07-06T11:00:00Z")), // expired, unanswered → in
        (2, Some("t"), None, Some("2026-07-06T13:00:00Z")), // not yet expired → out
        (3, Some("t"), Some("t"), Some("2026-07-06T11:00:00Z")), // answered → out
        (4, None, None, Some("2026-07-06T11:00:00Z")),      // never delivered → out
        (5, Some("t"), None, None),                         // no expiry → out
    ];
    for (id, delivered, responded, expires) in rows {
        sqlx::query(
            "INSERT INTO notifications (id, dedup_key, event_key, title, body, \
             delivered_native_at, responded_at, expires_at) VALUES (?, ?, 'e', 't', 'b', ?, ?, ?)",
        )
        .bind(id)
        .bind(format!("k{id}"))
        .bind(delivered)
        .bind(responded)
        .bind(expires)
        .execute(&pool)
        .await
        .unwrap();
    }
    assert_eq!(expired_unanswered(&pool, now).await, vec![1]);
    // The 'expired' stamp (record_response) removes it from the queue.
    record_response(&pool, 1, "expired", None, now)
        .await
        .unwrap();
    assert!(expired_unanswered(&pool, now).await.is_empty());
}

#[test]
fn every_category_has_valid_actions_json() {
    for cat in categories::ALL {
        let json = categories::actions_json(cat)
            .unwrap_or_else(|| panic!("category {cat} missing actions"));
        let parsed: serde_json::Value = serde_json::from_str(json)
            .unwrap_or_else(|e| panic!("category {cat} actions not valid JSON: {e}"));
        let arr = parsed.as_array().expect("actions must be a JSON array");
        assert!(!arr.is_empty(), "category {cat} has no actions");
        for a in arr {
            assert!(a.get("id").is_some() && a.get("title").is_some());
        }
    }
    assert_eq!(categories::actions_json("nope"), None);
}

#[test]
fn hhmm_parser_is_strict_like_the_route_regex() {
    // Accepts 1–2 digit hours, exactly 2 digit minutes.
    assert_eq!(hhmm_to_minutes("8:00"), Some(8 * 60));
    assert_eq!(hhmm_to_minutes("08:00"), Some(8 * 60));
    assert_eq!(hhmm_to_minutes("23:59"), Some(23 * 60 + 59));
    assert_eq!(hhmm_to_minutes(" 09:30 "), Some(9 * 60 + 30)); // outer trim only
                                                               // Rejects everything the original /^(\d{1,2}):(\d{2})$/ rejected.
    assert_eq!(hhmm_to_minutes("8:5"), None); // 1-digit minutes
    assert_eq!(hhmm_to_minutes("8:0"), None);
    assert_eq!(hhmm_to_minutes("+8:00"), None); // sign
    assert_eq!(hhmm_to_minutes("8:00:00"), None); // trailing seconds
    assert_eq!(hhmm_to_minutes("24:00"), None); // hour out of range
    assert_eq!(hhmm_to_minutes("8:60"), None); // minute out of range
    assert_eq!(hhmm_to_minutes("nope"), None);
}
