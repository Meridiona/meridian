//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

use super::ActivityKind;

/// `(app_name_substring, category, weight)`.
/// Specific apps (zoom, figma) get higher weight — they're unambiguous.
/// Generic apps (code, terminal, browser) get lower weight — content can override.
pub(super) static APP_PATTERNS: &[(&str, ActivityKind, f32)] = &[
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
pub(super) static TERMINAL_APPS: &[&str] = &[
    "terminal",
    "iterm",
    "warp",
    "alacritty",
    "kitty",
    "ghostty",
    "hyper",
];

/// Browsers — low base weight; window titles decide the real category.
pub(super) static BROWSER_APPS: &[&str] = &[
    "chrome", "safari", "firefox", "arc", "edge", "brave", "opera", "vivaldi",
];

// OCR / content tokens
pub(super) static DEVOPS_OCR_TOKENS: &[&str] = &[
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
pub(super) static CODE_OCR_TOKENS: &[&str] = &[
    "fn ", "def ", "class ", "import ", "const ", "async ", "#include",
];
pub(super) static MEETING_OCR_TOKENS: &[&str] =
    &["mute", "unmute", "leave meeting", "share screen"];
pub(super) static DIFF_OCR_TOKENS: &[&str] = &["+++", "@@"];

// Window-title tokens
pub(super) static PR_TITLE_TOKENS: &[&str] = &[
    "pull request",
    "/pull/",
    "merge request",
    "/merge_requests/",
];
pub(super) static DEVOPS_TITLE_TOKENS: &[&str] = &[
    "console.aws",
    "console.cloud.google",
    "portal.azure",
    "circleci.com",
    "app.datadoghq",
    "vercel.com/deployments",
    "netlify.com",
];
pub(super) static PLANNING_TITLE_TOKENS: &[&str] = &[
    "jira",
    "linear.app",
    "/issues",
    "/projects",
    "asana.com",
    "trello.com",
    "monday.com",
];
pub(super) static DESIGN_TITLE_TOKENS: &[&str] = &["figma.com"];
pub(super) static DOCS_TITLE_TOKENS: &[&str] = &[
    "confluence",
    "notion.so",
    "docs.google.com",
    "gitbook",
    "readme.io",
];
pub(super) static COMM_TITLE_TOKENS: &[&str] = &[
    "mail.google.com",
    "outlook.live",
    "outlook.office",
    "app.slack.com",
    "chat.google.com",
    "teams.microsoft.com",
    "discord.com/channels",
];
pub(super) static RESEARCH_TITLE_TOKENS: &[&str] = &[
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
pub(super) static IDLE_TITLE_TOKENS: &[&str] = &[
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
pub(super) static DEVELOPER_SUBREDDITS: &[&str] = &[
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
pub(super) static DEVOPS_WINDOW_TOKENS: &[&str] = &[
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

// Clipboard / signal tokens
pub(super) static BRANCH_PREFIXES: &[&str] = &[
    "feat/",
    "fix/",
    "chore/",
    "refactor/",
    "hotfix/",
    "release/",
];

/// Minimum confidence fraction for the winner to be trusted.
/// Below this, every signal was weak — fall back to IdlePersonal.
pub(super) const CONFIDENCE_FLOOR: f32 = 0.35;
