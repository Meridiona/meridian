// meridian — normalises screenpipe activity into structured app sessions
//
// System-health / fault-attribution layer. Each check is a content-free probe
// (counts, timestamps, booleans — never screen content) that attributes a
// degraded pipeline to the layer that broke (the fault ladder L1..L4), so a
// misclassification is never silently blamed on the model when the real cause
// was broken capture, a dead integration, or bad config.
//
// This module hosts the boot preflight and the `meridian doctor` on-demand
// sweep. L1 (screen capture) checks live in `capture`; further layers are added
// incrementally.

pub mod capture;

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
            Severity::Ok => " ok ",
            Severity::Info => "info",
            Severity::Warn => "warn",
            Severity::Critical => "FAIL",
        }
    }
}

/// One probe result. `layer` is the fault-ladder rung (e.g. "L1") this check
/// attributes a failure to.
#[derive(Debug, Clone)]
pub struct Check {
    pub name: String,
    pub severity: Severity,
    pub detail: String,
    pub layer: &'static str,
    /// The human action that fixes this check, shown as a `→` line in the doctor
    /// report. Set on prerequisites and actionable faults; `None` otherwise.
    pub remedy: Option<String>,
}

impl Check {
    pub fn ok(name: impl Into<String>, layer: &'static str, detail: impl Into<String>) -> Self {
        Check {
            name: name.into(),
            severity: Severity::Ok,
            detail: detail.into(),
            layer,
            remedy: None,
        }
    }

    pub fn warn(name: impl Into<String>, layer: &'static str, detail: impl Into<String>) -> Self {
        Check {
            name: name.into(),
            severity: Severity::Warn,
            detail: detail.into(),
            layer,
            remedy: None,
        }
    }

    pub fn critical(
        name: impl Into<String>,
        layer: &'static str,
        detail: impl Into<String>,
    ) -> Self {
        Check {
            name: name.into(),
            severity: Severity::Critical,
            detail: detail.into(),
            layer,
            remedy: None,
        }
    }

    /// A non-actionable diagnostic line (e.g. a per-app breakdown). Never a fault.
    pub fn info(name: impl Into<String>, layer: &'static str, detail: impl Into<String>) -> Self {
        Check {
            name: name.into(),
            severity: Severity::Info,
            detail: detail.into(),
            layer,
            remedy: None,
        }
    }

    /// Attach the remedy (the fix-it action) shown under a failing check.
    pub fn with_remedy(mut self, remedy: impl Into<String>) -> Self {
        self.remedy = Some(remedy.into());
        self
    }
}

/// A group of checks under one subsystem.
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    /// The worst severity across all checks (Ok if empty).
    pub fn worst(&self) -> Severity {
        self.checks
            .iter()
            .map(|c| c.severity)
            .max()
            .unwrap_or(Severity::Ok)
    }

    /// Human-readable report for `meridian doctor`.
    pub fn render_titled(&self, title: &str) -> String {
        let rule = "─".repeat(58);
        let mut s = format!("\n  Meridian doctor — {title}\n  {rule}\n");
        for c in &self.checks {
            s.push_str(&format!(
                "  [{}] {:<7} {:<26} {}\n",
                c.severity.glyph(),
                c.layer,
                c.name,
                c.detail
            ));
            // Show the fix-it action under any non-passing check that has one.
            if c.severity != Severity::Ok {
                if let Some(remedy) = &c.remedy {
                    s.push_str(&format!("         → {remedy}\n"));
                }
            }
        }
        let summary = match self.worst() {
            Severity::Ok | Severity::Info => "all checks passed",
            Severity::Warn => "completed with warnings",
            Severity::Critical => "CRITICAL issues found",
        };
        s.push_str(&format!("  {rule}\n  {summary}\n"));
        s
    }

    /// Machine-readable output for the `meridian` CLI wrapper to ingest and fold
    /// into its by-daemon table. One TSV line per check:
    /// `status<TAB>name<TAB>detail<TAB>remedy` (status ∈ ok|info|warn|fail).
    pub fn render_porcelain(&self) -> String {
        let mut s = String::new();
        for c in &self.checks {
            let status = match c.severity {
                Severity::Ok => "ok",
                Severity::Info => "info",
                Severity::Warn => "warn",
                Severity::Critical => "fail",
            };
            s.push_str(&format!(
                "{}\t{}\t{}\t{}\n",
                status,
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
                    stage = stage,
                    check = %c.name,
                    layer = c.layer,
                    detail = %c.detail,
                    "health check"
                ),
                Severity::Warn => tracing::warn!(
                    stage = stage,
                    check = %c.name,
                    layer = c.layer,
                    detail = %c.detail,
                    "health check degraded"
                ),
                Severity::Critical => tracing::error!(
                    stage = stage,
                    check = %c.name,
                    layer = c.layer,
                    detail = %c.detail,
                    "health check failed"
                ),
            }
        }
    }
}
