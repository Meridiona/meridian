// meridian — normalises screenpipe activity into structured app sessions
//
// Root-cause diagnosis. Correlates failing checks into the underlying cause so
// the operator sees "the summariser stalled (which is why hours are stuck and
// sessions sentinelled)" instead of three disconnected warnings. Rule-based
// over the check set, plus the escalation footer for the plain `doctor`.

use crate::health::{Report, Severity};

pub struct Diagnosis {
    /// The root cause, one line.
    pub title: String,
    /// Why it produces the symptoms below.
    pub cause: String,
    /// The failing checks this cause explains (group › name).
    pub contributing: Vec<String>,
    /// What to do about it.
    pub action: String,
}

/// Correlate the report's failing checks into root causes (most-specific first).
pub fn root_causes(report: &Report) -> Vec<Diagnosis> {
    let crit = |group: &str, needle: &str| {
        report.checks.iter().any(|c| {
            c.severity == Severity::Critical && c.group == group && c.name.contains(needle)
        })
    };
    let bad = |group: &str, needle: &str| {
        report
            .checks
            .iter()
            .any(|c| c.severity >= Severity::Warn && c.group == group && c.name.contains(needle))
    };
    let ok_check = |group: &str, needle: &str| {
        report
            .checks
            .iter()
            .any(|c| c.severity == Severity::Ok && c.group == group && c.name.contains(needle))
    };
    let contributing = |pairs: &[(&str, &str)]| -> Vec<String> {
        report
            .checks
            .iter()
            .filter(|c| {
                c.severity >= Severity::Warn
                    && pairs
                        .iter()
                        .any(|(g, n)| c.group == *g && c.name.contains(n))
            })
            .map(|c| format!("{} › {}", c.group, c.name))
            .collect()
    };

    let mut out = Vec::new();

    // 0. Daemon up but not progressing — supersedes every downstream symptom.
    //    Either launchd can't keep it loaded / it is crash-looping on startup
    //    (`running` is Critical), or the process is alive yet ETL has gone stale
    //    (the poll loop is hung). The most common crash-loop cause is a migration
    //    that was modified or renumbered after it was applied, which fails sqlx's
    //    checksum — so when the daemon is down the queue/jira/worklog stalls are
    //    not independent faults, they are this one cause.
    let daemon_wedged = crit("meridian daemon", "running")
        || (ok_check("meridian daemon", "running") && bad("meridian daemon", "etl freshness"));
    if daemon_wedged {
        out.push(Diagnosis {
            title: "The meridian daemon isn't making progress".into(),
            cause: "launchd either can't keep it loaded or it is crash-looping on startup, or it is alive but its poll loop is hung. A frequent crash-loop cause is a migration that was modified or renumbered after it was applied, which fails sqlx's checksum (\"migration N was previously applied but has been modified\"). Nothing advances while it is stuck — ETL, classification, Jira sync, and worklogs all stall behind it.".into(),
            contributing: contributing(&[
                ("meridian daemon", "etl"),
                ("meridian daemon", "queue"),
                ("meridian daemon", "classify errors"),
                ("jira", "ticket sync"),
                ("worklog", "hour ledger"),
            ]),
            action: "Inspect ~/.meridian/logs/daemon-error.log for a repeating startup error. A migration checksum mismatch means a migration file changed after it was applied — reconcile the _sqlx_migrations checksums with the current files. Otherwise `meridian start` (or `meridian doctor --fix`).".into(),
        });
    }

    // 1. MLX server down — the whole classify/summarise cascade.
    if crit("mlx-server", "reachable") {
        out.push(Diagnosis {
            title: "MLX classifier server is down".into(),
            cause: "All classification and summarisation run through the MLX server. With it down, sealed sessions pile up, eventually get sentinelled, and worklog hours stall behind them.".into(),
            contributing: contributing(&[
                ("meridian daemon", "queue"),
                ("meridian daemon", "classify errors"),
                ("worklog", "hour ledger"),
            ]),
            action: "Start it (`meridian start`), then `meridian doctor --fix` to drain the backlog.".into(),
        });
    } else if !daemon_wedged && bad("meridian daemon", "summariser queue") {
        out.push(Diagnosis {
            title: "Coding-agent summariser is stalled".into(),
            cause: "Sealed sessions aren't being summarised, so they never reach the classifier and the worklog hour-ledger backs up behind them.".into(),
            contributing: contributing(&[
                ("meridian daemon", "summariser queue"),
                ("meridian daemon", "classify errors"),
                ("worklog", "hour ledger"),
            ]),
            action: "The claude/codex CLI or MLX /summarise is likely failing — inspect with `meridian coding-agent-summarise --dry-run`, or run `meridian doctor --fix`.".into(),
        });
    }

    // 2. Jira: a rejected token vs a merely-stale cache.
    if crit("jira", "auth") {
        out.push(Diagnosis {
            title: "Jira token rejected".into(),
            cause: "The API token is expired or lacks scope, so the ticket cache can't refresh and the candidate set goes stale or empty.".into(),
            contributing: contributing(&[("jira", "ticket sync"), ("jira", "candidate")]),
            action: "Regenerate the Jira API token, update JIRA_API_TOKEN in .env, then `meridian restart`.".into(),
        });
    } else if !daemon_wedged && bad("jira", "ticket sync") {
        out.push(Diagnosis {
            title: "Jira cache is stale (auth OK)".into(),
            cause: "Auth works and the daemon refreshes every 30 min, so this usually means the daemon was down recently and it will self-heal — unless the fetch itself is erroring.".into(),
            contributing: contributing(&[("jira", "ticket sync")]),
            action: "If it persists past 30 min of healthy uptime, force a refresh via `meridian doctor --fix`.".into(),
        });
    }

    // (Daemon-down / crash-loop is handled by rule 0 above, which also folds in
    // the downstream queue/jira/worklog stalls instead of fragmenting them.)

    // 4. Capture degraded — garbage-in for the classifier.
    if crit("screenpipe", "text_present") || crit("screenpipe", "service") {
        out.push(Diagnosis {
            title: "Screen capture is degraded".into(),
            cause: "screenpipe isn't producing usable text, so every session feeds the classifier blank/garbage input — misclassifications here are L1 capture faults, not the model.".into(),
            contributing: contributing(&[("screenpipe", "text_present"), ("screenpipe", "service")]),
            action: "Check Screen-Recording permission for screenpipe and that it is running.".into(),
        });
    }

    // 4b. a11y capture regressed for specific apps.
    if bad("screenpipe", "a11y_regression") {
        out.push(Diagnosis {
            title: "Accessibility capture regressed for some apps".into(),
            cause: "Apps that used to yield structured a11y text dropped to OCR-only — capture broke for them, or the app updated and stopped exposing a tree. Their sessions now feed the classifier lower-fidelity input.".into(),
            contributing: contributing(&[("screenpipe", "a11y_regression")]),
            action: "Restart screenpipe; if it persists, the app changed its a11y support.".into(),
        });
    }

    // 5. Dashboard serving a broken build — up but blank.
    if crit("ui", "ui assets") || crit("ui", "ui serve mode") {
        out.push(Diagnosis {
            title: "Dashboard is serving a broken build".into(),
            cause: "The UI process is up and `/` returns 200, but its _next/static assets 404/500 — usually a stale build or an output:'standalone' build served with `next start`. The page loads blank.".into(),
            contributing: contributing(&[("ui", "ui assets"), ("ui", "ui serve mode")]),
            action: "Rebuild the UI (cd ui && npm run build) and restart; if standalone, serve via `node .next/standalone/server.js`.".into(),
        });
    }

    // 6. Settings split-brain (standalone config issue).
    if bad("config", "settings file") {
        out.push(Diagnosis {
            title: "UI settings aren't reaching the daemon".into(),
            cause: "The dashboard writes ~/.meridian/settings.json but the daemon reads <repo>/settings.json, so toggles made in the UI have no effect.".into(),
            contributing: contributing(&[("config", "settings file")]),
            action: "Align the two files — `meridian doctor --fix` can link them.".into(),
        });
    }

    out
}

/// The "Diagnosis" section for the plain `doctor` report.
pub fn render(dx: &[Diagnosis], color: bool) -> String {
    if dx.is_empty() {
        return String::new();
    }
    let paint = |code: &str, s: &str| {
        if color {
            format!("{code}{s}\x1b[0m")
        } else {
            s.to_string()
        }
    };
    let bold = |s: &str| paint("\x1b[1m", s);
    let dim = |s: &str| paint("\x1b[2m", s);

    let mut out = format!("\n  {}\n", bold("Diagnosis"));
    for d in dx {
        out.push_str(&format!(
            "  {} {}\n",
            paint("\x1b[33m", "●"),
            bold(&d.title)
        ));
        out.push_str(&format!("      {}\n", dim(&d.cause)));
        if !d.contributing.is_empty() {
            out.push_str(&format!(
                "      {} {}\n",
                dim("from:"),
                dim(&d.contributing.join(", "))
            ));
        }
        out.push_str(&format!(
            "      {} {}\n",
            paint("\x1b[36m", "fix:"),
            d.action
        ));
    }
    out
}

/// The escalation footer shown whenever the report has any warning/critical.
pub fn escalation_hint(color: bool) -> String {
    let paint = |code: &str, s: &str| {
        if color {
            format!("{code}{s}\x1b[0m")
        } else {
            s.to_string()
        }
    };
    let bold = |s: &str| paint("\x1b[1m", s);
    let dim = |s: &str| paint("\x1b[2m", s);
    format!(
        "\n  {}\n    • {}  {}\n    • {}\n",
        bold("Still stuck?"),
        "meridian doctor --fix",
        dim("attempt automatic + guided repair"),
        dim("share this output with the team, or run: claude \"debug my meridian doctor output\""),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::Check;

    #[test]
    fn summariser_backlog_chains_to_one_root_cause() {
        let report = Report::new(vec![
            Check::warn("summariser queue", "L2", "293 backed up").in_group("meridian daemon"),
            Check::warn("hour ledger", "L4", "5 stuck").in_group("worklog"),
            Check::ok("reachable", "L2", "ok").in_group("mlx-server"),
        ]);
        let dx = root_causes(&report);
        assert_eq!(dx.len(), 1);
        assert!(dx[0].title.contains("summariser"));
        // both symptoms attributed to the one cause
        assert_eq!(dx[0].contributing.len(), 2);
    }

    #[test]
    fn mlx_down_supersedes_the_queue_symptom() {
        let report = Report::new(vec![
            Check::critical("reachable", "L2", "down").in_group("mlx-server"),
            Check::warn("summariser queue", "L2", "293").in_group("meridian daemon"),
        ]);
        let dx = root_causes(&report);
        assert!(dx[0].title.contains("MLX"));
    }

    #[test]
    fn crash_looping_daemon_is_one_root_cause_not_three() {
        // The real-world bundle: daemon crash-looping on a modified migration,
        // with summariser/classify/jira/worklog all stalled behind it. Must
        // collapse to a single daemon root cause, not three disconnected ones.
        let report = Report::new(vec![
            Check::critical("daemon running", "system", "pid 50758 but last exit 256")
                .in_group("meridian daemon"),
            Check::warn("etl freshness", "L1", "last run 82m ago").in_group("meridian daemon"),
            Check::warn("summariser queue", "L2", "293").in_group("meridian daemon"),
            Check::warn("classify errors", "L2", "10").in_group("meridian daemon"),
            Check::warn("ticket sync", "L2", "86m stale").in_group("jira"),
            Check::warn("hour ledger", "L4", "8 stuck").in_group("worklog"),
            Check::ok("reachable", "L2", "ok").in_group("mlx-server"),
            Check::ok("auth", "L2", "ok").in_group("jira"),
        ]);
        let dx = root_causes(&report);
        let titles: Vec<&str> = dx.iter().map(|d| d.title.as_str()).collect();
        assert_eq!(dx.len(), 1, "should be one root cause, got {titles:?}");
        assert!(dx[0].title.contains("isn't making progress"));
        // the summariser + jira-stale symptom rules must be suppressed
        assert!(!dx.iter().any(|d| d.title.contains("summariser")));
        assert!(!dx.iter().any(|d| d.title.contains("Jira cache")));
    }

    #[test]
    fn alive_but_stale_etl_is_flagged_even_with_pid() {
        // Process is up (pid healthy, exit 0 → "running" Ok) but the poll loop
        // is hung, so ETL has gone stale. Still a daemon root cause.
        let report = Report::new(vec![
            Check::ok("daemon running", "system", "pid 1234").in_group("meridian daemon"),
            Check::warn("etl freshness", "L1", "last run 82m ago").in_group("meridian daemon"),
            Check::warn("summariser queue", "L2", "293").in_group("meridian daemon"),
        ]);
        let dx = root_causes(&report);
        assert_eq!(dx.len(), 1);
        assert!(dx[0].title.contains("isn't making progress"));
    }

    #[test]
    fn healthy_report_has_no_diagnosis() {
        let report = Report::new(vec![Check::ok("x", "L1", "fine").in_group("system")]);
        assert!(root_causes(&report).is_empty());
    }
}
