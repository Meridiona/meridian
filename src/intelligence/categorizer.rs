// meridian — normalises screenpipe activity into structured app sessions

use serde::{Deserialize, Serialize};

use crate::db::screenpipe::{ElementSample, SignalEvent, WindowTitleCount};

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

    fn index(self) -> usize {
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

    fn from_index(i: usize) -> Self {
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
    pub elements: &'a [ElementSample],
    pub signals: &'a [SignalEvent],
    pub audio_present: bool,
    pub duration_secs: u64,
}

// ---------------------------------------------------------------------------
// Score accumulator
// ---------------------------------------------------------------------------

/// Stack-allocated score vector — 40 bytes, no heap.
struct Scores([f32; 10]);

impl Scores {
    fn new() -> Self {
        Self([0.0; 10])
    }

    fn add(&mut self, kind: ActivityKind, weight: f32) {
        self.0[kind.index()] += weight;
    }

    /// Returns `(winner, confidence)`.
    /// `confidence = max / sum` — fraction of total evidence mass held by winner.
    /// Returns `(IdlePersonal, 0.0)` when no signal fired.
    fn winner(&self) -> (ActivityKind, f32) {
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
}

// ---------------------------------------------------------------------------
// Static pattern tables
// ---------------------------------------------------------------------------

/// `(app_name_substring, category, weight)`.
/// Specific apps (zoom, figma) get higher weight — they're unambiguous.
/// Generic apps (code, terminal, browser) get lower weight — content can override.
static APP_PATTERNS: &[(&str, ActivityKind, f32)] = &[
    // Meeting — highly specific, almost no false positives
    ("zoom.us", ActivityKind::Meeting, 40.0),
    ("zoom", ActivityKind::Meeting, 40.0),
    ("google meet", ActivityKind::Meeting, 40.0),
    ("webex", ActivityKind::Meeting, 40.0),
    ("whereby", ActivityKind::Meeting, 40.0),
    // Communication — specific apps
    ("slack", ActivityKind::Communication, 30.0),
    ("microsoft teams", ActivityKind::Communication, 30.0),
    ("discord", ActivityKind::Communication, 30.0),
    ("superhuman", ActivityKind::Communication, 30.0),
    ("spark", ActivityKind::Communication, 30.0),
    ("telegram", ActivityKind::Communication, 30.0),
    ("whatsapp", ActivityKind::Communication, 30.0),
    ("outlook", ActivityKind::Communication, 30.0),
    ("mail", ActivityKind::Communication, 25.0),
    // Design — specific apps
    ("figma", ActivityKind::Design, 35.0),
    ("sketch", ActivityKind::Design, 35.0),
    ("adobe xd", ActivityKind::Design, 35.0),
    ("framer", ActivityKind::Design, 35.0),
    ("canva", ActivityKind::Design, 35.0),
    ("affinity designer", ActivityKind::Design, 35.0),
    ("whimsical", ActivityKind::Design, 35.0),
    ("lottiefiles", ActivityKind::Design, 30.0),
    // Documentation — specific apps
    ("notion", ActivityKind::Documentation, 30.0),
    ("obsidian", ActivityKind::Documentation, 30.0),
    ("quip", ActivityKind::Documentation, 30.0),
    ("confluence", ActivityKind::Documentation, 30.0),
    ("bear", ActivityKind::Documentation, 30.0),
    ("ulysses", ActivityKind::Documentation, 30.0),
    // Planning — specific apps
    ("linear", ActivityKind::Planning, 30.0),
    ("asana", ActivityKind::Planning, 30.0),
    ("trello", ActivityKind::Planning, 30.0),
    ("monday", ActivityKind::Planning, 30.0),
    ("clickup", ActivityKind::Planning, 30.0),
    ("shortcut", ActivityKind::Planning, 30.0),
    // DevOps — specific apps
    ("datadog", ActivityKind::DeploymentDevops, 35.0),
    ("grafana", ActivityKind::DeploymentDevops, 35.0),
    ("pagerduty", ActivityKind::DeploymentDevops, 35.0),
    ("opsgenie", ActivityKind::DeploymentDevops, 35.0),
    ("jenkins", ActivityKind::DeploymentDevops, 35.0),
    ("argocd", ActivityKind::DeploymentDevops, 35.0),
    // Coding IDEs — medium weight; content signals can push to DevOps or CodeReview
    ("antigravity", ActivityKind::Coding, 50.0),
    ("cursor", ActivityKind::Coding, 25.0),
    ("intellij", ActivityKind::Coding, 25.0),
    ("pycharm", ActivityKind::Coding, 25.0),
    ("webstorm", ActivityKind::Coding, 25.0),
    ("goland", ActivityKind::Coding, 25.0),
    ("rubymine", ActivityKind::Coding, 25.0),
    ("clion", ActivityKind::Coding, 25.0),
    ("xcode", ActivityKind::Coding, 25.0),
    ("neovim", ActivityKind::Coding, 25.0),
    ("nvim", ActivityKind::Coding, 25.0),
    ("vim", ActivityKind::Coding, 25.0),
    ("emacs", ActivityKind::Coding, 25.0),
    ("zed", ActivityKind::Coding, 25.0),
    ("sublime text", ActivityKind::Coding, 25.0),
    ("nova", ActivityKind::Coding, 25.0),
    ("helix", ActivityKind::Coding, 25.0),
    // "code" last — short token, match after longer names
    ("code", ActivityKind::Coding, 20.0),
    // Media / entertainment — always idle
    ("music", ActivityKind::IdlePersonal, 30.0),
    ("spotify", ActivityKind::IdlePersonal, 30.0),
    ("vlc", ActivityKind::IdlePersonal, 30.0),
    ("quicktime player", ActivityKind::IdlePersonal, 30.0),
    // macOS system / personal apps
    ("finder", ActivityKind::IdlePersonal, 30.0),
    ("system settings", ActivityKind::IdlePersonal, 30.0),
    ("system preferences", ActivityKind::IdlePersonal, 30.0),
    ("app store", ActivityKind::IdlePersonal, 30.0),
    ("spotlight", ActivityKind::IdlePersonal, 25.0),
    ("preview", ActivityKind::IdlePersonal, 25.0),
    ("reminders", ActivityKind::IdlePersonal, 25.0),
    ("facetime", ActivityKind::IdlePersonal, 25.0),
    ("iphone mirroring", ActivityKind::IdlePersonal, 25.0),
    // claude.ai / Claude desktop — personal AI usage, not billable work
    ("claude", ActivityKind::IdlePersonal, 20.0),
];

/// Terminal emulators — low base weight; OCR decides the real category.
static TERMINAL_APPS: &[&str] = &[
    "terminal",
    "iterm",
    "warp",
    "alacritty",
    "kitty",
    "ghostty",
    "hyper",
];

/// Browsers — low base weight; window titles decide the real category.
static BROWSER_APPS: &[&str] = &[
    "chrome", "safari", "firefox", "arc", "edge", "brave", "opera", "vivaldi",
];

// OCR / content tokens
static DEVOPS_OCR_TOKENS: &[&str] = &[
    "kubectl",
    "docker",
    "terraform",
    "helm",
    "ansible",
    "aws ",
    "gcloud",
    "az ",
    "heroku",
    "vercel",
    "netlify",
    "k8s",
    "eksctl",
    "flyctl",
];
static CODE_OCR_TOKENS: &[&str] = &[
    "fn ", "def ", "class ", "import ", "const ", "async ", "#include",
];
static MEETING_OCR_TOKENS: &[&str] = &["mute", "unmute", "leave meeting", "share screen"];
static DIFF_OCR_TOKENS: &[&str] = &["+++", "@@"];

// Window-title tokens
static PR_TITLE_TOKENS: &[&str] = &[
    "pull request",
    "/pull/",
    "merge request",
    "/merge_requests/",
];
static DEVOPS_TITLE_TOKENS: &[&str] = &[
    "console.aws",
    "console.cloud.google",
    "portal.azure",
    "circleci.com",
    "app.datadoghq",
    "vercel.com/deployments",
    "netlify.com",
];
static PLANNING_TITLE_TOKENS: &[&str] = &[
    "jira",
    "linear.app",
    "/issues",
    "/projects",
    "asana.com",
    "trello.com",
    "monday.com",
];
static DESIGN_TITLE_TOKENS: &[&str] = &["figma.com"];
static DOCS_TITLE_TOKENS: &[&str] = &[
    "confluence",
    "notion.so",
    "docs.google.com",
    "gitbook",
    "readme.io",
];
static COMM_TITLE_TOKENS: &[&str] = &[
    "mail.google.com",
    "outlook.live",
    "outlook.office",
    "app.slack.com",
    "chat.google.com",
    "teams.microsoft.com",
    "discord.com/channels",
];
static RESEARCH_TITLE_TOKENS: &[&str] = &[
    "stackoverflow.com",
    "docs.rs",
    "developer.mozilla",
    "developer.apple",
    "pkg.go.dev",
    "npmjs.com",
    "pypi.org",
    "crates.io",
    "github.com",
    "gitlab.com",
    "arxiv.org",
    "hackernews",
    "news.ycombinator",
    "medium.com",
    "dev.to",
    "udemy.com",
    "coursera.org",
    "egghead.io",
    "frontendmasters.com",
    "pluralsight.com",
    "oreilly.com",
];
static IDLE_TITLE_TOKENS: &[&str] = &[
    "youtube.com",
    "- youtube -", // window title format when screenpipe stores the page title, not the URL
    "netflix.com",
    "twitter.com",
    "x.com",
    "instagram.com",
    "facebook.com",
    "tiktok.com",
    "twitch.tv",
    "spotify.com",
    "news.",
    "nytimes.com",
    "bbc.com",
];
static DEVELOPER_SUBREDDITS: &[&str] = &[
    "/r/rust",
    "/r/programming",
    "/r/learnprogramming",
    "/r/webdev",
    "/r/devops",
    "/r/python",
    "/r/javascript",
    "/r/typescript",
    "/r/golang",
    "/r/swift",
    "/r/kotlin",
    "/r/java",
    "/r/cpp",
    "/r/cscareerquestions",
    "/r/sysadmin",
    "/r/netsec",
    "/r/machinelearning",
    "/r/datascience",
    "/r/compsci",
    "/r/linux",
    "/r/opensource",
];

// DevOps file extensions / names seen in window titles (IDE tabs, terminal CWDs)
static DEVOPS_WINDOW_TOKENS: &[&str] = &[
    "dockerfile",
    ".tf",
    ".yaml",
    ".yml",
    "k8s",
    "kubernetes",
    "helm",
    "ansible",
    ".sh",
    "makefile",
    "jenkinsfile",
];

// A11y element roles that amplify the weight of matched text.
// Includes both bare role names (unit tests) and macOS AX-prefixed names (real screenpipe data).
// AXRadioButton is intentionally excluded — macOS uses it for browser tab bars, not action buttons.
static INTERACTIVE_ROLES: &[&str] = &[
    "button",
    "menuitem",
    "menubutton",
    "AXButton",
    "AXMenuItem",
    "AXMenuButton",
    "AXPopUpButton",
    "AXCheckBox",
];

// A11y button/action text tokens — only scored for interactive roles (see score_elements).
// "release" and "apply" are excluded: both appear in YouTube headings, form submit buttons,
// and informational UI text, causing systematic false positives.
static DEVOPS_ELEMENT_TOKENS: &[&str] = &["deploy", "run pipeline", "trigger", "publish"];
static MEETING_ELEMENT_TOKENS: &[&str] = &[
    "mute",
    "unmute",
    "leave",
    "share screen",
    "end meeting",
    "join",
];
static REVIEW_ELEMENT_TOKENS: &[&str] = &[
    "approve",
    "request changes",
    "submit review",
    "resolve",
    "comment",
];

// Clipboard / signal tokens
static BRANCH_PREFIXES: &[&str] = &[
    "feat/",
    "fix/",
    "chore/",
    "refactor/",
    "hotfix/",
    "release/",
];

// Minimum confidence fraction for the winner to be trusted.
// Below this, every signal was weak — fall back to IdlePersonal.
const CONFIDENCE_FLOOR: f32 = 0.35;

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
        signal_count = tracing::field::Empty,
        category = tracing::field::Empty,
        confidence = tracing::field::Empty,
    )
)]
pub fn categorize(signals: &SessionSignals<'_>) -> (ActivityKind, f32) {
    let mut scores = Scores::new();

    let app_lc = signals.app_name.to_lowercase();
    let ocr_lc = signals.ocr_text.to_lowercase();

    score_audio(signals, &mut scores);
    score_app_name(&app_lc, &mut scores);
    score_window_titles(signals.window_titles, &mut scores);
    score_ocr(&ocr_lc, &mut scores);
    score_elements(signals.elements, &mut scores);
    score_signals(signals.signals, &mut scores);

    let signal_count = signals.window_titles.len()
        + signals.elements.len()
        + signals.signals.len()
        + if signals.audio_present { 1 } else { 0 };
    tracing::Span::current().record("signal_count", signal_count);

    let (kind, confidence) = scores.winner();
    let (final_kind, final_conf) = if confidence < CONFIDENCE_FLOOR {
        (ActivityKind::IdlePersonal, confidence)
    } else {
        (kind, confidence)
    };
    tracing::Span::current().record("category", final_kind.as_str());
    tracing::Span::current().record("confidence", final_conf as f64);
    (final_kind, final_conf)
}

// ---------------------------------------------------------------------------
// Scorers
// ---------------------------------------------------------------------------

/// Audio + known meeting app is the strongest possible meeting signal.
fn score_audio(signals: &SessionSignals<'_>, scores: &mut Scores) {
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
    }
    // Audio alone (e.g. slack huddle) is a mild meeting signal.
    scores.add(ActivityKind::Meeting, 5.0);
}

/// App name lookup against the static table.
/// Terminals and browsers contribute a low base weight — their content signals dominate.
fn score_app_name(app_lc: &str, scores: &mut Scores) {
    // Terminal: very low Coding prior — OCR will override if DevOps tools appear.
    if TERMINAL_APPS.iter().any(|p| app_lc.contains(p)) {
        scores.add(ActivityKind::Coding, 10.0);
        return;
    }
    // Browser: no prior at all — window titles decide everything.
    if BROWSER_APPS.iter().any(|p| app_lc.contains(p)) {
        return;
    }
    for &(pattern, kind, weight) in APP_PATTERNS {
        if app_lc.contains(pattern) {
            scores.add(kind, weight);
            return;
        }
    }
}

/// Window titles, frequency-weighted.
/// Each title's contribution scales by its share of total observed focus time,
/// so a Dockerfile open for 80% of the session dominates a README open for 5%.
fn score_window_titles(titles: &[WindowTitleCount], scores: &mut Scores) {
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

        if contains_any(&t, PR_TITLE_TOKENS) {
            scores.add(ActivityKind::CodeReview, w);
        } else if contains_any(&t, DEVOPS_WINDOW_TOKENS)
            || contains_any(&t, DEVOPS_TITLE_TOKENS)
            || (t.contains("github.com/") && t.contains("actions"))
        {
            scores.add(ActivityKind::DeploymentDevops, w);
        } else if contains_any(&t, PLANNING_TITLE_TOKENS) {
            scores.add(ActivityKind::Planning, w);
        } else if contains_any(&t, DESIGN_TITLE_TOKENS) {
            scores.add(ActivityKind::Design, w);
        } else if contains_any(&t, DOCS_TITLE_TOKENS) {
            scores.add(ActivityKind::Documentation, w);
        } else if contains_any(&t, COMM_TITLE_TOKENS) {
            scores.add(ActivityKind::Communication, w);
        } else if t.contains("reddit.com/r/") {
            if contains_any(&t, DEVELOPER_SUBREDDITS) {
                scores.add(ActivityKind::Research, w);
            } else {
                scores.add(ActivityKind::IdlePersonal, w);
            }
        } else if t.contains("localhost") || t.contains("127.0.0.1") {
            // Local dev server — the user is testing their own code.
            scores.add(ActivityKind::Coding, w);
        } else if contains_any(&t, RESEARCH_TITLE_TOKENS) {
            scores.add(ActivityKind::Research, w);
        } else if contains_any(&t, IDLE_TITLE_TOKENS) {
            scores.add(ActivityKind::IdlePersonal, w);
        } else {
            // Unknown title — small Research bump as benefit of the doubt.
            scores.add(ActivityKind::Research, w * 0.4);
        }
    }
}

/// OCR content scoring — same token lists, flat weight per match.
/// Multiple matches accumulate (e.g. kubectl + docker → stronger DevOps signal).
fn score_ocr(ocr_lc: &str, scores: &mut Scores) {
    if ocr_lc.is_empty() {
        return;
    }
    for token in DEVOPS_OCR_TOKENS {
        if ocr_lc.contains(token) {
            scores.add(ActivityKind::DeploymentDevops, 10.0);
        }
    }
    for token in CODE_OCR_TOKENS {
        if ocr_lc.contains(token) {
            scores.add(ActivityKind::Coding, 5.0);
        }
    }
    for token in MEETING_OCR_TOKENS {
        if ocr_lc.contains(token) {
            scores.add(ActivityKind::Meeting, 10.0);
        }
    }
    for token in DIFF_OCR_TOKENS {
        if ocr_lc.contains(token) {
            scores.add(ActivityKind::CodeReview, 7.0);
        }
    }
}

/// A11y element scoring.
/// Interactive roles (button, menuitem) multiply the weight — an "Approve"
/// button is stronger evidence than the word "approve" in a text node.
///
/// DevOps tokens are gated on interactive roles only: heading/tab elements
/// that happen to contain words like "deploy" or "trigger" in content text
/// (GitHub READMEs, YouTube titles, browser tab bars) must not score.
fn score_elements(elements: &[ElementSample], scores: &mut Scores) {
    for el in elements {
        let text_lc = el.text.to_lowercase();
        let is_interactive = el
            .role
            .as_deref()
            .map(|r| {
                INTERACTIVE_ROLES
                    .iter()
                    .any(|ir| r.eq_ignore_ascii_case(ir))
            })
            .unwrap_or(false);
        let role_multiplier = if is_interactive { 1.5_f32 } else { 1.0 };

        if is_interactive && contains_any(&text_lc, DEVOPS_ELEMENT_TOKENS) {
            scores.add(ActivityKind::DeploymentDevops, 10.0 * role_multiplier);
        }
        if contains_any(&text_lc, MEETING_ELEMENT_TOKENS) {
            scores.add(ActivityKind::Meeting, 12.0 * role_multiplier);
        }
        if contains_any(&text_lc, REVIEW_ELEMENT_TOKENS) {
            scores.add(ActivityKind::CodeReview, 12.0 * role_multiplier);
        }
    }
}

/// Clipboard and app-switch signals.
/// Clipboard is high-intent evidence — the user deliberately copied something.
fn score_signals(signals: &[SignalEvent], scores: &mut Scores) {
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
        }
        // PR URL in clipboard.
        if contains_any(&v, PR_TITLE_TOKENS) {
            scores.add(ActivityKind::CodeReview, 20.0);
        }
        // kubectl / terraform in clipboard (copied a command).
        if contains_any(&v, DEVOPS_OCR_TOKENS) {
            scores.add(ActivityKind::DeploymentDevops, 15.0);
        }
    }
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
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
                elements: &[],
                signals: &[],
                audio_present: $audio,
                duration_secs: 120,
            };
            categorize(&signals).0
        }};
    }

    // --- App name ---

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

    // --- The key edge case: VS Code + DevOps content ---

    #[test]
    fn vscode_kubectl_ocr_is_devops() {
        let titles = make_titles(&[("Dockerfile", 40), ("main.rs", 5)]);
        let signals = SessionSignals {
            app_name: "Visual Studio Code",
            window_titles: &titles,
            ocr_text: "kubectl get pods -n production\ndocker build -t myapp:latest .",
            elements: &[],
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
            elements: &[],
            signals: &[],
            audio_present: false,
            duration_secs: 600,
        };
        assert_eq!(categorize(&signals).0, ActivityKind::Coding);
    }

    // --- Terminal OCR ---

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

    // --- Browser window titles ---

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

    // --- Frequency weighting ---

    #[test]
    fn dockerfile_dominant_title_beats_ide_prior() {
        let titles = make_titles(&[("Dockerfile", 45), ("README.md", 5)]);
        let signals = SessionSignals {
            app_name: "code",
            window_titles: &titles,
            ocr_text: "",
            elements: &[],
            signals: &[],
            audio_present: false,
            duration_secs: 300,
        };
        assert_eq!(categorize(&signals).0, ActivityKind::DeploymentDevops);
    }

    // --- A11y elements ---

    #[test]
    fn approve_button_boosts_code_review() {
        let elements = vec![ElementSample {
            text: "Approve".to_string(),
            role: Some("button".to_string()),
            window_name: None,
            timestamp: "2024-01-01T00:00:00Z".to_string(),
        }];
        let signals = SessionSignals {
            app_name: "chrome",
            window_titles: &[],
            ocr_text: "",
            elements: &elements,
            signals: &[],
            audio_present: false,
            duration_secs: 120,
        };
        assert_eq!(categorize(&signals).0, ActivityKind::CodeReview);
    }

    #[test]
    fn deploy_button_boosts_devops() {
        let elements = vec![ElementSample {
            text: "Deploy to production".to_string(),
            role: Some("button".to_string()),
            window_name: None,
            timestamp: "2024-01-01T00:00:00Z".to_string(),
        }];
        let signals = SessionSignals {
            app_name: "chrome",
            window_titles: &[],
            ocr_text: "",
            elements: &elements,
            signals: &[],
            audio_present: false,
            duration_secs: 120,
        };
        assert_eq!(categorize(&signals).0, ActivityKind::DeploymentDevops);
    }

    // --- Clipboard signals ---

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
            elements: &[],
            signals: &sigs,
            audio_present: false,
            duration_secs: 120,
        };
        // cursor(25 Coding) + branch(20 Planning + 10 Coding) → Coding=35, Planning=20
        assert_eq!(categorize(&signals).0, ActivityKind::Coding);
    }

    // --- Confidence ---

    #[test]
    fn confidence_is_between_zero_and_one() {
        let signals = SessionSignals {
            app_name: "zoom",
            window_titles: &[],
            ocr_text: "",
            elements: &[],
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
