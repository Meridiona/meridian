//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

use super::*;

fn ft(text: &str) -> FrameText {
    FrameText {
        frame_id: 0,
        timestamp: "2024-01-01T10:00:00Z".to_owned(),
        full_text: text.to_owned(),
        text_source: "ocr".to_owned(),
    }
}

// ── is_landmark ──────────────────────────────────────────────────────────

#[test]
fn landmark_url() {
    assert!(is_landmark("https://github.com/meridiona/meridian/pull/42"));
    assert!(is_landmark(
        "check http://localhost:3939/sessions for results"
    ));
    assert!(!is_landmark("no url here at all"));
}

#[test]
fn landmark_shell_prompt() {
    assert!(is_landmark("$ cargo build --release"));
    assert!(is_landmark("% npm run dev"));
    assert!(is_landmark("> git status"));
    assert!(!is_landmark("plain text without prompt"));
}

#[test]
fn landmark_error_keywords() {
    assert!(is_landmark("error[E0308]: mismatched types"));
    assert!(is_landmark("FAILED: 3 tests failed"));
    assert!(is_landmark("Traceback (most recent call last):"));
    assert!(is_landmark("exit code 1"));
    assert!(is_landmark("WARNING: deprecated API used"));
}

#[test]
fn landmark_code_signatures() {
    assert!(is_landmark(
        "fn run_etl(screenpipe: &SqlitePool) -> Result<()> {"
    ));
    assert!(is_landmark(
        "def classify_session(session_id: int) -> dict:"
    ));
    assert!(is_landmark("class SessionBuilder:"));
    assert!(is_landmark("impl BlockContext {"));
    assert!(is_landmark("function renderDashboard(props) {"));
}

#[test]
fn landmark_sql() {
    assert!(is_landmark(
        "SELECT app_name, session_text FROM app_sessions"
    ));
    assert!(is_landmark(
        "INSERT INTO gaps (started_at, gap_secs) VALUES (?, ?)"
    ));
    assert!(is_landmark(
        "CREATE TABLE app_sessions (id INTEGER PRIMARY KEY)"
    ));
}

#[test]
fn landmark_git_ref() {
    assert!(is_landmark("feat/session-text-cleaner"));
    assert!(is_landmark("fix/dbeaver-truncation"));
    assert!(is_landmark("chore/bump-version"));
    assert!(is_landmark("refactor/etl-runner"));
}

#[test]
fn landmark_issue_ref() {
    assert!(is_landmark("closes #299"));
    assert!(is_landmark("see KAN-#42 for context")); // # followed by digits
    assert!(is_landmark("PR #1234"));
    assert!(!is_landmark("hash # alone")); // only 1 char after #
}

#[test]
fn landmark_commit_hash() {
    assert!(is_landmark("commit 8dc05aa chore(release): 1.62.0"));
    assert!(is_landmark("deadbeef")); // 8 hex chars with letters
    assert!(!is_landmark("20240101")); // 8 digits, no letters a-f
    assert!(!is_landmark("12345678")); // pure digits
    assert!(is_landmark("abc123def456789")); // 15 hex chars with letters
}

// ── is_log_noise ─────────────────────────────────────────────────────────

#[test]
fn log_noise_json_blob() {
    assert!(is_log_noise(
        r#"{"level":"info","message":"server started","timestamp":"2024-01-01"}"#
    ));
    assert!(is_log_noise(
        r#"{"service.name":"meridian","level":"DEBUG"}"#
    ));
    assert!(is_log_noise(r#"{"timestamp":"2024-01-01","msg":"ok"}"#));
    assert!(!is_log_noise(r#"{"action":"user_clicked_button"}"#)); // no level/timestamp/service.name
}

#[test]
fn log_noise_level_prefixed() {
    assert!(is_log_noise(
        "INFO:     Uvicorn running on http://0.0.0.0:7823"
    ));
    assert!(is_log_noise("INFO:agents.server:classify request received"));
    assert!(is_log_noise("DEBUG:sqlx:query SELECT * FROM app_sessions"));
    assert!(is_log_noise("WARNING:  StatReload detected changes"));
    assert!(is_log_noise("CRITICAL:app.main:unhandled exception"));
    // A plain sentence starting with "INFO" but not log format
    assert!(!is_log_noise(
        "Information about the project lives in README.md"
    ));
}

#[test]
fn log_noise_rust_tracing() {
    // Timestamped form
    assert!(is_log_noise(
        "2024-01-01T12:00:00Z  INFO meridian::etl: processing batch"
    ));
    assert!(is_log_noise(
        "2024-01-01T12:00:00+00:00  DEBUG sqlx::query: SELECT 1"
    ));
    assert!(!is_log_noise("the 2024-01-01 build completed successfully"));
    // Compact form (no timestamp — cargo watch / direct daemon output)
    assert!(is_log_noise(
        "INFO meridian::config: config loaded screenpipe_db=..."
    ));
    assert!(is_log_noise(
        "INFO startup_tick:run_etl:close_block: meridian::etl::block_ops: session closed"
    ));
    assert!(is_log_noise(
        "WARN sqlx::query: summary=\"SELECT app_name\" rows_returned=4"
    ));
    assert!(is_log_noise(
        "DEBUG meridian::intelligence::task_linker: calling mlx server"
    ));
    // Must NOT filter plain English sentences that happen to start with INFO
    assert!(!is_log_noise(
        "Information about the project lives in README.md"
    ));
    // Must NOT filter if no "::" module separator present
    assert!(!is_log_noise("INFO this is just a label not a module path"));
}

#[test]
fn log_noise_model_server() {
    assert!(is_log_noise("Active mem: 7.23 GB"));
    assert!(is_log_noise("Peak mem: 8.11 GB"));
    assert!(is_log_noise("Loading weights from mlx-community/Qwen3..."));
    assert!(is_log_noise("Compiling FSM for constrained decoding"));
    assert!(is_log_noise("FSM ready (3.2s)"));
    assert!(is_log_noise("Fetching 10 files: 100%"));
    assert!(!is_log_noise(
        "Fetching the latest session data from the database"
    ));
}

#[test]
fn log_noise_progress_bar() {
    assert!(is_log_noise(
        "Downloading: 100%|████████| 500/500 [00:01<00:00]"
    ));
    assert!(is_log_noise("  45%|░░░░░░░░░░░░░░░░░░░░░░████"));
    assert!(is_log_noise("epoch 3/10: loss=0.42  78%|████"));
    assert!(!is_log_noise("the pipeline ran at 100 percent efficiency"));
}

#[test]
fn log_noise_does_not_drop_landmark() {
    // An ERROR log is still a landmark even if it looks like a log line.
    // Callers must check is_landmark first; this test documents that
    // is_log_noise itself does NOT check for landmarks.
    let error_log = r#"{"level":"error","message":"borrow checker failed","file":"main.rs"}"#;
    // The JSON blob pattern fires because "level" is present
    assert!(is_log_noise(error_log));
    // But is_landmark also fires (contains "failed")
    assert!(is_landmark(error_log));
    // Callers use: if is_landmark → keep; else if is_log_noise → drop
}

// ── alphabetic_ratio ─────────────────────────────────────────────────────

#[test]
fn alpha_ratio_pure_alpha() {
    assert!((alphabetic_ratio("hello") - 1.0).abs() < 1e-9);
}

#[test]
fn alpha_ratio_mixed() {
    // "abc123" → 3 alpha / 6 total = 0.5
    assert!((alphabetic_ratio("abc123") - 0.5).abs() < 1e-9);
}

#[test]
fn alpha_ratio_symbols_only() {
    assert_eq!(alphabetic_ratio("!@#$%^&*()"), 0.0);
}

#[test]
fn alpha_ratio_empty() {
    assert_eq!(alphabetic_ratio(""), 0.0);
}

#[test]
fn alpha_ratio_spaces_count() {
    // "ab cd" → 4 alpha / 5 total = 0.8
    assert!((alphabetic_ratio("ab cd") - 0.8).abs() < 1e-9);
}

// ── is_quality_line ───────────────────────────────────────────────────────

#[test]
fn quality_line_passes_clean_content() {
    assert!(is_quality_line("the user opened a new file in VS Code"));
    assert!(is_quality_line(
        "running cargo build to compile the project"
    ));
}

#[test]
fn quality_line_drops_short() {
    assert!(!is_quality_line("cmd")); // len 3 < 15
    assert!(!is_quality_line("hello")); // len 5 < 15
    assert!(!is_quality_line("ok build")); // len 8 < 15
}

#[test]
fn quality_line_drops_log_noise() {
    assert!(!is_quality_line(
        "INFO:agents.server: request received from client"
    ));
    assert!(!is_quality_line(r#"{"level":"info","message":"done"}"#));
}

#[test]
fn quality_line_drops_low_alpha() {
    // Pure symbol noise — zero alphabetic chars
    assert!(!is_quality_line("!!! ??? --- === *** ||| >>>"));
    // Mostly digits with minimal alphabetic content: "0x0000 0x0001 0x0002 0x0003 0x0004"
    // 'x'×5 in 34 chars → 5/34 ≈ 0.15 < 0.35
    assert!(!is_quality_line("0x0000 0x0001 0x0002 0x0003 0x0004"));
}

// ── build_chrome_set ──────────────────────────────────────────────────────

#[test]
fn chrome_set_detects_repeated_ui_line() {
    // "File  Edit  View" appears in all 5 frames → chrome.
    // Content lines vary per frame so they appear in fewer than CHROME_FREQ_THRESHOLD frames.
    let frames = vec![
        ft("File  Edit  View\ncontent line alpha one two three"),
        ft("File  Edit  View\ncontent line beta four five six"),
        ft("File  Edit  View\ncontent line gamma seven eight"),
        ft("File  Edit  View\ncontent line delta nine ten"),
        ft("File  Edit  View\ncontent line epsilon eleven"),
    ];
    let chrome = build_chrome_set(&frames);
    assert!(
        chrome.contains("File  Edit  View"),
        "repeated UI line should be chrome"
    );
    // Each content line appears in only 1 frame — well below the threshold of 4.
    assert!(
        !chrome.contains("content line alpha one two three"),
        "unique line should not be chrome"
    );
    assert!(
        !chrome.contains("content line beta four five six"),
        "unique line should not be chrome"
    );
}

#[test]
fn chrome_set_requires_threshold() {
    // Line appears in 3 frames (below threshold of 4) → not chrome
    let mut frames: Vec<FrameText> = (0..3)
        .map(|_| ft("Sidebar  Navigator  Panel\nsome other content"))
        .collect();
    frames.push(ft("different content entirely"));
    let chrome = build_chrome_set(&frames);
    assert!(
        !chrome.contains("Sidebar  Navigator  Panel"),
        "3 frames < threshold 4"
    );
}

#[test]
fn chrome_set_landmark_not_chrome() {
    // An error message persisting across 5 frames is a landmark — must not be chrome
    let frames: Vec<FrameText> = (0..5)
        .map(|_| ft("error: borrow checker failed\nFile  Edit  View"))
        .collect();
    let chrome = build_chrome_set(&frames);
    assert!(
        !chrome.contains("error: borrow checker failed"),
        "landmark must not be in chrome set even if repeated"
    );
    assert!(
        chrome.contains("File  Edit  View"),
        "non-landmark repeated line should be chrome"
    );
}

#[test]
fn chrome_set_per_frame_dedup() {
    // Line appears 3 times within a SINGLE frame but only once per unique frame → count = 1
    let frames = vec![
        ft("repeated line\nrepeated line\nrepeated line\nother content"),
        ft("different frame content"),
    ];
    let chrome = build_chrome_set(&frames);
    assert!(
        !chrome.contains("repeated line"),
        "intra-frame repeats must not inflate count"
    );
}

// ── contains_ticket_key ───────────────────────────────────────────────────

#[test]
fn ticket_key_jira_linear_style() {
    assert!(is_landmark("KAN-141"));
    assert!(is_landmark("MER-42"));
    assert!(is_landmark("PROJ-1"));
    assert!(is_landmark("AB-999"));
    assert!(is_landmark("fixing KAN-141 in this session")); // embedded in prose
}

#[test]
fn ticket_key_negative_cases() {
    assert!(!is_landmark("KAN")); // no dash or digits
    assert!(!is_landmark("K-42")); // only 1 uppercase letter before dash
    assert!(!is_landmark("123-456")); // digits before dash, not uppercase
    assert!(!is_landmark("kan-141")); // lowercase
}

// ── is_chrome_exempt ──────────────────────────────────────────────────────

#[test]
fn chrome_exempt_excludes_hash_and_gt_prefixes() {
    // `# main.py` was incorrectly protected as a shell prompt by is_landmark
    // (which accepts `# ` prefix). is_chrome_exempt must NOT protect it.
    assert!(!is_chrome_exempt("# main.py"));
    assert!(!is_chrome_exempt("> blockquote text here"));
}

#[test]
fn chrome_exempt_keeps_interactive_prompts() {
    assert!(is_chrome_exempt("$ cargo build --release"));
    assert!(is_chrome_exempt("% npm run dev"));
    assert!(is_chrome_exempt("❯ git status"));
}

#[test]
fn chrome_exempt_anchors_sql_to_line_start() {
    // "last update" contains "update" but is NOT anchored to line start → not exempt
    assert!(!is_chrome_exempt("last update was three days ago"));
    // "select all" → same, contains "select" in the middle
    assert!(!is_chrome_exempt("press select all to copy text here"));
    // Anchored SQL → exempt
    assert!(is_chrome_exempt("SELECT id, name FROM users"));
    assert!(is_chrome_exempt("UPDATE sessions SET status = 'done'"));
}

#[test]
fn chrome_exempt_ticket_keys_exempt() {
    assert!(is_chrome_exempt("KAN-141"));
    assert!(is_chrome_exempt("fixing MER-42 with this commit"));
}

#[test]
fn chrome_set_uses_tighter_exemption() {
    // "# main.py" repeats in 5 frames — with the old is_landmark it would be
    // protected (starts with "# "); with is_chrome_exempt it should be chrome.
    let frames: Vec<FrameText> = (0..5)
        .map(|_| ft("# main.py\nsome other content line here"))
        .collect();
    let chrome = build_chrome_set(&frames);
    assert!(
        chrome.contains("# main.py"),
        "# main.py is not chrome-exempt and must be filtered as chrome"
    );
}
