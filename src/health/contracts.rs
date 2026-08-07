//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Cross-process config contracts: values multiple processes must agree on, where
// a silent mismatch masquerades as an AI/runtime fault. Verified against the
// code: the UI reads MERIDIAN_DB_PATH (not MERIDIAN_DB), and the daemon and the
// dashboard must resolve the same settings.json (canonically
// ~/.meridian/settings.json, overridable via MERIDIAN_SETTINGS_PATH).

use crate::config::Config;
use crate::health::Check;
use std::path::PathBuf;

fn home() -> PathBuf {
    meridian_core::paths::home_dir_or_cwd()
}

fn expand(p: &str) -> String {
    p.strip_prefix("~/")
        .map(|rest| home().join(rest).display().to_string())
        .unwrap_or_else(|| p.to_string())
}

pub fn checks(cfg: &Config) -> Vec<Check> {
    vec![db_path_contract(cfg), settings_contract(), dead_poll_env()]
}

/// C1 — the daemon/MCP open MERIDIAN_DB; the UI opens MERIDIAN_DB_PATH. If
/// they diverge the dashboard silently reads a different (stale/empty) database.
fn db_path_contract(cfg: &Config) -> Check {
    let daemon_db = cfg.meridian_db.clone(); // already tilde-expanded by Config
    let ui_db = expand(
        &std::env::var("MERIDIAN_DB_PATH")
            .unwrap_or_else(|_| "~/.meridian/meridian.db".to_string()),
    );
    if daemon_db == ui_db {
        Check::ok("db path", "config", "daemon + UI agree")
    } else {
        Check::warn(
            "db path",
            "config",
            format!("daemon writes {daemon_db}; UI reads {ui_db}"),
        )
        .with_remedy("set MERIDIAN_DB_PATH (UI) equal to MERIDIAN_DB, or unset both")
    }
}

/// C7 — the daemon and the dashboard must read the SAME `settings.json`, or
/// toggles in the UI never take effect.
///
/// The old form of this check asserted the daemon reads `<repo>/settings.json`
/// and warned whenever only `~/.meridian/settings.json` existed. That premise is
/// **obsolete**, not mis-implemented: `settings_json_path()` resolves the
/// canonical `~/.meridian/settings.json` FIRST (a repo/cwd copy is only a
/// fallback when the canonical is absent), so the configuration it flagged as
/// broken is the correct one. On every packaged install — where no repo exists
/// at all — it fired permanently, and the summary escalated it to "UI settings
/// aren't reaching the daemon" on machines where they demonstrably were.
///
/// The one real split-brain left is `MERIDIAN_SETTINGS_PATH` pointing somewhere
/// other than the file the dashboard writes, so that is what this now checks.
fn settings_contract() -> Check {
    let ui_settings = home().join(".meridian/settings.json");
    let resolved = meridian_core::settings::settings_json_path();
    let ui_exists = ui_settings.is_file();

    // Two spellings of the same file (a symlinked home, a doubled separator, a
    // `/./`) are not a split-brain — warning there would tell the user to unset
    // a variable that is causing no harm. Only meaningful when both resolve on
    // disk; `canonicalize` fails on a path that does not exist, in which case
    // fall through to the textual comparison below.
    if let (Ok(a), Ok(b)) = (resolved.canonicalize(), ui_settings.canonicalize()) {
        if a == b {
            return Check::ok("settings file", "config", "daemon + dashboard agree");
        }
    }
    settings_verdict(&resolved, &ui_settings, ui_exists)
}

/// The decision half of [`settings_contract`], split out so every branch is
/// testable without touching `$HOME`, the real filesystem, or the
/// process-global `MERIDIAN_SETTINGS_PATH`.
fn settings_verdict(
    resolved: &std::path::Path,
    ui_settings: &std::path::Path,
    ui_exists: bool,
) -> Check {
    if resolved == ui_settings {
        return Check::ok("settings file", "config", "daemon + dashboard agree");
    }
    // Resolution order falls back to `<cwd>/settings.json` when the canonical
    // file does not exist — and doctor's cwd is not the daemon's cwd, so the
    // path resolved HERE is not necessarily the one the daemon resolved. Only
    // claim disagreement when the dashboard's file actually exists (in which
    // case the canonical rule would have selected it, so something is
    // overriding); otherwise report what was resolved without asserting a fault.
    if ui_exists {
        Check::warn(
            "settings file",
            "config",
            format!(
                "the daemon resolves {} instead of the dashboard's file",
                resolved.display()
            ),
        )
        .with_remedy("unset MERIDIAN_SETTINGS_PATH so both read ~/.meridian/settings.json")
    } else {
        Check::info(
            "settings file",
            "config",
            format!(
                "no settings.json yet — would resolve {}",
                resolved.display()
            ),
        )
    }
}

/// Dead env var: POLL_INTERVAL_SECS is documented but ignored (the daemon takes
/// the interval from settings.json). Flag if someone set it expecting an effect.
fn dead_poll_env() -> Check {
    if std::env::var("POLL_INTERVAL_SECS").is_ok() {
        Check::info(
            "poll interval",
            "config",
            "POLL_INTERVAL_SECS is set but ignored — the daemon uses settings.json",
        )
    } else {
        Check::ok("poll interval", "config", "from settings.json")
    }
}

#[cfg(test)]
mod tests {
    use super::{expand, settings_verdict};
    use std::path::Path;

    /// The canonical, correct configuration: the daemon resolves exactly the
    /// file the dashboard writes.
    ///
    /// This is the case the OLD check flagged as broken. It warned whenever
    /// `~/.meridian/settings.json` existed without a `<repo>/settings.json`
    /// beside it — which is every packaged install, and the summary escalated
    /// it to "UI settings aren't reaching the daemon" on machines where the
    /// settings were demonstrably being applied.
    #[test]
    fn agreeing_paths_are_ok() {
        let p = Path::new("/home/u/.meridian/settings.json");
        let c = settings_verdict(p, p, true);
        assert_eq!(c.severity, crate::health::Severity::Ok, "{c:?}");
    }

    /// A real split-brain: something (MERIDIAN_SETTINGS_PATH) points the daemon
    /// elsewhere while the dashboard's file exists, so UI toggles are ignored.
    #[test]
    fn an_override_that_shadows_the_dashboard_warns() {
        let c = settings_verdict(
            Path::new("/elsewhere/settings.json"),
            Path::new("/home/u/.meridian/settings.json"),
            true,
        );
        assert_eq!(c.severity, crate::health::Severity::Warn, "{c:?}");
    }

    /// Nothing written yet. `settings_json_path` falls back to
    /// `<cwd>/settings.json`, and doctor's cwd is NOT the daemon's cwd, so the
    /// two can legitimately differ here. Report, do not assert a fault.
    #[test]
    fn no_settings_file_yet_is_not_a_fault() {
        let c = settings_verdict(
            Path::new("/tmp/settings.json"),
            Path::new("/home/u/.meridian/settings.json"),
            false,
        );
        assert_eq!(c.severity, crate::health::Severity::Info, "{c:?}");
    }

    #[test]
    fn expand_handles_tilde_and_absolute() {
        // The separator comes from the platform, so assert on the component
        // rather than a literal "/x" — the latter is a Unix-only spelling and
        // fails on Windows even though expansion worked correctly.
        let expanded = expand("~/x");
        assert!(
            std::path::Path::new(&expanded)
                .file_name()
                .is_some_and(|n| n == "x"),
            "expected {expanded:?} to end in the component 'x'"
        );
        assert!(!expanded.starts_with('~'));
        assert_eq!(expand("/abs/path"), "/abs/path");
    }
}
