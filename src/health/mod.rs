// meridian — normalises screenpipe activity into structured app sessions
//
// System-health / fault-attribution layer. Each check is a content-free probe
// (counts, timestamps, booleans, reachability — never screen content) that
// attributes a degraded pipeline to the layer that broke (the fault ladder
// L1..L4), so a problem is never silently blamed on the model when the real
// cause was broken capture, a dead integration, bad config, or a missing
// dependency.
//
// This module hosts the boot preflight and the `meridian doctor` on-demand
// sweep. Checks are grouped by the daemon/subsystem they belong to and rendered
// as a table; `run_all` is the comprehensive sweep the CLI wrapper delegates to.

pub mod capture;
pub mod codingagent;
pub mod contracts;
pub mod daemon;
pub mod jira;
pub mod mlx;
pub mod observability;
pub mod platform;
pub mod worklog;

use crate::config::Config;
use crate::db::screenpipe::open_screenpipe;

/// Severity of a single check. Ordered Ok < Info < Warn < Critical. `Info` is a
/// non-actionable diagnostic line (e.g. a per-app breakdown) — it never trips a
/// warning or a non-zero exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Ok,
    Info,
    Warn,
    Critical,
}

impl Severity {
    /// Fixed-width glyph for the doctor report.
    fn glyph(self) -> &'static str {
        match self {
            Severity::Ok => "✓",
            Severity::Info => "·",
            Severity::Warn => "⊘",
            Severity::Critical => "✗",
        }
    }

    /// ANSI colour code (no reset) for the glyph when stdout is a terminal.
    fn color(self) -> &'static str {
        match self {
            Severity::Ok => "\x1b[32m",       // green
            Severity::Info => "\x1b[2m",      // dim
            Severity::Warn => "\x1b[33m",     // yellow
            Severity::Critical => "\x1b[31m", // red
        }
    }

    /// Porcelain status token consumed by the CLI wrapper.
    fn token(self) -> &'static str {
        match self {
            Severity::Ok => "ok",
            Severity::Info => "info",
            Severity::Warn => "warn",
            Severity::Critical => "fail",
        }
    }
}

/// One probe result. `group` is the daemon/subsystem this check belongs to (the
/// table section); `layer` is the fault-ladder rung (e.g. "L1") a failure is
/// attributed to.
#[derive(Debug, Clone)]
pub struct Check {
    pub group: &'static str,
    pub layer: &'static str,
    pub name: String,
    pub severity: Severity,
    pub detail: String,
    /// The human action that fixes this check, shown as a `→` line under a
    /// failing check. Set on prerequisites and actionable faults; `None` else.
    pub remedy: Option<String>,
}

impl Check {
    fn make(
        severity: Severity,
        name: impl Into<String>,
        layer: &'static str,
        detail: impl Into<String>,
    ) -> Self {
        Check {
            group: "",
            layer,
            name: name.into(),
            severity,
            detail: detail.into(),
            remedy: None,
        }
    }

    pub fn ok(name: impl Into<String>, layer: &'static str, detail: impl Into<String>) -> Self {
        Check::make(Severity::Ok, name, layer, detail)
    }
    pub fn info(name: impl Into<String>, layer: &'static str, detail: impl Into<String>) -> Self {
        Check::make(Severity::Info, name, layer, detail)
    }
    pub fn warn(name: impl Into<String>, layer: &'static str, detail: impl Into<String>) -> Self {
        Check::make(Severity::Warn, name, layer, detail)
    }
    pub fn critical(
        name: impl Into<String>,
        layer: &'static str,
        detail: impl Into<String>,
    ) -> Self {
        Check::make(Severity::Critical, name, layer, detail)
    }

    /// Attach the remedy (the fix-it action) shown under a failing check.
    pub fn with_remedy(mut self, remedy: impl Into<String>) -> Self {
        self.remedy = Some(remedy.into());
        self
    }

    /// Assign this check to a daemon/subsystem group (the table section).
    pub fn in_group(mut self, group: &'static str) -> Self {
        self.group = group;
        self
    }
}

/// Tag every check in a module's output with its daemon group in one call.
pub fn tag(group: &'static str, checks: Vec<Check>) -> Vec<Check> {
    checks.into_iter().map(|c| c.in_group(group)).collect()
}

/// A collection of checks, rendered grouped by daemon.
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    pub fn new(checks: Vec<Check>) -> Self {
        Report { checks }
    }

    /// The worst severity across all checks (Ok if empty).
    pub fn worst(&self) -> Severity {
        self.checks
            .iter()
            .map(|c| c.severity)
            .max()
            .unwrap_or(Severity::Ok)
    }

    /// (ok, info, warn, critical) counts.
    pub fn counts(&self) -> (usize, usize, usize, usize) {
        let mut c = (0, 0, 0, 0);
        for chk in &self.checks {
            match chk.severity {
                Severity::Ok => c.0 += 1,
                Severity::Info => c.1 += 1,
                Severity::Warn => c.2 += 1,
                Severity::Critical => c.3 += 1,
            }
        }
        c
    }

    /// Distinct groups in first-appearance order.
    fn groups(&self) -> Vec<&'static str> {
        let mut seen = Vec::new();
        for c in &self.checks {
            if !seen.contains(&c.group) {
                seen.push(c.group);
            }
        }
        seen
    }

    /// Rich, optionally-coloured table grouped by daemon. `color` should reflect
    /// whether stdout is a terminal.
    pub fn render(&self, color: bool) -> String {
        let paint = |code: &str, s: &str| -> String {
            if color {
                format!("{code}{s}\x1b[0m")
            } else {
                s.to_string()
            }
        };
        let dim = |s: &str| paint("\x1b[2m", s);
        let bold = |s: &str| paint("\x1b[1m", s);

        let mut out = String::new();
        out.push_str(&format!(
            "\n  {}\n",
            bold("Meridian doctor — health by daemon")
        ));
        out.push_str(&format!("  {}\n", dim(&"═".repeat(56))));

        // Surface the problem count up front so issues aren't missed in a long
        // mostly-green table.
        let (_, _, warn_n, crit_n) = self.counts();
        if warn_n + crit_n > 0 {
            out.push_str(&format!(
                "  {}\n",
                paint(
                    "\x1b[33m",
                    &format!("⚠ {} item(s) need attention", warn_n + crit_n)
                )
            ));
        }

        for group in self.groups() {
            let label = if group.is_empty() { "general" } else { group };
            out.push_str(&format!(
                "\n  {}\n",
                paint("\x1b[36m", &format!("▸ {label}"))
            ));
            for c in self.checks.iter().filter(|c| c.group == group) {
                let glyph = paint(c.severity.color(), c.severity.glyph());
                // Drop a redundant "<group>." prefix from the check name.
                let name = c.name.strip_prefix(&format!("{group}.")).unwrap_or(&c.name);
                out.push_str(&format!("    {glyph}  {name:<26} {}\n", dim(&c.detail)));
                if c.severity >= Severity::Warn {
                    if let Some(r) = &c.remedy {
                        out.push_str(&format!("       {} {}\n", dim("→"), dim(r)));
                    }
                }
            }
        }

        let (ok, info, warn, crit) = self.counts();
        out.push_str(&format!("\n  {}\n", dim(&"═".repeat(56))));
        let summary = format!(
            "{}  {}  {}  {}",
            paint("\x1b[32m", &format!("✓ {ok} ok")),
            dim(&format!("· {info} info")),
            paint("\x1b[33m", &format!("⊘ {warn} warn")),
            paint("\x1b[31m", &format!("✗ {crit} fail")),
        );
        out.push_str(&format!("  {summary}\n"));
        let verdict = match self.worst() {
            Severity::Critical => paint("\x1b[31m", "  ✗ critical issues — see remedies above"),
            Severity::Warn => paint("\x1b[33m", "  ⊘ healthy with warnings"),
            _ => paint("\x1b[32m", "  ✓ all systems healthy"),
        };
        out.push_str(&format!("{verdict}\n"));
        out
    }

    /// Machine-readable output for the `meridian` CLI wrapper to ingest and fold
    /// into its by-daemon table. One TSV line per check:
    /// `status<TAB>group<TAB>name<TAB>detail<TAB>remedy`.
    pub fn render_porcelain(&self) -> String {
        let mut s = String::new();
        for c in &self.checks {
            s.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\n",
                c.severity.token(),
                c.group,
                c.name,
                c.detail,
                c.remedy.as_deref().unwrap_or("")
            ));
        }
        s
    }

    /// Emit each check to tracing at a level matching its severity. Used by the
    /// boot preflight (non-fatal — the daemon still runs; we surface the fault).
    pub fn log(&self, stage: &str) {
        for c in &self.checks {
            match c.severity {
                Severity::Ok | Severity::Info => tracing::debug!(
                    stage = stage, check = %c.name, group = c.group, layer = c.layer,
                    detail = %c.detail, "health check"
                ),
                Severity::Warn => tracing::warn!(
                    stage = stage, check = %c.name, group = c.group, layer = c.layer,
                    detail = %c.detail, "health check degraded"
                ),
                Severity::Critical => tracing::error!(
                    stage = stage, check = %c.name, group = c.group, layer = c.layer,
                    detail = %c.detail, "health check failed"
                ),
            }
        }
    }
}

/// The comprehensive sweep behind `meridian doctor`. Opens what each subsystem
/// needs (read-only), runs every check module, and returns one grouped report.
/// Best-effort: a module that cannot open its dependency reports that as a
/// failing check rather than aborting the sweep.
pub async fn run_all(cfg: &Config) -> Report {
    let mut checks: Vec<Check> = Vec::new();

    // system — OS, config, disk, toolchains.
    checks.extend(tag("system", platform::system_checks(cfg)));

    // meridian daemon — service (binary/plist/process) + ETL liveness/queues.
    let md = daemon::open_meridian_ro(cfg).await;
    {
        let mut g = platform::daemon_service();
        g.extend(daemon::checks(cfg, md.as_ref()).await);
        checks.extend(tag("meridian daemon", g));
    }

    // screenpipe — service + L1 capture content (screenpipe DB, read-only).
    {
        let sp = open_screenpipe(&cfg.screenpipe_db_uri()).await.ok();
        let mut g = platform::screenpipe_service();
        g.extend(capture::checks(cfg, sp.as_ref()).await);
        checks.extend(tag("screenpipe", g));
        if let Some(p) = sp {
            p.close().await;
        }
    }

    // mlx-server — service + HTTP readiness probes.
    {
        let mut g = platform::mlx_service(cfg);
        g.extend(mlx::checks(cfg).await);
        checks.extend(tag("mlx-server", g));
    }

    // jira — auth, sync freshness, candidate completeness.
    checks.extend(tag("jira", jira::checks(cfg, md.as_ref()).await));

    // worklog — drafts awaiting review, stuck hours, Jira post failures.
    checks.extend(tag("worklog", worklog::checks(cfg, md.as_ref()).await));
    if let Some(p) = md {
        p.close().await;
    }

    // coding-agent — Claude/Codex CLI + ingest dirs (and the Cursor gap).
    checks.extend(tag("coding-agent", codingagent::checks(cfg)));

    // ui + mcp — build/service state.
    checks.extend(tag("ui", platform::ui_service()));
    checks.extend(tag("mcp", platform::mcp_service()));

    // observability — OpenObserve sink (the health layer's own eyes).
    checks.extend(tag("observability", observability::checks(cfg).await));

    // config — cross-process contracts (DB path, settings file, dead env).
    checks.extend(tag("config", contracts::checks(cfg)));

    Report::new(checks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Report {
        Report::new(vec![
            Check::ok("screenpipe.frames", "L1", "100").in_group("screenpipe"),
            Check::warn("queue", "L2", "deep")
                .with_remedy("fix it")
                .in_group("meridian daemon"),
            Check::info("a11y", "L1", "x").in_group("screenpipe"),
            Check::critical("auth", "L2", "401").in_group("jira"),
        ])
    }

    #[test]
    fn counts_and_worst() {
        let r = sample();
        assert_eq!(r.counts(), (1, 1, 1, 1));
        assert_eq!(r.worst(), Severity::Critical);
    }

    #[test]
    fn porcelain_is_five_tab_columns_with_group() {
        let out = sample().render_porcelain();
        let cols: Vec<&str> = out.lines().next().unwrap().split('\t').collect();
        assert_eq!(cols.len(), 5);
        assert_eq!(cols[0], "ok"); // status
        assert_eq!(cols[1], "screenpipe"); // group
        assert_eq!(cols[2], "screenpipe.frames"); // name (full, not stripped)
    }

    #[test]
    fn render_groups_strips_prefix_and_shows_remedy() {
        let out = sample().render(false);
        assert!(out.contains("▸ screenpipe"));
        assert!(out.contains("▸ meridian daemon"));
        // group prefix stripped in the table
        assert!(out.contains(" frames "));
        assert!(!out.contains("screenpipe.frames"));
        // remedy under the warn
        assert!(out.contains("→ fix it"));
        // counts summary present
        assert!(out.contains("1 ok"));
    }

    #[test]
    fn color_flag_gates_ansi() {
        assert!(!sample().render(false).contains('\x1b'));
        assert!(sample().render(true).contains('\x1b'));
    }
}
