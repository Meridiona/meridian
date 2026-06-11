//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Grounding: take the synth's JiraUpdate and keep only what we can prove.
// Drop every bullet that cites no session, recompute coverage, and attach the
// routing-relevant risk flags. Pure logic, no LLM, no IO — a faithful port of
// `pm_worklog_update/hooks.py` (build_grounded_narrative + risk_flagger +
// evidence_ref_validator). The Python computes coverage AFTER dropping, so it is
// effectively 1.0 when any grounded bullet survives and 0.0 when none do; we keep
// that exact semantics.

use std::collections::HashSet;

use super::models::{BulletWithEvidence, GroundedNarrative, JiraUpdate, SessionBundle};

// Risk-flag string values — identical to the Python `RiskFlag` enum values so
// the stored payload is byte-compatible.
pub const FLAG_LOW_EVIDENCE: &str = "low_evidence";
pub const FLAG_LOW_CONFIDENCE: &str = "low_confidence";
pub const FLAG_TICKET_CLOSED: &str = "ticket_closed_upstream";
pub const FLAG_CROSS_TICKET_LEAK: &str = "cross_ticket_leak";

/// Drop un-evidenced bullets, compute coverage, and attach risk flags.
///
/// `min_confidence` gates the `low_confidence` flag. The returned
/// `GroundedNarrative.update` has its `risk_flags` populated (sorted, deduped)
/// and — if nothing survived grounding — `confidence` forced to 0.
pub fn ground(
    mut update: JiraUpdate,
    bundle: &SessionBundle,
    min_confidence: f64,
) -> GroundedNarrative {
    let mut dropped: Vec<String> = Vec::new();

    // 1. Drop bullets with no evidence, per group, in place.
    keep_evidenced(&mut update.what_shipped, "what_shipped", &mut dropped);
    keep_evidenced(&mut update.in_progress, "in_progress", &mut dropped);
    keep_evidenced(&mut update.blockers, "blockers", &mut dropped);
    keep_evidenced(&mut update.decisions, "decisions", &mut dropped);

    let total_kept = update.bullets().count();
    let coverage = if total_kept == 0 { 0.0 } else { 1.0 };

    // 2. Risk flags.
    let mut flags: HashSet<String> = update.risk_flags.iter().cloned().collect();

    if total_kept == 0 {
        flags.insert(FLAG_LOW_EVIDENCE.to_string());
        update.confidence = 0.0;
    }
    if update.confidence < min_confidence {
        flags.insert(FLAG_LOW_CONFIDENCE.to_string());
    }
    if bundle.pm_task_is_terminal {
        flags.insert(FLAG_TICKET_CLOSED.to_string());
    }
    let bundle_ids: HashSet<i64> = bundle.sessions.iter().map(|s| s.id).collect();
    if update
        .bullets()
        .any(|b| b.evidence_refs.iter().any(|r| !bundle_ids.contains(r)))
    {
        flags.insert(FLAG_CROSS_TICKET_LEAK.to_string());
    }

    let mut sorted: Vec<String> = flags.into_iter().collect();
    sorted.sort();
    update.risk_flags = sorted;

    GroundedNarrative {
        update,
        coverage,
        dropped_bullets: dropped,
    }
}

/// Retain only bullets with at least one evidence ref; record the dropped ones
/// as `"{group}: {first 80 chars}"` (matching the Python log format).
fn keep_evidenced(bullets: &mut Vec<BulletWithEvidence>, group: &str, dropped: &mut Vec<String>) {
    bullets.retain(|b| {
        if b.evidence_refs.is_empty() {
            let snippet: String = b.text.chars().take(80).collect();
            dropped.push(format!("{group}: {snippet}"));
            false
        } else {
            true
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bullet(text: &str, refs: &[i64]) -> BulletWithEvidence {
        BulletWithEvidence {
            text: text.to_string(),
            evidence_refs: refs.to_vec(),
        }
    }

    fn base_update() -> JiraUpdate {
        JiraUpdate {
            task_key: "KAN-1".into(),
            window_start: "2026-05-30T05:00:00Z".into(),
            window_end: "2026-05-30T06:00:00Z".into(),
            cycle_index: 0,
            time_spent_seconds: 600,
            summary: "did work".into(),
            what_shipped: vec![bullet("shipped a", &[1]), bullet("ungrounded", &[])],
            in_progress: vec![],
            blockers: vec![],
            decisions: vec![],
            next_steps: vec![],
            risk_flags: vec![],
            confidence: 0.9,
            reasoning: String::new(),
        }
    }

    fn bundle_with_ids(ids: &[i64], is_terminal: bool) -> SessionBundle {
        SessionBundle {
            task_key: "KAN-1".into(),
            window_start: "2026-05-30T05:00:00Z".into(),
            window_end: "2026-05-30T06:00:00Z".into(),
            cycle_index: 0,
            sessions: ids
                .iter()
                .map(|&id| super::super::models::SessionDigest {
                    id,
                    app_name: "Code".into(),
                    started_at: "2026-05-30T05:00:00Z".into(),
                    ended_at: "2026-05-30T05:10:00Z".into(),
                    duration_s: 600,
                    idle_frame_s: 0,
                    top_titles: vec![],
                    dimensions: Default::default(),
                    excerpt: String::new(),
                    category: None,
                    text_source: None,
                })
                .collect(),
            total_seconds: 600,
            real_seconds: 600,
            raw_text_bytes: 0,
            is_heavy: false,
            pm_task_status: None,
            pm_task_is_terminal: is_terminal,
            pm_task_title: None,
            pm_task_description: None,
            assignee_name: None,
            earlier_today_summaries: vec![],
        }
    }

    #[test]
    fn drops_ungrounded_and_keeps_evidenced() {
        let g = ground(base_update(), &bundle_with_ids(&[1], false), 0.65);
        assert_eq!(g.update.what_shipped.len(), 1);
        assert_eq!(g.update.what_shipped[0].text, "shipped a");
        assert_eq!(g.dropped_bullets.len(), 1);
        assert_eq!(g.coverage, 1.0);
        assert!(g.update.risk_flags.is_empty());
    }

    #[test]
    fn no_evidence_at_all_flags_low_evidence_and_zeroes_confidence() {
        let mut u = base_update();
        u.what_shipped = vec![bullet("nothing proven", &[])];
        let g = ground(u, &bundle_with_ids(&[1], false), 0.65);
        assert_eq!(g.coverage, 0.0);
        assert!(g.update.risk_flags.contains(&FLAG_LOW_EVIDENCE.to_string()));
        assert!(g
            .update
            .risk_flags
            .contains(&FLAG_LOW_CONFIDENCE.to_string()));
        assert_eq!(g.update.confidence, 0.0);
    }

    #[test]
    fn cross_ticket_leak_when_ref_outside_bundle() {
        let mut u = base_update();
        u.what_shipped = vec![bullet("cites a foreign session", &[999])];
        let g = ground(u, &bundle_with_ids(&[1, 2], false), 0.65);
        assert!(g
            .update
            .risk_flags
            .contains(&FLAG_CROSS_TICKET_LEAK.to_string()));
    }

    #[test]
    fn ticket_closed_flag_when_terminal() {
        let g = ground(base_update(), &bundle_with_ids(&[1], true), 0.65);
        assert!(g
            .update
            .risk_flags
            .contains(&FLAG_TICKET_CLOSED.to_string()));
    }
}
