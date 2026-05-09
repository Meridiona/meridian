// meridian — normalises screenpipe activity into structured app sessions

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// ActivityKind
// ---------------------------------------------------------------------------

/// The 10 mutually-exclusive activity categories assigned to every closed
/// `app_session`. Used downstream to gate PM task matching.
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
    /// Returns `true` if the category is eligible for PM task matching.
    /// `Meeting`, `Communication`, and `IdlePersonal` are excluded because
    /// they are unlikely to map 1-to-1 with a tracked ticket.
    pub fn is_pm_mappable(self) -> bool {
        !matches!(self, Self::Meeting | Self::Communication | Self::IdlePersonal)
    }

    /// Human-readable display name for the category.
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
}

// ---------------------------------------------------------------------------
// SessionSignals
// ---------------------------------------------------------------------------

/// Lightweight view of a closed `app_session` used as input to `categorize`.
/// All fields are borrowed to keep the function allocation-free.
pub struct SessionSignals<'a> {
    /// The raw app name (e.g. `"Cursor"`, `"com.google.Chrome"`).
    pub app_name: &'a str,
    /// All distinct window titles observed during the session.
    pub window_titles: &'a [String],
    /// Concatenated OCR text from up to 20 samples. May be empty.
    pub ocr_text: &'a str,
    /// `true` when the session's `audio_snippets` array is non-empty.
    pub audio_present: bool,
    /// Elapsed seconds from `started_at` to `ended_at`.
    pub duration_secs: u64,
}

// ---------------------------------------------------------------------------
// Static pattern tables
// ---------------------------------------------------------------------------

/// Maps a lowercase substring of `app_name` to its category.
/// Checked in order; first match wins.
static APP_PATTERNS: &[(&str, ActivityKind)] = &[
    // --- Meeting ---
    ("zoom", ActivityKind::Meeting),
    ("zoom.us", ActivityKind::Meeting),
    ("google meet", ActivityKind::Meeting),
    ("webex", ActivityKind::Meeting),
    ("whereby", ActivityKind::Meeting),
    // --- Coding IDEs ---
    ("cursor", ActivityKind::Coding),
    ("intellij", ActivityKind::Coding),
    ("pycharm", ActivityKind::Coding),
    ("webstorm", ActivityKind::Coding),
    ("goland", ActivityKind::Coding),
    ("rubymine", ActivityKind::Coding),
    ("clion", ActivityKind::Coding),
    ("xcode", ActivityKind::Coding),
    ("neovim", ActivityKind::Coding),
    ("nvim", ActivityKind::Coding),
    ("vim", ActivityKind::Coding),
    ("emacs", ActivityKind::Coding),
    ("zed", ActivityKind::Coding),
    ("sublime text", ActivityKind::Coding),
    ("nova", ActivityKind::Coding),
    ("helix", ActivityKind::Coding),
    // "code" is intentionally last in IDE group — very short, match after longer names
    ("code", ActivityKind::Coding),
    // --- Terminal (refined by OCR — see categorize_terminal) ---
    ("iterm", ActivityKind::Coding),
    ("warp", ActivityKind::Coding),
    ("alacritty", ActivityKind::Coding),
    ("kitty", ActivityKind::Coding),
    ("ghostty", ActivityKind::Coding),
    ("hyper", ActivityKind::Coding),
    ("terminal", ActivityKind::Coding),
    // --- Communication ---
    ("slack", ActivityKind::Communication),
    ("microsoft teams", ActivityKind::Communication),
    ("discord", ActivityKind::Communication),
    ("superhuman", ActivityKind::Communication),
    ("spark", ActivityKind::Communication),
    ("telegram", ActivityKind::Communication),
    ("whatsapp", ActivityKind::Communication),
    ("outlook", ActivityKind::Communication),
    ("linear", ActivityKind::Planning),
    ("mail", ActivityKind::Communication),
    // --- Design ---
    ("figma", ActivityKind::Design),
    ("sketch", ActivityKind::Design),
    ("adobe xd", ActivityKind::Design),
    ("framer", ActivityKind::Design),
    ("canva", ActivityKind::Design),
    ("affinity designer", ActivityKind::Design),
    ("whimsical", ActivityKind::Design),
    ("lottiefiles", ActivityKind::Design),
    // --- Documentation ---
    ("notion", ActivityKind::Documentation),
    ("obsidian", ActivityKind::Documentation),
    ("quip", ActivityKind::Documentation),
    ("bear", ActivityKind::Documentation),
    ("ulysses", ActivityKind::Documentation),
    ("confluence", ActivityKind::Documentation),
    // --- Planning ---
    ("asana", ActivityKind::Planning),
    ("trello", ActivityKind::Planning),
    ("monday", ActivityKind::Planning),
    ("clickup", ActivityKind::Planning),
    ("shortcut", ActivityKind::Planning),
    // --- Deployment / DevOps ---
    ("datadog", ActivityKind::DeploymentDevops),
    ("grafana", ActivityKind::DeploymentDevops),
    ("pagerduty", ActivityKind::DeploymentDevops),
    ("opsgenie", ActivityKind::DeploymentDevops),
    ("jenkins", ActivityKind::DeploymentDevops),
    ("argocd", ActivityKind::DeploymentDevops),
    // --- Browsers (refined by window title — see categorize_browser) ---
    ("chrome", ActivityKind::Research),
    ("safari", ActivityKind::Research),
    ("firefox", ActivityKind::Research),
    ("arc", ActivityKind::Research),
    ("edge", ActivityKind::Research),
    ("brave", ActivityKind::Research),
    ("opera", ActivityKind::Research),
    ("vivaldi", ActivityKind::Research),
];

/// Terminal app substrings — used to decide whether to run OCR refinement.
static TERMINAL_APPS: &[&str] = &[
    "terminal", "iterm", "warp", "alacritty", "kitty", "ghostty", "hyper",
];

/// Browser app substrings — used to decide whether to run title refinement.
static BROWSER_APPS: &[&str] = &[
    "chrome", "safari", "firefox", "arc", "edge", "brave", "opera", "vivaldi",
];

/// DevOps-specific CLI tool patterns (checked against lowercased OCR text).
static DEVOPS_OCR_TOKENS: &[&str] = &[
    "kubectl", "docker", "terraform", "helm", "ansible", "aws ", "gcloud",
    "az ", "heroku", "vercel", "netlify", "k8s", "eksctl", "flyctl",
];

/// Code syntax tokens for content-based fallback (checked against lowercased OCR).
static CODE_OCR_TOKENS: &[&str] = &[
    "fn ", "def ", "class ", "import ", "const ", "async ", "#include",
];

/// Meeting UI tokens for content-based fallback (checked against lowercased OCR).
static MEETING_OCR_TOKENS: &[&str] = &[
    "mute", "unmute", "leave meeting", "share screen",
];

// ---------------------------------------------------------------------------
// Browser title pattern tables
// ---------------------------------------------------------------------------

static PR_TITLE_TOKENS: &[&str] = &[
    "pull request", "/pull/", "merge request", "/merge_requests/",
];

static DEVOPS_TITLE_TOKENS: &[&str] = &[
    "console.aws", "console.cloud.google", "portal.azure",
    "github.com/", // combined with "actions" check below via contains_any
    "circleci.com", "app.datadoghq", "grafana", "vercel.com/deployments",
    "netlify.com",
];

/// Separate token that must co-occur with "github.com/" for DevOps classification.
const GITHUB_ACTIONS_TOKEN: &str = "actions";

static PLANNING_TITLE_TOKENS: &[&str] = &[
    "jira", "linear.app", "/issues", "/projects",
    "asana.com", "trello.com", "monday.com",
];

static DESIGN_TITLE_TOKENS: &[&str] = &["figma.com"];

static DOCS_TITLE_TOKENS: &[&str] = &[
    "confluence", "notion.so", "docs.google.com", "gitbook", "readme.io",
];

static COMM_TITLE_TOKENS: &[&str] = &[
    "mail.google.com", "outlook.live", "outlook.office", "app.slack.com",
];

static RESEARCH_TITLE_TOKENS: &[&str] = &[
    "stackoverflow.com", "docs.rs", "developer.mozilla", "developer.apple",
    "pkg.go.dev", "npmjs.com", "pypi.org", "crates.io",
    "github.com", "gitlab.com", "arxiv.org",
    "reddit.com/r/programming", "hackernews", "news.ycombinator",
    "medium.com", "dev.to", "udemy.com", "coursera.org",
    "egghead.io", "frontendmasters.com", "pluralsight.com", "oreilly.com",
];

static IDLE_TITLE_TOKENS: &[&str] = &[
    "youtube.com", "netflix.com", "twitter.com", "x.com",
    "instagram.com", "facebook.com", "tiktok.com", "twitch.tv", "spotify.com",
    "news.", "nytimes.com", "bbc.com",
];

/// Developer-focused subreddits classified as Research rather than IdlePersonal.
static DEVELOPER_SUBREDDITS: &[&str] = &[
    "/r/rust", "/r/programming", "/r/learnprogramming", "/r/webdev",
    "/r/devops", "/r/python", "/r/javascript", "/r/typescript", "/r/golang",
    "/r/swift", "/r/kotlin", "/r/java", "/r/cpp", "/r/cscareerquestions",
    "/r/sysadmin", "/r/netsec", "/r/machinelearning", "/r/datascience",
    "/r/compsci", "/r/linux", "/r/opensource",
];

// ---------------------------------------------------------------------------
// Public: categorize
// ---------------------------------------------------------------------------

/// Classifies a closed session into an `ActivityKind` using a priority
/// waterfall.  The function is pure — no I/O, no allocations beyond small
/// temporaries, no locks.
pub fn categorize(signals: &SessionSignals<'_>) -> ActivityKind {
    let app_lc = signals.app_name.to_lowercase();

    // -----------------------------------------------------------------------
    // Priority 1 — Meeting: audio present AND app is a known meeting app
    // -----------------------------------------------------------------------
    if signals.audio_present && is_meeting_app(&app_lc) {
        return ActivityKind::Meeting;
    }

    // -----------------------------------------------------------------------
    // Priority 2 — Known app name lookup
    // -----------------------------------------------------------------------
    // Terminal and browser apps need further refinement; match them first to
    // decide the refinement path, but do not return their raw category yet.

    if is_terminal_app(&app_lc) {
        // Priority 3 — Terminal OCR refinement
        let ocr_lc = signals.ocr_text.to_lowercase();
        return categorize_terminal(&ocr_lc);
    }

    if is_browser_app(&app_lc) {
        // Priority 4 — Browser window-title refinement
        return categorize_browser(signals.window_titles);
    }

    // Non-terminal, non-browser: look up in the static table.
    for &(pattern, kind) in APP_PATTERNS {
        if app_lc.contains(pattern) {
            return kind;
        }
    }

    // -----------------------------------------------------------------------
    // Priority 5 — Content-based fallback (no app matched)
    // -----------------------------------------------------------------------
    let ocr_lc = signals.ocr_text.to_lowercase();

    if contains_any(&ocr_lc, CODE_OCR_TOKENS) {
        return ActivityKind::Coding;
    }

    if contains_any(&ocr_lc, MEETING_OCR_TOKENS) {
        return ActivityKind::Meeting;
    }

    ActivityKind::IdlePersonal
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Returns `true` if `app` (already lowercased) is a known meeting application.
fn is_meeting_app(app: &str) -> bool {
    matches_any_pattern(app, &["zoom", "zoom.us", "google meet", "webex", "whereby", "microsoft teams"])
}

/// Returns `true` if `app` (already lowercased) is a known terminal emulator.
fn is_terminal_app(app: &str) -> bool {
    matches_any_pattern(app, TERMINAL_APPS)
}

/// Returns `true` if `app` (already lowercased) is a known browser.
fn is_browser_app(app: &str) -> bool {
    matches_any_pattern(app, BROWSER_APPS)
}

/// Checks whether any of the given substrings appear in `app`.
fn matches_any_pattern(app: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|p| app.contains(p))
}

/// Refines a terminal session using lowercased OCR text.
fn categorize_terminal(ocr_lc: &str) -> ActivityKind {
    if contains_any(ocr_lc, DEVOPS_OCR_TOKENS) {
        ActivityKind::DeploymentDevops
    } else {
        ActivityKind::Coding
    }
}

/// Refines a browser session using its window titles, walking priority rules.
fn categorize_browser(titles: &[String]) -> ActivityKind {
    // Work with lowercased copies so each comparison is case-insensitive.
    let lc_titles: Vec<String> = titles.iter().map(|t| t.to_lowercase()).collect();

    // 1. PR / diff
    if lc_titles.iter().any(|t| contains_any(t, PR_TITLE_TOKENS)) {
        return ActivityKind::CodeReview;
    }

    // 2. DevOps console — generic tokens
    //    Special case: "github.com/" only counts if "actions" also appears.
    let has_devops = lc_titles.iter().any(|t| {
        let has_generic = DEVOPS_TITLE_TOKENS
            .iter()
            .filter(|&&tok| tok != "github.com/")
            .any(|tok| t.contains(tok));
        let has_gh_actions =
            t.contains("github.com/") && t.contains(GITHUB_ACTIONS_TOKEN);
        has_generic || has_gh_actions
    });
    if has_devops {
        return ActivityKind::DeploymentDevops;
    }

    // 3. Planning
    if lc_titles.iter().any(|t| contains_any(t, PLANNING_TITLE_TOKENS)) {
        return ActivityKind::Planning;
    }

    // 4. Design
    if lc_titles.iter().any(|t| contains_any(t, DESIGN_TITLE_TOKENS)) {
        return ActivityKind::Design;
    }

    // 5. Documentation / writing
    if lc_titles.iter().any(|t| contains_any(t, DOCS_TITLE_TOKENS)) {
        return ActivityKind::Documentation;
    }

    // 6. Communication
    if lc_titles.iter().any(|t| contains_any(t, COMM_TITLE_TOKENS)) {
        return ActivityKind::Communication;
    }

    // 7. Reddit special case: developer subreddits → Research, others → IdlePersonal.
    //    Handled before the general research/idle passes so the blanket reddit.com
    //    pattern does not swallow developer communities.
    if let Some(title) = lc_titles.iter().find(|t| t.contains("reddit.com/r/")) {
        if contains_any(title, DEVELOPER_SUBREDDITS) {
            return ActivityKind::Research;
        }
        return ActivityKind::IdlePersonal;
    }

    // 8. Research — developer-focused sites
    //    "github.com" only counts here if it wasn't caught by PR/devops above.
    if lc_titles.iter().any(|t| contains_any(t, RESEARCH_TITLE_TOKENS)) {
        return ActivityKind::Research;
    }

    // 9. Idle / personal
    if lc_titles.iter().any(|t| contains_any(t, IDLE_TITLE_TOKENS)) {
        return ActivityKind::IdlePersonal;
    }

    // 9. Unknown browser content — default to Research (benefit of the doubt)
    ActivityKind::Research
}

/// Returns `true` if `haystack` contains at least one of `needles`.
fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

#[cfg(test)]
mod tests {
    use super::{categorize, ActivityKind, SessionSignals};

    macro_rules! cat {
        ($app:expr, $titles:expr, $ocr:expr, $audio:expr) => {{
            let titles: Vec<String> = $titles.iter().map(|s| s.to_string()).collect();
            let signals = SessionSignals {
                app_name: $app,
                window_titles: &titles,
                ocr_text: $ocr,
                audio_present: $audio,
                duration_secs: 120,
            };
            categorize(&signals)
        }};
    }

    // --- 1. App name matching ---

    #[test]
    fn test_vscode_is_coding() {
        assert_eq!(cat!("vscode", &[] as &[&str], "", false), ActivityKind::Coding);
    }

    #[test]
    fn test_code_is_coding() {
        assert_eq!(cat!("code", &[] as &[&str], "", false), ActivityKind::Coding);
    }

    #[test]
    fn test_cursor_is_coding() {
        assert_eq!(cat!("cursor", &[] as &[&str], "", false), ActivityKind::Coding);
    }

    #[test]
    fn test_intellij_idea_is_coding() {
        assert_eq!(cat!("intellij idea", &[] as &[&str], "", false), ActivityKind::Coding);
    }

    #[test]
    fn test_xcode_is_coding() {
        assert_eq!(cat!("xcode", &[] as &[&str], "", false), ActivityKind::Coding);
    }

    #[test]
    fn test_slack_no_audio_is_communication() {
        assert_eq!(cat!("slack", &[] as &[&str], "", false), ActivityKind::Communication);
    }

    #[test]
    fn test_microsoft_teams_no_audio_is_communication() {
        assert_eq!(cat!("microsoft teams", &[] as &[&str], "", false), ActivityKind::Communication);
    }

    #[test]
    fn test_figma_app_is_design() {
        assert_eq!(cat!("figma", &[] as &[&str], "", false), ActivityKind::Design);
    }

    #[test]
    fn test_notion_app_is_documentation() {
        assert_eq!(cat!("notion", &[] as &[&str], "", false), ActivityKind::Documentation);
    }

    #[test]
    fn test_zoom_with_audio_is_meeting() {
        assert_eq!(cat!("zoom", &[] as &[&str], "", true), ActivityKind::Meeting);
    }

    #[test]
    fn test_zoom_without_audio_is_still_meeting() {
        assert_eq!(cat!("zoom", &[] as &[&str], "", false), ActivityKind::Meeting);
    }

    #[test]
    fn test_datadog_is_devops() {
        assert_eq!(cat!("datadog", &[] as &[&str], "", false), ActivityKind::DeploymentDevops);
    }

    #[test]
    fn test_pagerduty_is_devops() {
        assert_eq!(cat!("pagerduty", &[] as &[&str], "", false), ActivityKind::DeploymentDevops);
    }

    // --- 2. Mixed-case app name ---

    #[test]
    fn test_visual_studio_code_mixed_case_is_coding() {
        assert_eq!(cat!("Visual Studio Code", &[] as &[&str], "", false), ActivityKind::Coding);
    }

    #[test]
    fn test_arc_capital_a_is_research_via_browser_unknown_title() {
        assert_eq!(
            cat!("Arc", &["somecompany-internal-tool.com"] as &[&str], "", false),
            ActivityKind::Research
        );
    }

    // --- 3. Terminal OCR refinement ---

    #[test]
    fn test_terminal_kubectl_is_devops() {
        assert_eq!(cat!("terminal", &[] as &[&str], "kubectl get pods -n production", false), ActivityKind::DeploymentDevops);
    }

    #[test]
    fn test_terminal_docker_is_devops() {
        assert_eq!(cat!("terminal", &[] as &[&str], "docker build -t myapp:latest .", false), ActivityKind::DeploymentDevops);
    }

    #[test]
    fn test_terminal_terraform_is_devops() {
        assert_eq!(cat!("terminal", &[] as &[&str], "terraform apply -auto-approve", false), ActivityKind::DeploymentDevops);
    }

    #[test]
    fn test_terminal_helm_is_devops() {
        assert_eq!(cat!("terminal", &[] as &[&str], "helm upgrade --install myrelease ./chart", false), ActivityKind::DeploymentDevops);
    }

    #[test]
    fn test_terminal_ansible_is_devops() {
        assert_eq!(cat!("terminal", &[] as &[&str], "ansible-playbook -i inventory site.yml", false), ActivityKind::DeploymentDevops);
    }

    #[test]
    fn test_terminal_aws_cli_is_devops() {
        assert_eq!(cat!("terminal", &[] as &[&str], "aws s3 cp dist/ s3://my-bucket --recursive", false), ActivityKind::DeploymentDevops);
    }

    #[test]
    fn test_terminal_gcloud_is_devops() {
        assert_eq!(cat!("terminal", &[] as &[&str], "gcloud run deploy myservice --region us-central1", false), ActivityKind::DeploymentDevops);
    }

    #[test]
    fn test_terminal_az_cli_is_devops() {
        assert_eq!(cat!("terminal", &[] as &[&str], "az webapp up --name myapp", false), ActivityKind::DeploymentDevops);
    }

    #[test]
    fn test_terminal_heroku_is_devops() {
        assert_eq!(cat!("terminal", &[] as &[&str], "heroku releases --app myapp", false), ActivityKind::DeploymentDevops);
    }

    #[test]
    fn test_terminal_vercel_is_devops() {
        assert_eq!(cat!("terminal", &[] as &[&str], "vercel --prod", false), ActivityKind::DeploymentDevops);
    }

    #[test]
    fn test_terminal_netlify_is_devops() {
        assert_eq!(cat!("terminal", &[] as &[&str], "netlify deploy --prod", false), ActivityKind::DeploymentDevops);
    }

    #[test]
    fn test_terminal_k8s_keyword_is_devops() {
        assert_eq!(cat!("terminal", &[] as &[&str], "k8s cluster-info", false), ActivityKind::DeploymentDevops);
    }

    #[test]
    fn test_iterm2_kubectl_is_devops() {
        assert_eq!(cat!("iterm2", &[] as &[&str], "kubectl rollout status deploy/api", false), ActivityKind::DeploymentDevops);
    }

    #[test]
    fn test_terminal_git_commit_is_coding() {
        assert_eq!(cat!("terminal", &[] as &[&str], "git commit -m 'fix: handle nil pointer'", false), ActivityKind::Coding);
    }

    #[test]
    fn test_terminal_cargo_build_is_coding() {
        assert_eq!(cat!("terminal", &[] as &[&str], "cargo build --release", false), ActivityKind::Coding);
    }

    #[test]
    fn test_terminal_npm_install_is_coding() {
        assert_eq!(cat!("terminal", &[] as &[&str], "npm install --save-dev jest", false), ActivityKind::Coding);
    }

    #[test]
    fn test_terminal_empty_ocr_is_coding() {
        assert_eq!(cat!("terminal", &[] as &[&str], "", false), ActivityKind::Coding);
    }

    // --- 4. Browser window-title refinement ---

    #[test]
    fn test_safari_github_pull_request_is_code_review() {
        assert_eq!(cat!("safari", &["github.com/org/repo/pull/123"] as &[&str], "", false), ActivityKind::CodeReview);
    }

    #[test]
    fn test_chrome_gitlab_merge_request_is_code_review() {
        assert_eq!(cat!("chrome", &["gitlab.com/org/repo/-/merge_requests/5"] as &[&str], "", false), ActivityKind::CodeReview);
    }

    #[test]
    fn test_arc_pull_request_keyword_is_code_review() {
        assert_eq!(cat!("arc", &["Review pull request #42 — myorg/myrepo"] as &[&str], "", false), ActivityKind::CodeReview);
    }

    #[test]
    fn test_browser_aws_console_is_devops() {
        assert_eq!(cat!("chrome", &["console.aws.amazon.com/ec2/v2/home"] as &[&str], "", false), ActivityKind::DeploymentDevops);
    }

    #[test]
    fn test_browser_gcp_console_is_devops() {
        assert_eq!(cat!("chrome", &["console.cloud.google.com/kubernetes"] as &[&str], "", false), ActivityKind::DeploymentDevops);
    }

    #[test]
    fn test_browser_azure_portal_is_devops() {
        assert_eq!(cat!("safari", &["portal.azure.com/#blade/resource-groups"] as &[&str], "", false), ActivityKind::DeploymentDevops);
    }

    #[test]
    fn test_browser_github_actions_is_devops() {
        assert_eq!(cat!("chrome", &["github.com/org/repo/actions/runs/9999"] as &[&str], "", false), ActivityKind::DeploymentDevops);
    }

    #[test]
    fn test_browser_circleci_is_devops() {
        assert_eq!(cat!("chrome", &["app.circleci.com/pipelines/github/org/repo"] as &[&str], "", false), ActivityKind::DeploymentDevops);
    }

    #[test]
    fn test_browser_linear_issues_is_planning() {
        assert_eq!(cat!("arc", &["app.linear.app/team/issues"] as &[&str], "", false), ActivityKind::Planning);
    }

    #[test]
    fn test_browser_jira_board_is_planning() {
        assert_eq!(cat!("chrome", &["org.atlassian.net/jira/board"] as &[&str], "", false), ActivityKind::Planning);
    }

    #[test]
    fn test_browser_asana_is_planning() {
        assert_eq!(cat!("chrome", &["app.asana.com/0/123456789"] as &[&str], "", false), ActivityKind::Planning);
    }

    #[test]
    fn test_browser_trello_is_planning() {
        assert_eq!(cat!("safari", &["trello.com/b/boardid/sprint-board"] as &[&str], "", false), ActivityKind::Planning);
    }

    #[test]
    fn test_browser_github_projects_is_planning() {
        assert_eq!(cat!("chrome", &["github.com/orgs/myorg/projects/3"] as &[&str], "", false), ActivityKind::Planning);
    }

    #[test]
    fn test_browser_figma_is_design() {
        assert_eq!(cat!("chrome", &["figma.com/file/abc123/Dashboard-v2"] as &[&str], "", false), ActivityKind::Design);
    }

    #[test]
    fn test_browser_notion_is_documentation() {
        assert_eq!(cat!("arc", &["notion.so/myworkspace/Architecture-Overview"] as &[&str], "", false), ActivityKind::Documentation);
    }

    #[test]
    fn test_browser_confluence_is_documentation() {
        assert_eq!(cat!("chrome", &["confluence.atlassian.net/wiki/spaces/ENG/overview"] as &[&str], "", false), ActivityKind::Documentation);
    }

    #[test]
    fn test_browser_google_docs_is_documentation() {
        assert_eq!(cat!("chrome", &["docs.google.com/document/d/abc123/edit"] as &[&str], "", false), ActivityKind::Documentation);
    }

    #[test]
    fn test_browser_gmail_is_communication() {
        assert_eq!(cat!("chrome", &["mail.google.com/mail/u/0/#inbox"] as &[&str], "", false), ActivityKind::Communication);
    }

    #[test]
    fn test_browser_outlook_live_is_communication() {
        assert_eq!(cat!("safari", &["outlook.live.com/mail/0/inbox"] as &[&str], "", false), ActivityKind::Communication);
    }

    #[test]
    fn test_browser_slack_web_is_communication() {
        assert_eq!(cat!("chrome", &["app.slack.com/client/T0000/C0000"] as &[&str], "", false), ActivityKind::Communication);
    }

    #[test]
    fn test_browser_stackoverflow_is_research() {
        assert_eq!(cat!("chrome", &["stackoverflow.com/questions/123456/how-to-use-tokio"] as &[&str], "", false), ActivityKind::Research);
    }

    #[test]
    fn test_browser_docs_rs_is_research() {
        assert_eq!(cat!("safari", &["docs.rs/anyhow/1.0.75/anyhow/"] as &[&str], "", false), ActivityKind::Research);
    }

    #[test]
    fn test_browser_crates_io_is_research() {
        assert_eq!(cat!("arc", &["crates.io/crates/tokio"] as &[&str], "", false), ActivityKind::Research);
    }

    #[test]
    fn test_browser_mdn_is_research() {
        assert_eq!(cat!("chrome", &["developer.mozilla.org/en-US/docs/Web/JavaScript"] as &[&str], "", false), ActivityKind::Research);
    }

    #[test]
    fn test_browser_udemy_is_research() {
        assert_eq!(cat!("chrome", &["udemy.com/course/the-complete-rust-programming-course"] as &[&str], "", false), ActivityKind::Research);
    }

    #[test]
    fn test_browser_youtube_is_idle_personal() {
        assert_eq!(cat!("chrome", &["youtube.com/watch?v=dQw4w9WgXcQ"] as &[&str], "", false), ActivityKind::IdlePersonal);
    }

    #[test]
    fn test_browser_netflix_is_idle_personal() {
        assert_eq!(cat!("safari", &["netflix.com/watch/12345678"] as &[&str], "", false), ActivityKind::IdlePersonal);
    }

    #[test]
    fn test_browser_twitter_is_idle_personal() {
        assert_eq!(cat!("chrome", &["twitter.com/home"] as &[&str], "", false), ActivityKind::IdlePersonal);
    }

    #[test]
    fn test_browser_instagram_is_idle_personal() {
        assert_eq!(cat!("safari", &["instagram.com/"] as &[&str], "", false), ActivityKind::IdlePersonal);
    }

    #[test]
    fn test_browser_reddit_developer_community_is_research() {
        assert_eq!(cat!("chrome", &["reddit.com/r/rust/comments/abc/my_post"] as &[&str], "", false), ActivityKind::Research);
    }

    #[test]
    fn test_browser_reddit_gaming_is_idle_personal() {
        assert_eq!(cat!("chrome", &["reddit.com/r/gaming/comments/xyz/cool_game"] as &[&str], "", false), ActivityKind::IdlePersonal);
    }

    #[test]
    fn test_browser_unknown_internal_tool_is_research() {
        assert_eq!(cat!("chrome", &["somecompany-internal-tool.com/dashboard"] as &[&str], "", false), ActivityKind::Research);
    }

    // --- 5. Audio override ---

    #[test]
    fn test_zoom_audio_false_is_still_meeting() {
        assert_eq!(cat!("zoom", &[] as &[&str], "", false), ActivityKind::Meeting);
    }

    #[test]
    fn test_slack_audio_true_is_communication() {
        // Slack is mapped to Communication at tier 2; audio alone does not promote it.
        assert_eq!(cat!("slack", &[] as &[&str], "", true), ActivityKind::Communication);
    }

    #[test]
    fn test_microsoft_teams_audio_true_is_meeting() {
        assert_eq!(cat!("microsoft teams", &[] as &[&str], "", true), ActivityKind::Meeting);
    }

    // --- 6. OCR fallback ---

    #[test]
    fn test_ocr_code_syntax_unknown_app_is_coding() {
        let titles: Vec<String> = Vec::new();
        let signals = SessionSignals {
            app_name: "unknownide",
            window_titles: &titles,
            ocr_text: "fn main() {\n    let x: u32 = 42;\n    println!(\"{}\", x);\n}",
            audio_present: false,
            duration_secs: 60,
        };
        assert_eq!(categorize(&signals), ActivityKind::Coding);
    }

    #[test]
    fn test_ocr_meeting_ui_unknown_app_is_meeting() {
        let titles: Vec<String> = Vec::new();
        let signals = SessionSignals {
            app_name: "unknownmeetingapp",
            window_titles: &titles,
            ocr_text: "You are muted  •  12 participants  •  Leave meeting",
            audio_present: false,
            duration_secs: 900,
        };
        assert_eq!(categorize(&signals), ActivityKind::Meeting);
    }

    #[test]
    fn test_ocr_fallback_nothing_recognisable_is_idle_personal() {
        let titles: Vec<String> = Vec::new();
        let signals = SessionSignals {
            app_name: "unknownapp",
            window_titles: &titles,
            ocr_text: "The quick brown fox jumps over the lazy dog.",
            audio_present: false,
            duration_secs: 45,
        };
        assert_eq!(categorize(&signals), ActivityKind::IdlePersonal);
    }

    // --- 7. Edge cases ---

    #[test]
    fn test_empty_app_no_titles_no_ocr_is_idle_personal() {
        let titles: Vec<String> = Vec::new();
        let signals = SessionSignals {
            app_name: "",
            window_titles: &titles,
            ocr_text: "",
            audio_present: false,
            duration_secs: 30,
        };
        assert_eq!(categorize(&signals), ActivityKind::IdlePersonal);
    }

    #[test]
    fn test_very_short_session_still_returns_a_category() {
        let titles: Vec<String> = vec!["vscode — main.rs".to_string()];
        let signals = SessionSignals {
            app_name: "vscode",
            window_titles: &titles,
            ocr_text: "",
            audio_present: false,
            duration_secs: 5,
        };
        assert_eq!(categorize(&signals), ActivityKind::Coding);
    }

    #[test]
    fn test_duration_zero_does_not_panic() {
        let titles: Vec<String> = Vec::new();
        let signals = SessionSignals {
            app_name: "notion",
            window_titles: &titles,
            ocr_text: "",
            audio_present: false,
            duration_secs: 0,
        };
        assert_eq!(categorize(&signals), ActivityKind::Documentation);
    }

    #[test]
    fn test_multiple_window_titles_any_can_match() {
        // First title is neutral; second triggers CodeReview.
        assert_eq!(
            cat!(
                "chrome",
                &["GitHub — Notifications", "github.com/org/repo/pull/77 — Add feature X"] as &[&str],
                "",
                false
            ),
            ActivityKind::CodeReview
        );
    }

    #[test]
    fn test_browser_title_mixed_case_pull_request_phrase() {
        // Window title with capital letters must still match.
        assert_eq!(
            cat!("safari", &["Review Pull Request #10 — myorg/myrepo"] as &[&str], "", false),
            ActivityKind::CodeReview
        );
    }

    // --- 8. is_pm_mappable exhaustive ---

    #[test]
    fn test_is_pm_mappable_false_for_meeting() { assert!(!ActivityKind::Meeting.is_pm_mappable()); }

    #[test]
    fn test_is_pm_mappable_false_for_communication() { assert!(!ActivityKind::Communication.is_pm_mappable()); }

    #[test]
    fn test_is_pm_mappable_false_for_idle_personal() { assert!(!ActivityKind::IdlePersonal.is_pm_mappable()); }

    #[test]
    fn test_is_pm_mappable_true_for_coding() { assert!(ActivityKind::Coding.is_pm_mappable()); }

    #[test]
    fn test_is_pm_mappable_true_for_code_review() { assert!(ActivityKind::CodeReview.is_pm_mappable()); }

    #[test]
    fn test_is_pm_mappable_true_for_documentation() { assert!(ActivityKind::Documentation.is_pm_mappable()); }

    #[test]
    fn test_is_pm_mappable_true_for_research() { assert!(ActivityKind::Research.is_pm_mappable()); }

    #[test]
    fn test_is_pm_mappable_true_for_planning() { assert!(ActivityKind::Planning.is_pm_mappable()); }

    #[test]
    fn test_is_pm_mappable_true_for_deployment_devops() { assert!(ActivityKind::DeploymentDevops.is_pm_mappable()); }

    #[test]
    fn test_is_pm_mappable_true_for_design() { assert!(ActivityKind::Design.is_pm_mappable()); }

    #[test]
    fn test_pm_mappable_count_is_seven() {
        let all = [
            ActivityKind::Coding, ActivityKind::CodeReview, ActivityKind::Documentation,
            ActivityKind::Research, ActivityKind::Communication, ActivityKind::Meeting,
            ActivityKind::Planning, ActivityKind::DeploymentDevops, ActivityKind::Design,
            ActivityKind::IdlePersonal,
        ];
        let count = all.iter().filter(|k| k.is_pm_mappable()).count();
        assert_eq!(count, 7, "expected exactly 7 pm-mappable variants");
    }
}
