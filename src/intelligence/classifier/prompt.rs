// meridian — normalises screenpipe activity into structured app sessions

use super::ClassifyRequest;
use std::collections::HashSet;

pub const SYSTEM: &str = "\
You are a JSON-only classifier. Match a developer screen session to a Jira issue.\n\
Return {\"issue_key\": null} for: music, system settings, idle time, social media,\n\
or anything unrelated to software development work.\n\
Only return keys from the provided issue list — never invent a key.";

pub fn build_prompts(req: &ClassifyRequest) -> (String, String) {
    let issues_text: String = req
        .tasks
        .iter()
        .map(|t| format!("- {}: {}", t.key, t.title))
        .collect::<Vec<_>>()
        .join("\n");

    let windows = if req.windows.is_empty() {
        "(none)".to_string()
    } else {
        req.windows.join(" | ")
    };

    let ocr = if req.ocr_snippet.is_empty() {
        "(none)".to_string()
    } else {
        req.ocr_snippet.clone()
    };

    let user = format!(
        "App: {} ({}s)\nWindows: {}\nScreen: {}\n\nIssues:\n{}\n\nJSON: {{\"issue_key\": \"KEY\"}} or {{\"issue_key\": null}}",
        req.app_name, req.duration_s, windows, ocr, issues_text
    );

    (SYSTEM.to_string(), user)
}

pub fn extract_key(text: &str, valid_keys: &HashSet<String>) -> Option<String> {
    let trimmed = text.trim().trim_matches('`');
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(key) = v.get("issue_key").and_then(|k| k.as_str()) {
            if valid_keys.contains(key) {
                return Some(key.to_string());
            }
            return None;
        }
    }
    for key in valid_keys {
        if text.contains(key.as_str()) {
            return Some(key.clone());
        }
    }
    None
}
