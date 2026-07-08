//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Interactive-notification response consumer — the daemon half of the outbox's
// response leg (migration 057). The tray records the user's answer on the row
// (`meridian_core::notifications::record_response`); every poll tick this reads
// the answered-but-unconsumed rows, acts on them, and stamps
// `response_consumed_at`. Consumption is idempotent end-to-end: a snooze
// re-enqueue keys its dedup on the original row's `responded_at`, so a crash
// between enqueue and the consume-stamp retries into a no-op next tick.
//
// V1 handlers:
//   plan.nudge / worklog.ready + 'snooze' → re-enqueue the same event with a
//     fresh dedup suffix, scheduled an hour out.
//   everything else ('tap', 'dismiss', 'open', 'view', unknown) → just stamp
//     consumed — navigation already happened at delivery time in the tray.

use anyhow::Result;
use sqlx::SqlitePool;

use crate::notifications::{self, NewNotification};
use meridian_core::notifications::{
    mark_response_consumed, unconsumed_responses, NotificationResponse,
};

/// How far out a 'snooze' answer reschedules the event.
const SNOOZE_SECS: i64 = 3600;

/// Drain the response queue: dispatch each answered row, then ack it. A failed
/// handler leaves its row unconsumed (retried next tick); a failed ack is
/// logged and retried next tick too — the dedup keys make both retries no-ops.
#[tracing::instrument(skip(pool))]
pub async fn consume_responses(pool: &SqlitePool) -> Result<()> {
    let responses = unconsumed_responses(pool).await;
    if responses.is_empty() {
        return Ok(());
    }
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut consumed = 0usize;
    for r in responses {
        let handled = match (r.event_key.as_str(), r.response_action.as_str()) {
            ("plan.nudge" | "worklog.ready", "snooze") => snooze(pool, &r).await,
            // 'tap' / 'dismiss' / foreground buttons — the tray already routed
            // any navigation at delivery time; nothing to do daemon-side.
            _ => Ok(()),
        };
        if let Err(e) = handled {
            tracing::warn!(
                error = %e,
                id = r.id,
                event_key = %r.event_key,
                action = %r.response_action,
                "notification response handler failed — will retry next tick"
            );
            continue;
        }
        if let Err(e) = mark_response_consumed(pool, r.id, &now).await {
            tracing::warn!(error = %e, id = r.id, "response consume-ack failed");
            continue;
        }
        consumed += 1;
    }
    tracing::info!(consumed, "notification responses consumed");
    Ok(())
}

/// Re-enqueue the answered event one hour out. The dedup key extends the
/// original with the response stamp, so (a) it never collides with the original
/// row's key and (b) re-processing the same response is a no-op.
#[tracing::instrument(skip(pool, r), fields(id = r.id, event_key = %r.event_key))]
async fn snooze(pool: &SqlitePool, r: &NotificationResponse) -> Result<()> {
    let scheduled_for = (chrono::Utc::now() + chrono::Duration::seconds(SNOOZE_SECS))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let dedup = format!("{}:snooze:{}", r.dedup_key, r.responded_at);
    notifications::enqueue(
        pool,
        NewNotification {
            dedup_key: &dedup,
            event_key: &r.event_key,
            severity: &r.severity,
            title: &r.title,
            body: &r.body,
            deep_link: r.deep_link.as_deref(),
            channels: &r.channels,
            scheduled_for: Some(&scheduled_for),
            expires_at: None,
            category: r.category.as_deref(),
            actions: r.actions.as_deref(),
        },
    )
    .await?;
    tracing::info!(dedup_key = %dedup, scheduled_for = %scheduled_for, "snoozed notification re-enqueued");
    Ok(())
}
