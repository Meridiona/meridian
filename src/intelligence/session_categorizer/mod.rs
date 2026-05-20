// meridian — normalises screenpipe activity into structured app sessions

mod patterns;
mod scoring;

use serde::{Deserialize, Serialize};

use crate::db::screenpipe::{SignalEvent, WindowTitleCount};

use patterns::CONFIDENCE_FLOOR;
use scoring::{score_app_name, score_audio, score_ocr, score_signals, score_window_titles};

// ---------------------------------------------------------------------------
// ActivityKind
// ---------------------------------------------------------------------------

/// The 10 mutually-exclusive activity categories assigned to every closed
/// `app_session`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Coding,
    CodeReview,
    Meeting,
    Communication,
    Design,
    Documentation,
    Planning,
    DeploymentDevops,
    Research,
    IdlePersonal,
}

impl ActivityKind {
    pub fn is_pm_mappable(self) -> bool {
        !matches!(
            self,
            Self::Meeting | Self::Communication | Self::IdlePersonal
        )
    }

    /// Snake-case string stored in the database `category` column.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Coding => "coding",
            Self::CodeReview => "code_review",
            Self::Meeting => "meeting",
            Self::Communication => "communication",
            Self::Design => "design",
            Self::Documentation => "documentation",
            Self::Planning => "planning",
            Self::DeploymentDevops => "deployment_devops",
            Self::Research => "research",
            Self::IdlePersonal => "idle_personal",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Coding => "Coding",
            Self::CodeReview => "Code Review",
            Self::Meeting => "Meeting",
            Self::Communication => "Communication",
            Self::Design => "Design",
            Self::Documentation => "Documentation",
            Self::Planning => "Planning",
            Self::DeploymentDevops => "Deployment / DevOps",
            Self::Research => "Research",
            Self::IdlePersonal => "Idle / Personal",
        }
    }

    pub(super) fn index(self) -> usize {
        match self {
            Self::Coding => 0,
            Self::CodeReview => 1,
            Self::Meeting => 2,
            Self::Communication => 3,
            Self::Design => 4,
            Self::Documentation => 5,
            Self::Planning => 6,
            Self::DeploymentDevops => 7,
            Self::Research => 8,
            Self::IdlePersonal => 9,
        }
    }

    pub(super) fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Coding,
            1 => Self::CodeReview,
            2 => Self::Meeting,
            3 => Self::Communication,
            4 => Self::Design,
            5 => Self::Documentation,
            6 => Self::Planning,
            7 => Self::DeploymentDevops,
            8 => Self::Research,
            _ => Self::IdlePersonal,
        }
    }
}

// ---------------------------------------------------------------------------
// SessionSignals
// ---------------------------------------------------------------------------

/// All evidence available for one closed `app_session`.
/// Borrows from the caller — zero heap allocation in the categoriser itself.
pub struct SessionSignals<'a> {
    pub app_name: &'a str,
    pub window_titles: &'a [WindowTitleCount],
    pub ocr_text: &'a str,
    pub signals: &'a [SignalEvent],
    pub audio_present: bool,
    pub duration_secs: u64,
}

// ---------------------------------------------------------------------------
// Categorization reasoning
// ---------------------------------------------------------------------------

/// Tracks which signals fired during categorization for debugging.
#[derive(Debug, Clone, Default)]
pub(super) struct Reasoning {
    pub app_match: Option<String>,
    pub window_title_matches: Vec<String>,
    pub ocr_matches: Vec<String>,
    pub audio_match: Option<String>,
    pub signal_matches: Vec<String>,
}

impl Reasoning {
    pub(super) fn format(&self) -> String {
        let mut parts = Vec::new();

        if let Some(ref app) = self.app_match {
            parts.push(format!("app={}", app));
        }
        if !self.window_title_matches.is_empty() {
            parts.push(format!("titles=[{}]", self.window_title_matches.join(",")));
        }
        if !self.ocr_matches.is_empty() {
            parts.push(format!("ocr=[{}]", self.ocr_matches.join(",")));
        }
        if let Some(ref audio) = self.audio_match {
            parts.push(format!("audio={}", audio));
        }
        if !self.signal_matches.is_empty() {
            parts.push(format!("signals=[{}]", self.signal_matches.join(",")));
        }

        if parts.is_empty() {
            "no_signals".to_string()
        } else {
            parts.join(" ")
        }
    }
}

// ---------------------------------------------------------------------------
// Score accumulator
// ---------------------------------------------------------------------------

/// Stack-allocated score vector — 40 bytes, no heap.
pub(super) struct Scores([f32; 10]);

impl Scores {
    pub(super) fn new() -> Self {
        Self([0.0; 10])
    }

    pub(super) fn add(&mut self, kind: ActivityKind, weight: f32) {
        self.0[kind.index()] += weight;
    }

    /// Returns `(winner, confidence)`.
    /// `confidence = max / sum` — fraction of total evidence mass held by winner.
    /// Returns `(IdlePersonal, 0.0)` when no signal fired.
    pub(super) fn winner(&self) -> (ActivityKind, f32) {
        let total: f32 = self.0.iter().sum();
        if total == 0.0 {
            return (ActivityKind::IdlePersonal, 0.0);
        }
        let (idx, &max) = self
            .0
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        (ActivityKind::from_index(idx), max / total)
    }

    /// Returns a formatted breakdown of all category scores for debugging.
    pub(super) fn breakdown(&self) -> String {
        let categories = [
            ("Coding", 0),
            ("CodeReview", 1),
            ("Meeting", 2),
            ("Communication", 3),
            ("Design", 4),
            ("Documentation", 5),
            ("Planning", 6),
            ("DeploymentDevops", 7),
            ("Research", 8),
            ("IdlePersonal", 9),
        ];
        let parts: Vec<String> = categories
            .iter()
            .filter_map(|(name, idx)| {
                let score = self.0[*idx];
                if score > 0.0 {
                    Some(format!("{}={:.1}", name, score))
                } else {
                    None
                }
            })
            .collect();
        if parts.is_empty() {
            "no_signals".to_string()
        } else {
            parts.join(" ")
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Classifies a closed session into an `ActivityKind` and a confidence score.
///
/// `confidence` is the fraction of total evidence mass held by the winner
/// (`winner_score / sum_of_all_scores`).  Ranges 0–1.  Stored in
/// `ticket_links.confidence` so PM matching can gate on it.
///
/// Pure function — no I/O, no allocations beyond small `to_lowercase` copies.
#[tracing::instrument(
    skip_all,
    fields(
        app_name = %signals.app_name,
        window_title_count = tracing::field::Empty,
        ocr_text_bytes = tracing::field::Empty,
        signal_count = tracing::field::Empty,
        audio_present = signals.audio_present,
        duration_secs = signals.duration_secs,
        category = tracing::field::Empty,
        confidence = tracing::field::Empty,
        confidence_floor_applied = tracing::field::Empty,
    )
)]
pub fn categorize(signals: &SessionSignals<'_>) -> (ActivityKind, f32) {
    let mut scores = Scores::new();
    let mut reasoning = Reasoning::default();

    let app_lc = signals.app_name.to_lowercase();
    let ocr_lc = signals.ocr_text.to_lowercase();

    score_audio(signals, &mut scores, &mut reasoning);
    score_app_name(&app_lc, &mut scores, &mut reasoning);
    score_window_titles(signals.window_titles, &mut scores, &mut reasoning);
    score_ocr(&ocr_lc, &mut scores, &mut reasoning);
    score_signals(signals.signals, &mut scores, &mut reasoning);

    let signal_count = signals.window_titles.len()
        + signals.signals.len()
        + if signals.audio_present { 1 } else { 0 };
    tracing::Span::current().record("window_title_count", signals.window_titles.len());
    tracing::Span::current().record("ocr_text_bytes", signals.ocr_text.len());
    tracing::Span::current().record("signal_count", signal_count);

    let (kind, confidence) = scores.winner();
    let (final_kind, final_conf) = if confidence < CONFIDENCE_FLOOR {
        tracing::Span::current().record("confidence_floor_applied", true);
        (ActivityKind::IdlePersonal, confidence)
    } else {
        tracing::Span::current().record("confidence_floor_applied", false);
        (kind, confidence)
    };
    tracing::Span::current().record("category", final_kind.as_str());
    tracing::Span::current().record("confidence", final_conf as f64);

    let score_breakdown = scores.breakdown();
    let reasoning_breakdown = reasoning.format();

    tracing::debug!(
        app_name = signals.app_name,
        window_titles = signals.window_titles.len(),
        signals = signal_count,
        ocr_text_bytes = signals.ocr_text.len(),
        audio_present = signals.audio_present,
        category = final_kind.as_str(),
        confidence = final_conf,
        scores = score_breakdown,
        "session categorized"
    );

    tracing::debug!(
        reasoning = reasoning_breakdown,
        "categorization reasoning"
    );

    (final_kind, final_conf)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_titles(titles: &[(&str, i64)]) -> Vec<WindowTitleCount> {
        titles
            .iter()
            .map(|(name, count)| WindowTitleCount {
                window_name: name.to_string(),
                count: *count,
            })
            .collect()
    }

    macro_rules! cat {
        ($app:expr, $titles:expr, $ocr:expr, $audio:expr) => {{
            let signals = SessionSignals {
                app_name: $app,
                window_titles: $titles,
                ocr_text: $ocr,
                signals: &[],
                audio_present: $audio,
                duration_secs: 120,
            };
            categorize(&signals).0
        }};
    }

    #[test]
    fn cursor_is_coding() {
        assert_eq!(cat!("cursor", &[], "", false), ActivityKind::Coding);
    }

    #[test]
    fn vscode_mixed_case_is_coding() {
        assert_eq!(
            cat!("Visual Studio Code", &[], "", false),
            ActivityKind::Coding
        );
    }

    #[test]
    fn zoom_no_audio_is_meeting() {
        assert_eq!(cat!("zoom", &[], "", false), ActivityKind::Meeting);
    }

    #[test]
    fn zoom_with_audio_is_meeting() {
        assert_eq!(cat!("zoom", &[], "", true), ActivityKind::Meeting);
    }

    #[test]
    fn slack_no_audio_is_communication() {
        assert_eq!(cat!("slack", &[], "", false), ActivityKind::Communication);
    }

    #[test]
    fn figma_is_design() {
        assert_eq!(cat!("figma", &[], "", false), ActivityKind::Design);
    }

    #[test]
    fn notion_is_documentation() {
        assert_eq!(cat!("notion", &[], "", false), ActivityKind::Documentation);
    }

    #[test]
    fn datadog_is_devops() {
        assert_eq!(
            cat!("datadog", &[], "", false),
            ActivityKind::DeploymentDevops
        );
    }

    #[test]
    fn vscode_kubectl_ocr_is_devops() {
        let titles = make_titles(&[("Dockerfile", 40), ("main.rs", 5)]);
        let signals = SessionSignals {
            app_name: "Visual Studio Code",
            window_titles: &titles,
            ocr_text: "kubectl get pods -n production\ndocker build -t myapp:latest .",
            signals: &[],
            audio_present: false,
            duration_secs: 600,
        };
        assert_eq!(categorize(&signals).0, ActivityKind::DeploymentDevops);
    }

    #[test]
    fn vscode_mostly_code_is_coding() {
        let titles = make_titles(&[("main.rs", 50), ("lib.rs", 20), ("Dockerfile", 2)]);
        let signals = SessionSignals {
            app_name: "Visual Studio Code",
            window_titles: &titles,
            ocr_text: "fn main() {\n    let x = 42;\n}",
            signals: &[],
            audio_present: false,
            duration_secs: 600,
        };
        assert_eq!(categorize(&signals).0, ActivityKind::Coding);
    }

    #[test]
    fn terminal_kubectl_is_devops() {
        assert_eq!(
            cat!("terminal", &[], "kubectl get pods -n production", false),
            ActivityKind::DeploymentDevops
        );
    }

    #[test]
    fn terminal_docker_is_devops() {
        assert_eq!(
            cat!("terminal", &[], "docker build -t myapp .", false),
            ActivityKind::DeploymentDevops
        );
    }

    #[test]
    fn terminal_terraform_is_devops() {
        assert_eq!(
            cat!("terminal", &[], "terraform apply -auto-approve", false),
            ActivityKind::DeploymentDevops
        );
    }

    #[test]
    fn terminal_no_devops_is_coding() {
        assert_eq!(
            cat!("iterm", &[], "cargo build --release", false),
            ActivityKind::Coding
        );
    }

    #[test]
    fn browser_pr_url_is_code_review() {
        let titles = make_titles(&[("Fix null check · Pull Request #42 · github.com", 10)]);
        assert_eq!(cat!("chrome", &titles, "", false), ActivityKind::CodeReview);
    }

    #[test]
    fn browser_github_actions_is_devops() {
        let titles = make_titles(&[("github.com/org/repo/actions/runs/123", 10)]);
        assert_eq!(
            cat!("safari", &titles, "", false),
            ActivityKind::DeploymentDevops
        );
    }

    #[test]
    fn browser_stackoverflow_is_research() {
        let titles = make_titles(&[("stackoverflow.com — how to center a div", 10)]);
        assert_eq!(cat!("chrome", &titles, "", false), ActivityKind::Research);
    }

    #[test]
    fn browser_youtube_is_idle() {
        let titles = make_titles(&[("youtube.com - funny cat videos", 10)]);
        assert_eq!(
            cat!("chrome", &titles, "", false),
            ActivityKind::IdlePersonal
        );
    }

    #[test]
    fn browser_reddit_rust_is_research() {
        let titles = make_titles(&[("reddit.com/r/rust/comments/abc/question", 10)]);
        assert_eq!(cat!("chrome", &titles, "", false), ActivityKind::Research);
    }

    #[test]
    fn browser_reddit_gaming_is_idle() {
        let titles = make_titles(&[("reddit.com/r/gaming/comments/xyz/cool_game", 10)]);
        assert_eq!(
            cat!("chrome", &titles, "", false),
            ActivityKind::IdlePersonal
        );
    }

    #[test]
    fn browser_unknown_defaults_to_research() {
        let titles = make_titles(&[("somecompany-internal-tool.com/dashboard", 10)]);
        assert_eq!(cat!("arc", &titles, "", false), ActivityKind::Research);
    }

    #[test]
    fn dockerfile_dominant_title_beats_ide_prior() {
        let titles = make_titles(&[("Dockerfile", 45), ("README.md", 5)]);
        let signals = SessionSignals {
            app_name: "code",
            window_titles: &titles,
            ocr_text: "",
            signals: &[],
            audio_present: false,
            duration_secs: 300,
        };
        assert_eq!(categorize(&signals).0, ActivityKind::DeploymentDevops);
    }

    #[test]
    fn branch_name_clipboard_adds_planning() {
        let sigs = vec![SignalEvent {
            event_type: "clipboard".to_string(),
            value: Some("feat/KAN-7-add-auth".to_string()),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
        }];
        let signals = SessionSignals {
            app_name: "cursor",
            window_titles: &[],
            ocr_text: "",
            signals: &sigs,
            audio_present: false,
            duration_secs: 120,
        };
        assert_eq!(categorize(&signals).0, ActivityKind::Coding);
    }

    #[test]
    fn confidence_is_between_zero_and_one() {
        let signals = SessionSignals {
            app_name: "zoom",
            window_titles: &[],
            ocr_text: "",
            signals: &[],
            audio_present: false,
            duration_secs: 120,
        };
        let (_, conf) = categorize(&signals);
        assert!((0.0..=1.0).contains(&conf));
    }

    #[test]
    fn is_pm_mappable_excludes_meeting_comm_idle() {
        assert!(!ActivityKind::Meeting.is_pm_mappable());
        assert!(!ActivityKind::Communication.is_pm_mappable());
        assert!(!ActivityKind::IdlePersonal.is_pm_mappable());
        assert!(ActivityKind::Coding.is_pm_mappable());
        assert!(ActivityKind::DeploymentDevops.is_pm_mappable());
    }
}
