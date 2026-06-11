//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Shared status normalisation for every PM provider. Trackers expose arbitrary,
// user-defined column / state names ("In Review", "QA", "Ready for Deploy", …),
// so we store each name verbatim (`status_raw`) for display and derive the single
// signal downstream logic needs — `is_terminal`, i.e. "is this ticket done?".
//
// Resolution precedence (first hit wins):
//   1. Env override   — `<PROVIDER>_TERMINAL_STATUSES` / `<PROVIDER>_OPEN_STATUSES`
//   2. Native category — the tracker's own done/closed flag, when it exposes one
//      (Jira statusCategory, Linear state type, Azure StateCategory). Pass `None`
//      when the tracker has no such field (GitHub Projects, Trello) OR when its
//      field is ambiguous (Jira's "undefined"/No-Category status), so the keyword
//      heuristic and any override still get a say instead of blind-bucketing.
//   3. Keyword heuristic on the raw name — the last-resort fallback.
//
// Env vars are comma-separated, case-insensitive lists of raw status names, e.g.
//   GITHUB_TERMINAL_STATUSES="Shipped,Deployed,Archived"
//   JIRA_OPEN_STATUSES="Ready for Release"
// `<PROVIDER>` is the provider id upper-cased with '-' → '_' (e.g. azure_devops →
// AZURE_DEVOPS). An override always wins, so a team can correct a tracker that
// mislabels (or fails to label) a status without a code change.

use std::env;

/// A provider status after normalisation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedStatus {
    /// Verbatim provider status / column name, for display. May be empty.
    pub raw: String,
    /// Whether this status means the ticket is done / closed.
    pub is_terminal: bool,
}

/// Resolve a provider status into `(raw, is_terminal)`.
///
/// `native_terminal` is `Some(_)` only when the tracker exposes a trustworthy
/// done/closed category for this status. Pass `None` for trackers with no such
/// field, or when the field is present but ambiguous, so the heuristic decides.
pub fn resolve(provider: &str, raw: &str, native_terminal: Option<bool>) -> ResolvedStatus {
    let raw = raw.trim();
    let is_terminal = env_override(provider, raw)
        .or(native_terminal)
        .unwrap_or_else(|| heuristic_terminal(raw));
    ResolvedStatus {
        raw: raw.to_string(),
        is_terminal,
    }
}

/// User-configured override for this raw status: `Some(true)` if listed terminal,
/// `Some(false)` if listed open, `None` if unlisted.
fn env_override(provider: &str, raw: &str) -> Option<bool> {
    if raw.is_empty() {
        return None;
    }
    let key = provider.to_ascii_uppercase().replace('-', "_");
    if env_list_contains(&format!("{key}_TERMINAL_STATUSES"), raw) {
        return Some(true);
    }
    if env_list_contains(&format!("{key}_OPEN_STATUSES"), raw) {
        return Some(false);
    }
    None
}

fn env_list_contains(var: &str, needle: &str) -> bool {
    env::var(var)
        .ok()
        .map(|v| {
            v.split(',')
                .any(|item| item.trim().eq_ignore_ascii_case(needle))
        })
        .unwrap_or(false)
}

/// Keyword fallback for trackers with no trustworthy done/closed category. Covers
/// the vocabulary real boards use for terminal columns. An empty name is open.
fn heuristic_terminal(raw: &str) -> bool {
    const TERMINAL_KEYWORDS: &[&str] = &[
        "done", "complete", "closed", "resolved", "shipped", "merged", "deployed", "released",
        "archived", // "cancel" covers cancel / cancelled / canceled
        "cancel",
    ];
    let lower = raw.to_lowercase();
    TERMINAL_KEYWORDS.iter().any(|kw| lower.contains(kw))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Env-dependent assertions are gated behind a serial guard so concurrent
    // tests can't observe each other's vars. Each sets a unique provider prefix.
    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn native_terminal_passes_through() {
        let r = resolve("jira", "Done", Some(true));
        assert_eq!(r.raw, "Done");
        assert!(r.is_terminal);
        assert!(!resolve("jira", "In Progress", Some(false)).is_terminal);
    }

    #[test]
    fn raw_name_is_preserved_verbatim() {
        // The display name survives even for exotic columns — never collapsed.
        assert_eq!(
            resolve("github", "Ready for Deploy", None).raw,
            "Ready for Deploy"
        );
        assert_eq!(resolve("github", "  Backlog  ", None).raw, "Backlog");
    }

    #[test]
    fn heuristic_classifies_terminal_columns() {
        for name in [
            "Done",
            "Shipped",
            "Deployed",
            "Closed",
            "Archived",
            "Cancelled",
        ] {
            assert!(
                resolve("github", name, None).is_terminal,
                "{name} should be terminal"
            );
        }
    }

    #[test]
    fn heuristic_leaves_open_columns_open() {
        for name in ["Backlog", "In Review", "QA", "Blocked", "Doing", ""] {
            assert!(
                !resolve("github", name, None).is_terminal,
                "{name} should be open"
            );
        }
    }

    #[test]
    fn env_terminal_override_wins_over_native() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("JIRA_TERMINAL_STATUSES", "Ready for Release, Verified");
        // Native says open, but the team marked it terminal.
        assert!(resolve("jira", "Ready for Release", Some(false)).is_terminal);
        assert!(resolve("jira", "verified", Some(false)).is_terminal); // case-insensitive
        std::env::remove_var("JIRA_TERMINAL_STATUSES");
    }

    #[test]
    fn env_open_override_wins_over_heuristic() {
        let _g = ENV_LOCK.lock().unwrap();
        // "Done-ish" name the team does NOT consider closed.
        std::env::set_var("GITHUB_OPEN_STATUSES", "Done Pending Review");
        assert!(!resolve("github", "Done Pending Review", None).is_terminal);
        std::env::remove_var("GITHUB_OPEN_STATUSES");
    }

    #[test]
    fn provider_id_uppercased_for_env_lookup() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("AZURE_DEVOPS_TERMINAL_STATUSES", "Deployed to Prod");
        assert!(resolve("azure_devops", "Deployed to Prod", Some(false)).is_terminal);
        std::env::remove_var("AZURE_DEVOPS_TERMINAL_STATUSES");
    }
}
