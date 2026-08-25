//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Centralised fault bus. The daemon calls `raise` when something breaks and
// `clear` when it recovers. The UI reads `system_notices` via the SSE stream
// and surfaces banners on every page — users never have to check terminal logs.
//
// Notice IDs follow the pattern `<subsystem>.<fault>`, e.g.:
//   pm.jira       — Jira sync failing
//   pm.linear     — Linear sync failing
//   etl.failed    — ETL pipeline error

use anyhow::{Context, Result};
use sqlx::SqlitePool;
use tracing::Instrument;

/// Notice id raised when `meridian.db` is structurally damaged.
///
/// Lives here rather than beside the raiser in `main.rs` because THREE parties
/// must agree on it byte-for-byte: the daemon raises it (latched - see the
/// usage-site doc in `main.rs`), the UI banners it, and `db::repair` clears it
/// from the rebuilt file so the banner does not survive its own fix. It is
/// also its own `event_key` (not the shared `system.fault`), so clearing must
/// pass it for both.
pub const DB_CORRUPT: &str = "db.corrupt";

/// Raise (or refresh) a named notice. Idempotent — upserts so repeated calls
/// from the poll loop don't accumulate duplicate rows. The paired toast is
/// always stamped `system.fault` — use [`raise_typed`] when the caller needs
/// its own distinct `event_key` instead of sharing the `system.fault` bucket.
pub async fn raise(
    pool: &SqlitePool,
    id: &str,
    severity: &str,
    title: &str,
    detail: &str,
    remedy: Option<&str>,
) -> Result<()> {
    // A tracker fault is fixed by reconnecting the tracker, which is what this
    // notice's own `remedy` tells the user ("Reconnect X in Settings →
    // Integrations") — so the click-through goes there. It used to point at
    // `/tasks?integrations=1`, the pre-fold Next route, which the shell stopped
    // resolving when that route was deleted and silently opened the default
    // view instead. Everything else has no specific destination, so it goes to
    // the Support ID / Export Diagnostics section (see `deep_links::LOGS`).
    use meridian_core::notifications::deep_links;
    let link = if id.starts_with("pm.") {
        Some(deep_links::INTEGRATIONS)
    } else {
        Some(deep_links::LOGS)
    };
    raise_typed(
        pool,
        Notice {
            id,
            severity,
            title,
            detail,
            remedy,
            event_key: "system.fault",
            deep_link: link,
        },
    )
    .await
}

/// A notice to raise via [`raise_typed`] — grouped into a struct so callers
/// read clearly and to stay under clippy's argument limit (see `BlockBounds`
/// in `src/etl/runner.rs` for the same convention elsewhere in this repo).
pub struct Notice<'a> {
    pub id: &'a str,
    /// `info` | `warning` | `error`.
    pub severity: &'a str,
    pub title: &'a str,
    pub detail: &'a str,
    pub remedy: Option<&'a str>,
    /// The paired toast's event_key, distinct from the shared `system.fault`
    /// bucket [`raise`] uses.
    pub event_key: &'a str,
    pub deep_link: Option<&'a str>,
}

/// Raise (or refresh) a named notice whose paired toast is stamped with
/// `n.event_key` instead of the shared `system.fault` bucket [`raise`] uses.
/// Same upsert/idempotency semantics as `raise`; pair with [`clear_typed`]
/// using the same `event_key` so the retract dedup key matches the enqueue.
pub async fn raise_typed(pool: &SqlitePool, n: Notice<'_>) -> Result<()> {
    sqlx::query(
        "INSERT INTO system_notices (notice_id, severity, title, detail, remedy, deep_link)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(notice_id) DO UPDATE SET
           severity  = excluded.severity,
           title     = excluded.title,
           detail    = excluded.detail,
           remedy    = excluded.remedy,
           deep_link = excluded.deep_link,
           raised_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')",
    )
    .bind(n.id)
    .bind(n.severity)
    .bind(n.title)
    .bind(n.detail)
    .bind(n.remedy)
    .bind(n.deep_link)
    .execute(pool)
    .await
    .context("raising system notice")?;

    // Promote the fault to an OS-level toast (the dashboard banner already comes
    // from this table, so the notification is native-only to avoid a double
    // banner). Best-effort: never let notification delivery break the fault-bus
    // write.
    let dedup = toast_dedup_key(n.event_key, n.id);
    // Native-only (the dashboard banner already comes from this table).
    // `.interactive()` pairs category + actions from the one source of truth, so
    // the [View] button set can't drift from the other producers.
    use meridian_core::notifications::categories;
    let mut note =
        crate::notifications::NewNotification::event(&dedup, n.event_key, n.title, n.detail)
            .via(crate::notifications::CHANNEL_NATIVE)
            .interactive(categories::SYSTEM_FAULT);
    note.severity = n.severity;
    note.deep_link = n.deep_link;
    let _ = crate::notifications::enqueue(pool, note).await;
    Ok(())
}

/// The dedup key for a notice's paired toast.
///
/// For most faults this is `<event_key>:<id>` — one toast, ever, until the fault
/// clears and the row is retracted.
///
/// A TRACKER fault (`pm.*`) additionally carries the local date, so it toasts
/// again once a day for as long as it lasts. That is a deliberate exception, and
/// it is there because "notify once" quietly failed the case it most needed to
/// handle. A dead OAuth grant is not a condition that resolves itself: it stays
/// until the user reconnects, and until they do, every hour of their work goes
/// unlogged. A single toast on day one is trivially missed — dismissed while
/// busy, fired during a meeting, delivered to a machine that was locked. One
/// tester's Jira stayed dead for five days with a banner and a toast both
/// technically present the whole time.
///
/// Daily, not hourly: this is a nag with a real remedy attached, and it has to
/// stay on the right side of the line between "reminder" and "the thing that
/// made me turn notifications off".
fn toast_dedup_key(event_key: &str, id: &str) -> String {
    if id.starts_with("pm.") {
        format!("{event_key}:{id}:{}", meridian_core::date::today_string())
    } else {
        format!("{event_key}:{id}")
    }
}

/// Clear a notice raised via [`raise`] — called when the daemon recovers from
/// a fault.
pub async fn clear(pool: &SqlitePool, id: &str) -> Result<()> {
    clear_typed(pool, id, "system.fault").await
}

/// Clear a notice raised via [`raise_typed`] with the matching `event_key`.
pub async fn clear_typed(pool: &SqlitePool, id: &str, event_key: &str) -> Result<()> {
    clear_typed_reporting(pool, id, event_key).await?;
    Ok(())
}

/// Same as [`clear_typed`], but reports whether a notice actually existed and
/// was deleted. Used by callers that must tell "a real fault was cleared"
/// apart from "there was nothing to clear" — e.g. `refresh_health`'s
/// startup reconciliation, which clears a `tray.daemon_quiet` notice that may
/// be stale from a *previous* process instance (a fresh tray process can
/// never observe that instance's down→up transition itself) and should only
/// fire the "back online" toast when something was actually there.
pub async fn clear_typed_reporting(pool: &SqlitePool, id: &str, event_key: &str) -> Result<bool> {
    let mut conn = pool.acquire().await.context("acquiring for clear")?;
    clear_typed_on(&mut conn, id, event_key).await
}

/// [`clear_typed_reporting`] for callers holding a single `SqliteConnection`
/// rather than a pool.
///
/// Deletes the `system_notices` row for `id` and best-effort retracts the
/// paired notification keyed `{event_key}:{id}`. Returns `true` only when the
/// `system_notices` row existed and was deleted — the paired notification
/// delete is best-effort (logged, never propagated) and does not affect the
/// return value.
///
/// Exists for `db::repair`'s rebuild, which must run EVERY write on one
/// explicitly-closed connection: a pooled acquire there is handed back to the
/// pool from a spawned task on drop, `pool.close()` does not reliably wait for
/// that hand-back, and the surviving connection holds the WAL open — tripping
/// the rebuild's "un-checkpointed write-ahead log" guard and failing the whole
/// repair (intermittently, under load). See the closing comments in
/// `db::repair::rebuild::build_replacement`.
pub async fn clear_typed_on(
    conn: &mut sqlx::SqliteConnection,
    id: &str,
    event_key: &str,
) -> Result<bool> {
    let result = sqlx::query("DELETE FROM system_notices WHERE notice_id = ?")
        .bind(id)
        .execute(&mut *conn)
        .instrument(tracing::debug_span!("notices.clear.system_notices"))
        .await
        .context("clearing system notice")?;
    // Retract the paired toast so a future re-occurrence of this fault notifies
    // again instead of being deduped away. Matches BOTH dedup-key shapes
    // `toast_dedup_key` can produce: the bare `<event_key>:<id>`, and the
    // date-suffixed rows a `pm.*` fault accumulates one per day. Missing the
    // suffixed ones would leave an undelivered toast for today's date sitting in
    // the outbox after the user has already fixed the fault.
    //
    // `\` escapes LIKE's own wildcards so an id containing `_` (which LIKE reads
    // as "any single character") cannot widen the match to a different notice.
    if let Err(e) =
        sqlx::query("DELETE FROM notifications WHERE dedup_key = ? OR dedup_key LIKE ? ESCAPE '\\'")
            .bind(format!("{event_key}:{id}"))
            .bind(format!("{}:{}:%", like_escape(event_key), like_escape(id)))
            .execute(&mut *conn)
            .instrument(tracing::debug_span!("notices.clear.notifications"))
            .await
    {
        tracing::warn!(id, event_key, error = %e, "notices: best-effort notification retract failed");
    }
    Ok(result.rows_affected() > 0)
}

/// Escape SQL `LIKE`'s wildcards (`%`, `_`) and the escape character itself, so
/// a literal id can be used as a prefix pattern without matching more than it
/// should. Paired with `ESCAPE '\'` at the call site.
fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// This notice's id — shared with its own `event_key` (see [`raise_typed`]'s doc on that
/// pairing), so both this module and any other caller name it identically.
pub const GROQ_DEPRECATED: &str = "llm.groq_deprecated";

/// Raise or clear [`GROQ_DEPRECATED`] to match whether Groq is the currently active custom
/// provider — the single source of truth for this banner's condition, called from TWO
/// places that must never answer differently:
///
/// - the daemon's poll tick (`main.rs`), every ~60 s, the steady-state driver;
/// - the tray's `update_settings` command, the INSTANT `llm_provider`/`llm_provider_custom_id`
///   changes — mirroring `push_health_update`'s existing "don't make a fresh pick wait out
///   the poll interval" fix for `llm_provider_ok`. Before this existed, switching away from
///   Groq left the banner up for up to a minute after the fix had already taken effect,
///   which reads as "did that actually work?" at the exact moment a user is checking.
///
/// Idempotent either way (upsert / delete-if-present), so calling it from both places on the
/// same transition is harmless — whichever call lands first does the work, the other is a
/// no-op.
#[tracing::instrument(skip(pool))]
pub async fn sync_groq_deprecated_notice(pool: &SqlitePool, active_vendor: Option<&str>) {
    if active_vendor == Some("groq") {
        if let Err(e) = raise_typed(
            pool,
            Notice {
                id: GROQ_DEPRECATED,
                severity: "error",
                title: "Groq is disabled",
                detail: "Groq's free tier can't reliably serve Meridian's \
                    hourly summaries, so Meridian no longer sends it any \
                    requests. Hourly summaries are paused until you switch \
                    providers.",
                remedy: Some("Add a free Ollama key in Settings - Intelligence to resume."),
                event_key: GROQ_DEPRECATED,
                deep_link: Some(meridian_core::notifications::deep_links::INTELLIGENCE),
            },
        )
        .await
        {
            tracing::warn!(
                error = %crate::errors::chain(&e),
                active_vendor,
                "notices: failed to raise the Groq-deprecated notice"
            );
        }
    } else if let Err(e) = clear_typed(pool, GROQ_DEPRECATED, GROQ_DEPRECATED).await {
        tracing::warn!(
            error = %crate::errors::chain(&e),
            active_vendor,
            "notices: failed to clear the Groq-deprecated notice"
        );
    }
}

#[cfg(test)]
mod tests {

    /// A tracker fault must produce a NEW dedup key each local day, so the toast
    /// fires again for a fault that is still unresolved. Everything else keeps
    /// the once-only key — a daily nag is right for "reconnect Jira", which no
    /// amount of waiting fixes, and wrong for faults the daemon may clear on its
    /// own.
    #[test]
    fn only_tracker_faults_get_a_daily_toast_key() {
        let today = meridian_core::date::today_string();
        assert_eq!(
            super::toast_dedup_key("system.fault", "pm.jira"),
            format!("system.fault:pm.jira:{today}")
        );
        assert_eq!(
            super::toast_dedup_key("system.fault", "db.corrupt"),
            "system.fault:db.corrupt"
        );
        assert_eq!(
            super::toast_dedup_key("system.fault", "etl.failed"),
            "system.fault:etl.failed"
        );
    }

    /// The clear path deletes by LIKE prefix, so an id carrying a LIKE wildcard
    /// must not widen the match onto a neighbouring notice. `_` is the dangerous
    /// one: unescaped, a `pm_x` id would also match `pm.x`.
    #[test]
    fn like_escape_neutralises_wildcards() {
        assert_eq!(super::like_escape("pm.jira"), "pm.jira");
        assert_eq!(super::like_escape("pm_jira"), r"pm\_jira");
        assert_eq!(super::like_escape("a%b"), r"a\%b");
        assert_eq!(super::like_escape(r"a\b"), r"a\\b");
    }
    use super::*;
    use sqlx::sqlite::SqliteConnectOptions;
    use std::str::FromStr;

    async fn fresh_db() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!("src/migrations").run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn clear_typed_reporting_is_true_only_when_a_row_actually_existed() {
        let pool = fresh_db().await;

        // Nothing raised yet — clearing is a no-op, reported as such.
        let cleared_nothing = clear_typed_reporting(&pool, "tray.daemon_quiet", "system.health")
            .await
            .unwrap();
        assert!(!cleared_nothing);

        raise_typed(
            &pool,
            Notice {
                id: "tray.daemon_quiet",
                severity: "warning",
                title: "Meridian went quiet.",
                detail: "Tap to check what happened.",
                remedy: None,
                event_key: "system.health",
                deep_link: Some(meridian_core::notifications::deep_links::LOGS),
            },
        )
        .await
        .unwrap();

        // A real notice exists — clearing it reports true, exactly once.
        let cleared_real = clear_typed_reporting(&pool, "tray.daemon_quiet", "system.health")
            .await
            .unwrap();
        assert!(cleared_real);

        let cleared_again = clear_typed_reporting(&pool, "tray.daemon_quiet", "system.health")
            .await
            .unwrap();
        assert!(!cleared_again);
    }

    /// THE bug this test exists for: `deep_link` reached the paired native toast
    /// (`notifications.deep_link`, migration 042) but was silently dropped before it ever
    /// reached the persistent banner (`system_notices` had no such column at all) - so a
    /// banner that outlived its toast, which is the common case, had no click-through left.
    /// Asserts on `system_notices` directly, via the same reader the banner actually uses
    /// (`meridian_core::notices::read_notices`), not just on what `raise_typed` was asked to
    /// do - a regression here could pass a Rust-only check on the query and still ship a
    /// banner with no button if the reader's column list drifted.
    #[tokio::test]
    async fn raise_typed_persists_deep_link_onto_the_banner_row() {
        let pool = fresh_db().await;

        raise_typed(
            &pool,
            Notice {
                id: "llm.groq_deprecated",
                severity: "error",
                title: "Groq is disabled",
                detail: "Switch providers to resume.",
                remedy: None,
                event_key: "llm.groq_deprecated",
                deep_link: Some(meridian_core::notifications::deep_links::INTELLIGENCE),
            },
        )
        .await
        .unwrap();

        let notices = meridian_core::notices::read_notices(&pool).await;
        let banner = notices
            .iter()
            .find(|n| n.notice_id == "llm.groq_deprecated")
            .expect("raise_typed must have written the banner row");
        assert_eq!(
            banner.deep_link.as_deref(),
            Some(meridian_core::notifications::deep_links::INTELLIGENCE)
        );

        // A re-raise (the poll loop's idempotent every-tick call) must not clear it either -
        // the UPDATE branch of the upsert has its own column list to keep in sync.
        raise_typed(
            &pool,
            Notice {
                id: "llm.groq_deprecated",
                severity: "error",
                title: "Groq is disabled",
                detail: "Switch providers to resume.",
                remedy: None,
                event_key: "llm.groq_deprecated",
                deep_link: Some(meridian_core::notifications::deep_links::INTELLIGENCE),
            },
        )
        .await
        .unwrap();
        let notices = meridian_core::notices::read_notices(&pool).await;
        let banner = notices
            .iter()
            .find(|n| n.notice_id == "llm.groq_deprecated")
            .unwrap();
        assert_eq!(
            banner.deep_link.as_deref(),
            Some(meridian_core::notifications::deep_links::INTELLIGENCE)
        );
    }

    /// THE function two different processes (the daemon's poll tick and the tray's
    /// `update_settings`) both call and must agree on. Exercises both directions plus the
    /// idempotent double-call each caller relies on (the daemon re-syncs every tick; the tray
    /// syncs once on a settings write — either could run twice for the same state).
    #[tokio::test]
    async fn sync_groq_deprecated_notice_raises_for_groq_and_clears_for_everything_else() {
        let pool = fresh_db().await;

        sync_groq_deprecated_notice(&pool, Some("groq")).await;
        let notices = meridian_core::notices::read_notices(&pool).await;
        assert!(notices.iter().any(|n| n.notice_id == GROQ_DEPRECATED));

        // Re-syncing the SAME state (the daemon's next tick, or a redundant tray call) must
        // not error or duplicate the row.
        sync_groq_deprecated_notice(&pool, Some("groq")).await;
        let notices = meridian_core::notices::read_notices(&pool).await;
        assert_eq!(
            notices
                .iter()
                .filter(|n| n.notice_id == GROQ_DEPRECATED)
                .count(),
            1
        );

        // Switching to Ollama (or any other vendor) clears it immediately - no dependence on
        // a poll interval.
        sync_groq_deprecated_notice(&pool, Some("ollama")).await;
        let notices = meridian_core::notices::read_notices(&pool).await;
        assert!(!notices.iter().any(|n| n.notice_id == GROQ_DEPRECATED));

        // `None` (a CLI provider, or nothing configured) clears it too.
        sync_groq_deprecated_notice(&pool, Some("groq")).await;
        sync_groq_deprecated_notice(&pool, None).await;
        let notices = meridian_core::notices::read_notices(&pool).await;
        assert!(!notices.iter().any(|n| n.notice_id == GROQ_DEPRECATED));
    }

    /// A tracker fault toasts once per day, so by the time the user fixes it
    /// there may be several days' rows in the outbox — including one for today
    /// that has not been delivered yet. Clearing must take all of them: leaving
    /// today's behind means the user reconnects Jira and is then told, minutes
    /// later, that Jira sync is failing.
    #[tokio::test]
    async fn clearing_a_tracker_fault_retracts_every_day_it_toasted() {
        let pool = fresh_db().await;

        // Today's row, via the real producer.
        raise_typed(
            &pool,
            Notice {
                id: "pm.jira",
                severity: "error",
                title: "Jira sync failing",
                detail: "auth failed",
                remedy: Some("Reconnect Jira in Settings - Integrations"),
                event_key: "system.fault",
                deep_link: Some(meridian_core::notifications::deep_links::INTEGRATIONS),
            },
        )
        .await
        .unwrap();

        // Two earlier days, as the outbox would already hold them.
        for day in ["2026-08-20", "2026-08-21"] {
            crate::notifications::enqueue(
                &pool,
                crate::notifications::NewNotification::event(
                    &format!("system.fault:pm.jira:{day}"),
                    "system.fault",
                    "Jira sync failing",
                    "auth failed",
                ),
            )
            .await
            .unwrap();
        }

        // A different tracker must not be swept up by the prefix match.
        crate::notifications::enqueue(
            &pool,
            crate::notifications::NewNotification::event(
                "system.fault:pm.linear:2026-08-21",
                "system.fault",
                "Linear sync failing",
                "unrelated",
            ),
        )
        .await
        .unwrap();

        let jira_before: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM notifications WHERE dedup_key LIKE 'system.fault:pm.jira%'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(jira_before, 3, "today plus the two backdated rows");

        let mut conn = pool.acquire().await.unwrap();
        clear_typed_on(&mut conn, "pm.jira", "system.fault")
            .await
            .unwrap();
        drop(conn);

        let jira_after: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM notifications WHERE dedup_key LIKE 'system.fault:pm.jira%'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(jira_after, 0, "every day's toast must be retracted");

        let linear_after: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM notifications WHERE dedup_key = 'system.fault:pm.linear:2026-08-21'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(linear_after, 1, "another tracker's toast must survive");
    }

    /// The test above only checks `system_notices` — it would pass even if
    /// `clear_typed_on`'s second DELETE (the paired notification retract)
    /// were broken or missing entirely. Exercises `clear_typed_on` directly,
    /// since that is the connection-scoped API `db::repair::rebuild` actually
    /// calls, and asserts on the `notifications` table itself: the matching
    /// `dedup_key` row is gone, and an unrelated one is untouched — proving
    /// the DELETE is scoped to `{event_key}:{id}`, not a blanket wipe.
    #[tokio::test]
    async fn clear_typed_on_retracts_only_the_matching_notification() {
        let pool = fresh_db().await;

        raise_typed(
            &pool,
            Notice {
                id: "tray.daemon_quiet",
                severity: "warning",
                title: "Meridian went quiet.",
                detail: "Tap to check what happened.",
                remedy: None,
                event_key: "system.health",
                deep_link: Some(meridian_core::notifications::deep_links::LOGS),
            },
        )
        .await
        .unwrap();

        // Unrelated dedup key — must survive the clear below.
        crate::notifications::enqueue(
            &pool,
            crate::notifications::NewNotification::event(
                "system.fault:other.fault",
                "system.fault",
                "Unrelated fault",
                "should not be touched",
            ),
        )
        .await
        .unwrap();

        let matching = "system.health:tray.daemon_quiet";
        let matching_before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM notifications WHERE dedup_key = ?")
                .bind(matching)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            matching_before, 1,
            "raise_typed must have enqueued the paired toast"
        );

        let mut conn = pool.acquire().await.unwrap();
        let cleared = clear_typed_on(&mut conn, "tray.daemon_quiet", "system.health")
            .await
            .unwrap();
        drop(conn);
        assert!(cleared);

        let matching_after: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM notifications WHERE dedup_key = ?")
                .bind(matching)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            matching_after, 0,
            "clear_typed_on must retract the paired notification, not just system_notices"
        );

        let unrelated: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM notifications WHERE dedup_key = ?")
                .bind("system.fault:other.fault")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(unrelated, 1, "an unrelated dedup key must not be touched");
    }
}
