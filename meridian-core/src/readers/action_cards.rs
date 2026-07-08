//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Action-card counts — the Rust mirror of `ui/components/timeline/useActionItems.ts`.
//!
//! The dashboard assembles a priority-ordered stack of action cards (must-fix →
//! cleanup → drafts) in the Overview panel; the tray's menu-bar attention dot
//! must light exactly when that stack would be non-empty, even while the
//! dashboard webview is closed. The webview can't drive the tray in that case,
//! so this recomputes the same three counts in the tray's background poll.
//!
//! **This is a mirror — it MUST stay in lockstep with `useActionItems.ts`.** The
//! must-fix / cleanup tiers are split DISJOINTLY off the same per-ticket
//! [`crate::hygiene::Hygiene`] verdict (a ticket lands in exactly one tier);
//! connected-provider filtering mirrors `filterByConnectedProviders` (unknown
//! providers pass through, known-but-disconnected are dropped); drafts mirror
//! `isPending`. The snooze filter is applied by the caller against
//! [`crate::action_card_snooze::get_active_snoozes`] — kept out of here so this
//! stays a pure DB counter (and the daemon's `board.mustfix` producer can reuse
//! the same counts without re-deciding snoozes).
//!
//! # Who calls this
//! `tray/src-tauri/src/poll/refresh.rs` (`refresh_attention`) → this → the
//! `AppState.attention` flag the menu-bar icon renders a dot from.
//!
//! # Related
//! - [`crate::tasks::get_tasks`] — the per-ticket hygiene + provider source.
//! - [`crate::worklogs::get_worklogs`] — the drafts source.
//! - [`crate::action_card_snooze`] — the per-card dismissals the caller applies.

use crate::hygiene::Hygiene;
use crate::readers::{tasks, worklogs};
use crate::SqlitePool;

/// The five trackers Meridian knows how to connect. A task whose `provider` is
/// one of these counts only when that tracker is connected; a task with any
/// other provider string passes through (mirrors `filterByConnectedProviders`'s
/// `!(t.provider in int)` pass-through for future/unknown providers).
const KNOWN_PROVIDERS: [&str; 5] = ["jira", "linear", "github", "trello", "azure_devops"];

/// The three action-card tiers' current counts. All zero ⇒ the stack is empty
/// ⇒ the attention dot is off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActionCardCounts {
    /// Tickets missing must-have info (blocks accurate tracking).
    pub mustfix: usize,
    /// Remaining cleanup-worthy tickets (disjoint from `mustfix`).
    pub cleanup: usize,
    /// Worklog drafts pending review (`isPending`).
    pub drafts: usize,
}

/// True when at least one tier warrants an action card. NOTE: this is the raw
/// count; the caller still masks per-tier snoozes before deciding the dot.
impl ActionCardCounts {
    pub fn any(&self) -> bool {
        self.mustfix > 0 || self.cleanup > 0 || self.drafts > 0
    }
}

/// A ticket carries a must-fix issue — mirror of TS `hasMustFix`.
fn has_must_fix(h: &Hygiene) -> bool {
    h.issues.iter().any(|i| i.severity == "must_fix")
}

/// A ticket is worth surfacing in board cleanup — mirror of TS `isCleanupWorthy`:
/// it has any hygiene issue, or sits in a stale/unsure bucket.
fn is_cleanup_worthy(h: &Hygiene) -> bool {
    !h.issues.is_empty() || h.bucket == "looks_stale" || h.bucket == "not_sure"
}

/// A worklog is a pending draft — mirror of TS `isPending`: a proposed item is
/// pending in the `proposed` state, a real one in the `drafted` state. Takes the
/// two fields (not `&WorklogItem`) so it's unit-testable without constructing the
/// full row.
fn is_pending(is_proposed: bool, state: &str) -> bool {
    if is_proposed {
        state == "proposed"
    } else {
        state == "drafted"
    }
}

/// Whether a task's provider should be counted given the connected set —
/// mirror of `filterByConnectedProviders`.
fn provider_counts(provider: &str, connected: &[&str]) -> bool {
    !KNOWN_PROVIDERS.contains(&provider) || connected.contains(&provider)
}

/// Count the three action-card tiers. `connected` is the set of connected
/// tracker keys (resolved tray-side from env/OAuth files); empty ⇒ solo user ⇒
/// all zero, mirroring `useActionItems`' `if (isSolo) return []` (which
/// suppresses the drafts card too). `today`/`week_start`/`now_iso` are threaded
/// straight into [`crate::tasks::get_tasks`].
pub async fn action_card_counts(
    pool: &SqlitePool,
    connected: &[&str],
    today: &str,
    week_start: &str,
    now_iso: &str,
) -> anyhow::Result<ActionCardCounts> {
    // Solo → empty stack (no cards of any tier, drafts included).
    if connected.is_empty() {
        return Ok(ActionCardCounts::default());
    }

    let tasks = tasks::get_tasks(pool, today, week_start, now_iso).await?;
    let hygienic = tasks
        .tasks
        .iter()
        .filter(|t| provider_counts(&t.provider, connected))
        .filter_map(|t| t.hygiene.as_ref());

    let (mut mustfix, mut cleanup) = (0usize, 0usize);
    for h in hygienic {
        if has_must_fix(h) {
            mustfix += 1;
        } else if is_cleanup_worthy(h) {
            cleanup += 1;
        }
    }

    let worklogs = worklogs::get_worklogs(pool, today).await?;
    let drafts = worklogs
        .items
        .iter()
        .filter(|w| is_pending(w.is_proposed, &w.state))
        .count();

    Ok(ActionCardCounts {
        mustfix,
        cleanup,
        drafts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hygiene::{Hygiene, HygieneIssue};

    fn issue(severity: &str) -> HygieneIssue {
        HygieneIssue {
            code: "x".into(),
            hint: String::new(),
            fix: None,
            severity: severity.into(),
        }
    }

    fn hygiene(bucket: &str, severities: &[&str]) -> Hygiene {
        Hygiene {
            bucket: bucket.into(),
            issues: severities.iter().map(|s| issue(s)).collect(),
            decision: None,
        }
    }

    #[test]
    fn must_fix_wins_over_cleanup_disjointly() {
        // A ticket with a must-fix issue is must-fix, never also cleanup.
        let h = hygiene("needs_detail", &["must_fix", "optional"]);
        assert!(has_must_fix(&h));
        assert!(is_cleanup_worthy(&h)); // it *is* cleanup-worthy…
                                        // …but the counting loop assigns it to must-fix only (see below).
    }

    #[test]
    fn cleanup_worthy_matches_ts_rule() {
        // Any issue → cleanup-worthy.
        assert!(is_cleanup_worthy(&hygiene("good", &["optional"])));
        // No issues but stale/unsure bucket → cleanup-worthy.
        assert!(is_cleanup_worthy(&hygiene("looks_stale", &[])));
        assert!(is_cleanup_worthy(&hygiene("not_sure", &[])));
        // No issues, healthy bucket → not cleanup-worthy.
        assert!(!is_cleanup_worthy(&hygiene("good", &[])));
    }

    #[test]
    fn provider_filter_mirrors_frontend() {
        let connected = ["jira"];
        assert!(provider_counts("jira", &connected)); // known + connected
        assert!(!provider_counts("github", &connected)); // known + disconnected
        assert!(provider_counts("some_future_tracker", &connected)); // unknown → pass-through
    }

    #[test]
    fn is_pending_splits_proposed_and_real() {
        assert!(is_pending(true, "proposed"));
        assert!(!is_pending(true, "drafted"));
        assert!(is_pending(false, "drafted"));
        assert!(!is_pending(false, "proposed"));
        assert!(!is_pending(false, "approved"));
    }
}
