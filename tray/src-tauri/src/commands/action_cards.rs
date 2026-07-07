//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Dismiss/snooze commands for the standardized action-card stack (must-fix /
//! cleanup / drafts) — no route to port, new work alongside migration 057.
//!
//! Each write resolves the per-kind snooze window here (mirrors
//! [`crate::commands::triage::triage_decision`]'s snooze-expiry pattern) so
//! [`meridian_core::action_card_snooze`] stays deterministic/testable.
//!
//! # Who calls this
//! `ui/components/timeline/useTimelineData.ts`'s `actionCardSnoozes` state +
//! `dismissActionCard` action, consumed by `useActionItems` to filter the
//! stack and `ActionCard`'s dismiss (×) button.
//!
//! # Related
//! - [`meridian_core::action_card_snooze`] — the DB read/write this wraps.
//! - [`crate::commands::triage`] — sibling ticket-level snooze, same shape.

use serde::{Deserialize, Serialize};
use tauri::State;

/// Seconds-precision UTC RFC3339, matching `triage`'s `now_iso`.
fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Per-kind dismiss window. Must-fix is shortest (it blocks tracking accuracy
/// — resurface soon); cleanup is longest (low urgency, naggy if frequent);
/// drafts sit in between (same-day resurface). See the "Attention surfaces"
/// design doc for the reasoning.
fn snooze_window_hours(card_kind: &str) -> i64 {
    match card_kind {
        "mustfix" => 24,
        "cleanup" => 24 * 3,
        "drafts" => 4,
        _ => 24,
    }
}

/// The dismissed-kinds read (no route — new work). Resolves `now` here so the
/// core fn stays deterministic; returns only still-active snoozes.
#[tauri::command]
#[tracing::instrument(skip(pool))]
pub async fn get_action_card_snoozes(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
) -> Result<Vec<meridian_core::action_card_snooze::ActionCardSnooze>, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    meridian_core::action_card_snooze::get_active_snoozes(pool, &now_iso())
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "get_action_card_snoozes failed");
            e.to_string()
        })
}

/// POST body for [`snooze_action_card`] (`{ card_kind }`).
#[derive(Debug, Deserialize)]
pub struct SnoozeActionCardBody {
    pub card_kind: String,
}

/// Ack for [`snooze_action_card`] — echoes the resolved expiry.
#[derive(Debug, Serialize)]
pub struct SnoozeActionCardAck {
    pub ok: bool,
    pub card_kind: String,
    pub snoozed_until: String,
}

/// Dismiss a card kind (the `×` on `ActionCard`) — resolves `now +
/// snooze_window_hours(kind)` and upserts `action_card_snooze`.
#[tauri::command]
#[tracing::instrument(skip(pool, body), fields(card_kind = %body.card_kind))]
pub async fn snooze_action_card(
    pool: State<'_, Option<meridian_core::SqlitePool>>,
    body: SnoozeActionCardBody,
) -> Result<SnoozeActionCardAck, String> {
    let Some(pool) = pool.inner() else {
        return Err("meridian.db is not open yet".to_string());
    };
    if body.card_kind.is_empty() {
        return Err("card_kind required".to_string());
    }
    let hours = snooze_window_hours(&body.card_kind);
    let snoozed_until = (chrono::Utc::now() + chrono::Duration::hours(hours))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    meridian_core::action_card_snooze::snooze_card(pool, &body.card_kind, &snoozed_until)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "snooze_action_card failed");
            e.to_string()
        })?;

    Ok(SnoozeActionCardAck {
        ok: true,
        card_kind: body.card_kind,
        snoozed_until,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snooze_windows_match_the_design_doc() {
        assert_eq!(snooze_window_hours("mustfix"), 24);
        assert_eq!(snooze_window_hours("cleanup"), 72);
        assert_eq!(snooze_window_hours("drafts"), 4);
        // Unknown kind falls back to the conservative (shortest-of-the-longer) window.
        assert_eq!(snooze_window_hours("bogus"), 24);
    }
}
