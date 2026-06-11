//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

use crate::db::screenpipe::{SignalEvent, WindowTitleCount};

use super::patterns::{
    APP_PATTERNS, BRANCH_PREFIXES, BROWSER_APPS, CODE_OCR_TOKENS, COMM_TITLE_TOKENS,
    DESIGN_TITLE_TOKENS, DEVELOPER_SUBREDDITS, DEVOPS_OCR_TOKENS, DEVOPS_TITLE_TOKENS,
    DEVOPS_WINDOW_TOKENS, DIFF_OCR_TOKENS, DOCS_TITLE_TOKENS, IDLE_TITLE_TOKENS,
    MEETING_OCR_TOKENS, PLANNING_TITLE_TOKENS, PR_TITLE_TOKENS, RESEARCH_TITLE_TOKENS,
    TERMINAL_APPS,
};
use super::{ActivityKind, Reasoning, Scores, SessionSignals};

/// Audio + known meeting app is the strongest possible meeting signal.
pub(super) fn score_audio(
    signals: &SessionSignals<'_>,
    scores: &mut Scores,
    reasoning: &mut Reasoning,
) {
    if !signals.audio_present {
        return;
    }
    let app_lc = signals.app_name.to_lowercase();
    let is_meeting = [
        "zoom",
        "zoom.us",
        "google meet",
        "webex",
        "whereby",
        "microsoft teams",
    ]
    .iter()
    .any(|p| app_lc.contains(p));
    if is_meeting {
        scores.add(ActivityKind::Meeting, 50.0);
        reasoning.audio_match = Some("meeting_app_with_audio".to_string());
    } else {
        // Audio alone (e.g. slack huddle) is a mild meeting signal.
        scores.add(ActivityKind::Meeting, 5.0);
        reasoning.audio_match = Some("audio_only".to_string());
    }
}

/// App name lookup against the static table.
/// Terminals and browsers contribute a low base weight — their content signals dominate.
pub(super) fn score_app_name(app_lc: &str, scores: &mut Scores, reasoning: &mut Reasoning) {
    // Terminal: very low Coding prior — OCR will override if DevOps tools appear.
    if TERMINAL_APPS.iter().any(|p| app_lc.contains(p)) {
        scores.add(ActivityKind::Coding, 10.0);
        reasoning.app_match = Some("terminal".to_string());
        return;
    }
    // Browser: no prior at all — window titles decide everything.
    if BROWSER_APPS.iter().any(|p| app_lc.contains(p)) {
        reasoning.app_match = Some("browser".to_string());
        return;
    }
    for &(pattern, kind, weight) in APP_PATTERNS {
        if app_lc.contains(pattern) {
            scores.add(kind, weight);
            reasoning.app_match = Some(format!("{}({})", pattern, kind.as_str()));
            return;
        }
    }
}

/// Window titles, frequency-weighted.
/// Each title's contribution scales by its share of total observed focus time,
/// so a Dockerfile open for 80% of the session dominates a README open for 5%.
pub(super) fn score_window_titles(
    titles: &[WindowTitleCount],
    scores: &mut Scores,
    reasoning: &mut Reasoning,
) {
    if titles.is_empty() {
        return;
    }
    let total_count: i64 = titles.iter().map(|t| t.count).sum();
    if total_count == 0 {
        return;
    }

    const BASE: f32 = 35.0;

    for title in titles {
        let freq = title.count as f32 / total_count as f32;
        let t = title.window_name.to_lowercase();
        let w = freq * BASE;
        let freq_pct = (freq * 100.0) as i32;

        if contains_any(&t, PR_TITLE_TOKENS) {
            scores.add(ActivityKind::CodeReview, w);
            reasoning
                .window_title_matches
                .push(format!("pr({}%)", freq_pct));
        } else if contains_any(&t, DEVOPS_WINDOW_TOKENS)
            || contains_any(&t, DEVOPS_TITLE_TOKENS)
            || (t.contains("github.com/") && t.contains("actions"))
        {
            scores.add(ActivityKind::DeploymentDevops, w);
            reasoning
                .window_title_matches
                .push(format!("devops({}%)", freq_pct));
        } else if contains_any(&t, PLANNING_TITLE_TOKENS) {
            scores.add(ActivityKind::Planning, w);
            reasoning
                .window_title_matches
                .push(format!("planning({}%)", freq_pct));
        } else if contains_any(&t, DESIGN_TITLE_TOKENS) {
            scores.add(ActivityKind::Design, w);
            reasoning
                .window_title_matches
                .push(format!("design({}%)", freq_pct));
        } else if contains_any(&t, DOCS_TITLE_TOKENS) {
            scores.add(ActivityKind::Documentation, w);
            reasoning
                .window_title_matches
                .push(format!("docs({}%)", freq_pct));
        } else if contains_any(&t, COMM_TITLE_TOKENS) {
            scores.add(ActivityKind::Communication, w);
            reasoning
                .window_title_matches
                .push(format!("comm({}%)", freq_pct));
        } else if t.contains("reddit.com/r/") {
            if contains_any(&t, DEVELOPER_SUBREDDITS) {
                scores.add(ActivityKind::Research, w);
                reasoning
                    .window_title_matches
                    .push(format!("reddit_dev({}%)", freq_pct));
            } else {
                scores.add(ActivityKind::IdlePersonal, w);
                reasoning
                    .window_title_matches
                    .push(format!("reddit_idle({}%)", freq_pct));
            }
        } else if t.contains("localhost") || t.contains("127.0.0.1") {
            // Local dev server — the user is testing their own code.
            scores.add(ActivityKind::Coding, w);
            reasoning
                .window_title_matches
                .push(format!("localhost({}%)", freq_pct));
        } else if contains_any(&t, RESEARCH_TITLE_TOKENS) {
            scores.add(ActivityKind::Research, w);
            reasoning
                .window_title_matches
                .push(format!("research({}%)", freq_pct));
        } else if contains_any(&t, IDLE_TITLE_TOKENS) {
            scores.add(ActivityKind::IdlePersonal, w);
            reasoning
                .window_title_matches
                .push(format!("idle({}%)", freq_pct));
        } else {
            // Unknown title — small Research bump as benefit of the doubt.
            scores.add(ActivityKind::Research, w * 0.4);
            reasoning
                .window_title_matches
                .push(format!("unknown({}%)", freq_pct));
        }
    }
}

/// OCR content scoring — same token lists, flat weight per match.
/// Multiple matches accumulate (e.g. kubectl + docker → stronger DevOps signal).
pub(super) fn score_ocr(ocr_lc: &str, scores: &mut Scores, reasoning: &mut Reasoning) {
    if ocr_lc.is_empty() {
        return;
    }
    for token in DEVOPS_OCR_TOKENS {
        if ocr_lc.contains(token) {
            scores.add(ActivityKind::DeploymentDevops, 10.0);
            reasoning.ocr_matches.push(token.to_string());
        }
    }
    for token in CODE_OCR_TOKENS {
        if ocr_lc.contains(token) {
            scores.add(ActivityKind::Coding, 5.0);
            reasoning.ocr_matches.push(token.to_string());
        }
    }
    for token in MEETING_OCR_TOKENS {
        if ocr_lc.contains(token) {
            scores.add(ActivityKind::Meeting, 10.0);
            reasoning.ocr_matches.push(token.to_string());
        }
    }
    for token in DIFF_OCR_TOKENS {
        if ocr_lc.contains(token) {
            scores.add(ActivityKind::CodeReview, 7.0);
            reasoning.ocr_matches.push(token.to_string());
        }
    }
}

/// Clipboard and app-switch signals.
/// Clipboard is high-intent evidence — the user deliberately copied something.
pub(super) fn score_signals(
    signals: &[SignalEvent],
    scores: &mut Scores,
    reasoning: &mut Reasoning,
) {
    for signal in signals {
        if signal.event_type != "clipboard" {
            continue;
        }
        let Some(ref value) = signal.value else {
            continue;
        };
        let v = value.to_lowercase();

        // Branch name in clipboard → strong coding/planning signal.
        if BRANCH_PREFIXES.iter().any(|p| v.contains(p)) {
            scores.add(ActivityKind::Planning, 20.0);
            scores.add(ActivityKind::Coding, 10.0);
            reasoning.signal_matches.push("branch_name".to_string());
        }
        // PR URL in clipboard.
        if contains_any(&v, PR_TITLE_TOKENS) {
            scores.add(ActivityKind::CodeReview, 20.0);
            reasoning.signal_matches.push("pr_url".to_string());
        }
        // kubectl / terraform in clipboard (copied a command).
        if contains_any(&v, DEVOPS_OCR_TOKENS) {
            scores.add(ActivityKind::DeploymentDevops, 15.0);
            reasoning.signal_matches.push("devops_cmd".to_string());
        }
    }
}

pub(super) fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}
