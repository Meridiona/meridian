//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Tauri command surface, grouped by domain.
//!
//! This module is the root for every `#[tauri::command]` the tray exposes. The
//! commands live in domain submodules; this file re-exports each command at the
//! `crate::commands::*` path so `lib.rs`'s `invoke_handler!` and other callers
//! name them flatly regardless of which submodule they sit in.
//!
//! - [`dashboard`] — the ported `/api/*` DB reads (active/today/week/tasks/…).
//! - [`account`]   — Clerk publishable-key resolution + the captured sign-in email.
//! - [`app_icons`] — real macOS app icon extraction for "Time by app" (`get_app_icon`).
//! - [`daemon`]    — daemon lifecycle (restart/pause/resume) + status probes.
//! - [`system`]    — OS/window actions (open URLs, System Settings panes).
//! - [`health`]    — the `/api/health` check (also reused by [`crate::poll`]).
//! - [`diagnostics`] — the "Export Diagnostics" bundle-and-reveal action
//!   (also the local read path for `meridian logs`'s OTel-spool decode — see
//!   `src/telemetry_spool/render.rs` — since the old JSONL-tailing `logs`
//!   module and its dashboard consumer are both gone).
//! - [`integrations`] — which trackers are connected (`/api/integrations`).
//! - [`notices`]   — clear a fault banner (`/api/notices/[id]` DELETE).
//! - [`notifications`] — the in-app banner dismiss write.
//! - [`parents`]   — valid parent tickets for the hygiene "link a parent" fix.
//! - [`pm_tool_request`] — "I don't see my tool" on `ConnectTrackers`: local
//!   settings mirror + a best-effort PostHog demand signal.
//! - [`settings`]  — runtime settings read + write (`/api/settings` GET/PUT).
//! - [`statuses`]  — ticket status list + set (spawns `meridian ticket-statuses`
//!   / `ticket-set-status`) for the dashboard's status control.
//! - [`tasks`]     — board re-sync action (`/api/tasks/sync`, spawns `meridian`).
//! - [`triage`]    — cleanup working set + the decision/ignore DB writes.
//! - [`setup`]     — first-run detection, permission probes, provider detection.
//! - [`uninstall`] — the in-app uninstall wizard's plan + execute commands.
//! - [`day_summary`] — the AI-composed end-of-day review (generate / read / its data).
//! - [`version`]   — installed vs. published version (`/api/version`).
//! - [`whats_new`] — curated changelog + roadmap for the "What's New" modal.
//! - [`worklogs`]  — worklog review read + edit/approve/reject/unapprove writes.
//!
//! # Related
//! - [`crate::install`] — install-mode + db-path resolution the commands consume.
//! - [`crate::sys`] — shared uid / notify / ui_base helpers.

pub mod account;
pub mod app_icons;
pub mod cli_exec;
pub mod custom_llm;
pub mod daemon;
pub mod daemon_control;
pub mod dashboard;
pub mod day_summary;
pub mod diagnostics;
pub mod health;
pub mod integrations;
pub mod llm_lab;
pub mod notices;
pub mod notifications;
pub mod parents;
pub mod pause;
pub mod plan_tasks;
pub mod pm_tool_request;
pub mod repair;
pub mod settings;
pub mod setup;
pub mod statuses;
pub mod system;
pub mod tasks;
pub mod triage;
pub mod uninstall;
pub mod version;
pub mod walkthrough;
pub mod whats_new;
pub mod worklog_generate;
pub mod worklogs;

// Glob re-exports so callers use `crate::commands::<fn>` regardless of submodule.
// Globs (not explicit names) are required: the `#[tauri::command]` macro emits
// hidden sibling items (`__cmd__*`) that `generate_handler!` resolves through
// this path, and only a glob carries them along with the command fn.
pub use account::*;
pub use app_icons::*;
pub use custom_llm::*;
pub use daemon::*;
pub use dashboard::*;
pub use day_summary::*;
pub use diagnostics::*;
pub use health::*;
pub use integrations::*;
pub use llm_lab::*;
pub use notices::*;
pub use notifications::*;
pub use parents::*;
pub use pause::*;
pub use plan_tasks::*;
pub use pm_tool_request::*;
pub use settings::*;
pub use setup::*;
pub use statuses::*;
pub use system::*;
pub use tasks::*;
pub use triage::*;
pub use uninstall::*;
pub use version::*;
pub use walkthrough::*;
pub use whats_new::*;
pub use worklog_generate::*;
pub use worklogs::*;

/// Log a failing command's error and render it for the frontend, keeping the
/// WHOLE `anyhow` source chain in both.
///
/// Use at every `#[tauri::command]` error boundary that returns `Result<_,
/// String>` over an `anyhow::Error`:
///
/// ```ignore
/// meridian_core::tasks::get_tasks(pool, &today, &week_start, &now_iso)
///     .await
///     .map_err(|e| crate::cmd_err!(e, "get_tasks failed"))
/// ```
///
/// Extra structured fields are passed through ahead of the message, exactly as
/// `tracing::warn!` takes them: `crate::cmd_err!(e, id = body.id, "edit_worklog
/// failed")`.
///
/// The default level is `warn`. Sites where the failure is fatal to the
/// operation take the explicit form, which is otherwise identical:
///
/// ```ignore
/// crate::cmd_err!(level: error, e, "could not write the repair marker")
/// ```
///
/// # Scope — this helps `anyhow` errors ONLY
/// `{:#}` walks a source chain because **`anyhow`'s** `Display` impl checks
/// `f.alternate()`. `std::io::Error`, `reqwest::Error` and `JoinError` do not:
/// for those, `format!("{:#}", e)` is byte-identical to `format!("{}", e)` and
/// this macro adds a log line but recovers nothing. Such a site needs the
/// error promoted first (`anyhow::Error::from(e)`, as
/// `meridian::telemetry_spool::with_causes` does for the OTLP ship leg) —
/// routing it through `cmd_err!` unchanged would look fixed while still
/// dropping `source()`. `cmd_err_is_a_no_op_for_non_anyhow_errors` pins this.
///
/// # Why this exists
/// `anyhow::Error`'s `Display` (`{}`, and therefore `.to_string()` and
/// `tracing`'s `%e`) renders **only the outermost `.context(...)`** — the
/// source chain below it is silently discarded. Every reader in
/// `meridian-core` adds context to each query (`.context("tasks: today
/// presence")`), so the bare pattern this replaces
///
/// ```ignore
/// .map_err(|e| { tracing::warn!(error = %e, "get_tasks failed"); e.to_string() })
/// ```
///
/// threw away the only part anyone needed. In the field (1.83.2, two machines)
/// a corrupt `meridian.db` surfaced in the dashboard as the bare string
/// `tasks: today presence` and shipped to central OpenObserve as the same
/// thing, while the actual fault — `(code: 11) database disk image is
/// malformed` — never left the machine. It was recoverable only by luck:
/// `get_today`'s reader happens not to wrap that query, so its untouched sqlx
/// error leaked the cause on an adjacent log line. `{:#}` walks the full chain
/// (`"outer: inner: root"`), so the diagnosis no longer depends on which
/// sibling command forgot to add context.
///
/// # Egress safety
/// The `error` key is on BOTH `SAFE_STRING_KEYS` and `FREE_TEXT_KEYS`
/// (`src/telemetry_spool/redact.rs`), so a shipped chain is URL/email/token
/// scrubbed and clamped to 2000 chars — far above any chain this produces.
/// Widening `{}` to `{:#}` therefore adds causes, not new leak surface.
#[macro_export]
macro_rules! cmd_err {
    (level: $level:ident, $e:expr, $($rest:tt)+) => {{
        let rendered = format!("{:#}", $e);
        ::tracing::$level!(error = %rendered, $($rest)+);
        rendered
    }};
    ($e:expr, $($rest:tt)+) => {
        $crate::cmd_err!(level: warn, $e, $($rest)+)
    };
}

#[cfg(test)]
mod cmd_err_tests {
    /// The exact `anyhow` chain the field incident produced: a SQLCipher b-tree
    /// fault, wrapped by the reader's `.context("tasks: today presence")`.
    fn field_incident_chain() -> anyhow::Error {
        anyhow::anyhow!("error returned from database: (code: 11) database disk image is malformed")
            .context("tasks: today presence")
    }

    /// Regression guard for the 1.83.2 field incident. The dashboard showed
    /// `tasks: today presence` and nothing else, so a corrupt database was
    /// indistinguishable from a schema/logic fault — both on screen and in the
    /// shipped error report.
    #[test]
    fn cmd_err_keeps_the_root_cause() {
        let rendered = crate::cmd_err!(field_incident_chain(), "get_tasks failed");
        assert!(
            rendered.contains("database disk image is malformed"),
            "root cause was dropped: {rendered}"
        );
        // The outer context is what says WHICH query died — keep it too.
        assert!(
            rendered.contains("tasks: today presence"),
            "outer context was dropped: {rendered}"
        );
    }

    /// Pins the reason the macro exists. If `anyhow` ever made `Display` walk
    /// the chain this would fail, and `cmd_err!` could be retired — until then
    /// this is proof the plain rendering is NOT equivalent.
    #[test]
    fn plain_display_still_drops_the_root_cause() {
        let plain = field_incident_chain().to_string();
        assert_eq!(
            plain, "tasks: today presence",
            "anyhow's Display now walks the chain — re-evaluate cmd_err!"
        );
    }

    /// The string handed to the frontend and the string logged for central
    /// OpenObserve must be the SAME value, or a user quoting their on-screen
    /// error finds nothing in the backend. One `format!`, used twice.
    #[test]
    fn logged_and_returned_values_match() {
        let returned = crate::cmd_err!(field_incident_chain(), "get_tasks failed");
        assert_eq!(returned, format!("{:#}", field_incident_chain()));
    }

    /// Extra structured fields must still pass through — many command sites
    /// carry an `id`/`day` alongside the error.
    #[test]
    fn extra_fields_pass_through() {
        let rendered = crate::cmd_err!(field_incident_chain(), id = 7, "edit_worklog failed");
        assert!(rendered.contains("database disk image is malformed"));
    }

    /// `cmd_err!` on an error whose `Display` ignores `f.alternate()` recovers
    /// nothing — it is a log line and a rename. This is the boundary of what the
    /// macro can fix, and the reason the source guard below matches one shape
    /// rather than every site that renders an error: a scan cannot see types, so
    /// forcing `std::io::Error` / `reqwest::Error` sites through `cmd_err!` would
    /// turn the guard green while `source()` was still being dropped. Those need
    /// `anyhow::Error::from(e)` first (`telemetry_spool::with_causes`).
    #[test]
    fn cmd_err_is_a_no_op_for_non_anyhow_errors() {
        #[derive(Debug)]
        struct Inner;
        impl std::fmt::Display for Inner {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "dns error: failed to lookup address information")
            }
        }
        impl std::error::Error for Inner {}
        #[derive(Debug)]
        struct Outer(Inner);
        impl std::fmt::Display for Outer {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "error sending request for url (https://example)")
            }
        }
        impl std::error::Error for Outer {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }

        let rendered = crate::cmd_err!(Outer(Inner), "ship failed");
        assert!(
            !rendered.contains("dns error"),
            "`{{:#}}` now walks a plain source() chain — the non-anyhow sites \
             (cli_exec, integrations .env writes) can be converted directly and \
             the guard can widen to the class: {rendered}"
        );
    }

    /// The `level:` arm must hold the same invariant as the default one — the
    /// value shipped to central OpenObserve and the value shown on screen are
    /// one `format!`. `repair.rs` drifted apart exactly here before this PR: it
    /// logged `%e` (truncated) and returned `{e:#}` (full), so the on-screen
    /// string carried a cause the backend never saw.
    #[test]
    fn level_arm_logs_and_returns_the_same_value() {
        let returned = crate::cmd_err!(level: error, field_incident_chain(), "repair failed");
        assert_eq!(returned, format!("{:#}", field_incident_chain()));
        assert!(returned.contains("database disk image is malformed"));
    }

    /// Byte offset → 1-based line number, for offender reporting.
    fn line_of(src: &str, offset: usize) -> usize {
        src[..offset].bytes().filter(|b| *b == b'\n').count() + 1
    }

    /// Reads `IDENT` out of `map_err(|IDENT|`, or `None` for a destructuring
    /// pattern (`|(a, b)|`) we cannot reason about.
    fn closure_binding(rest: &str) -> Option<&str> {
        let end = rest.find('|')?;
        let ident = rest[..end].trim();
        (!ident.is_empty()
            && ident
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '\''))
        .then_some(ident)
    }

    /// True if `body` contains `needle` as a whole token (not as the prefix of a
    /// longer identifier) — so a binding named `e` does not match `err`.
    fn has_token(body: &str, needle: &str) -> bool {
        let mut from = 0;
        while let Some(i) = body[from..].find(needle) {
            let at = from + i;
            let after = body[at + needle.len()..].chars().next();
            if !matches!(after, Some(c) if c.is_ascii_alphanumeric() || c == '_') {
                return true;
            }
            from = at + needle.len();
        }
        false
    }

    /// The seam. Finds `map_err` closures that both log `error = %<binding>` and
    /// return `<binding>.to_string()` — `Display` on both halves, so an `anyhow`
    /// source chain is dropped twice over.
    ///
    /// Works on the closure body extracted by delimiter matching, with
    /// whitespace collapsed, so it is indifferent to how `cargo fmt` breaks the
    /// site across lines and to what the binding is named. Returns line numbers.
    fn lossy_map_err_sites(src: &str) -> Vec<usize> {
        const OPEN: &str = "map_err(|";
        let mut hits = Vec::new();
        let mut from = 0;
        while let Some(i) = src[from..].find(OPEN) {
            let at = from + i;
            let after_pipe = at + OPEN.len();
            from = after_pipe;
            let Some(ident) = closure_binding(&src[after_pipe..]) else {
                continue;
            };
            // Body spans from the closing `|` to the `)` closing `map_err(`.
            let body_start = after_pipe + ident.len() + 1;
            let mut depth = 1usize; // the `(` of `map_err(`
            let mut end = body_start;
            for (off, ch) in src[body_start..].char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = body_start + off;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let body: String = src[body_start..end]
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let logs_display = has_token(&body, &format!("error = %{ident}"))
                || has_token(&body, &format!("error=%{ident}"));
            if logs_display && has_token(&body, &format!("{ident}.to_string()")) {
                hits.push(line_of(src, at));
            }
        }
        hits
    }

    /// Every `.rs` under `commands/`, recursively — the 500-line rule pushes
    /// toward `commands/<domain>/mod.rs` splits (cf. `readers/today/`), and a
    /// non-recursive walk would go quietly blind the day that happens.
    fn command_sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir)
            .expect("commands/ dir must exist")
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                command_sources(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }

    /// Source-scanning guard, in the spirit of the dashboard's
    /// `no-native-dialogs.test.ts`: the hand-rolled shape `cmd_err!` replaced is
    /// the obvious thing to type, reads as correct, and fails silently — the
    /// command still returns an error, it is just missing the cause. Nothing at
    /// the call site reveals that, so only a scan keeps the converted sites from
    /// drifting back one paste at a time.
    #[test]
    fn no_command_hand_rolls_the_lossy_error_shape() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("commands");
        let mut files = Vec::new();
        command_sources(&dir, &mut files);
        assert!(files.len() > 20, "scan found suspiciously few sources");

        let mut offenders = Vec::new();
        for path in files {
            let src = std::fs::read_to_string(&path).expect("read command source");
            for line in lossy_map_err_sites(&src) {
                offenders.push(format!(
                    "{}:{line}",
                    path.file_name().unwrap().to_string_lossy()
                ));
            }
        }
        assert!(
            offenders.is_empty(),
            "these sites drop the anyhow source chain - use crate::cmd_err!(e, \"…\") instead: {offenders:?}"
        );
    }

    /// The reason the previous scan was rewritten, and it is self-demonstrating:
    /// `cargo fmt` split this PR's own new timeout arm in `tasks.rs` across five
    /// lines. The old line-pair match required `error = %e` to sit on the same
    /// physical line as `tracing::warn!(`, so any site with enough fields to
    /// wrap became invisible without ever failing.
    #[test]
    fn the_scan_survives_cargo_fmt_line_splits() {
        let split = r#"
            thing().map_err(|e| {
                tracing::warn!(
                    bin = %bin,
                    cwd = %cwd.display(),
                    error = %e,
                    "get_tasks failed"
                );
                e.to_string()
            })?;
        "#;
        assert_eq!(
            lossy_map_err_sites(split).len(),
            1,
            "a reformatted offender must still be caught"
        );
    }

    /// The binding is not always named `e`; the shape is the defect, not the
    /// spelling.
    #[test]
    fn the_scan_catches_a_renamed_binding() {
        let renamed = r#"
            thing().map_err(|err| {
                tracing::warn!(error = %err, "get_tasks failed");
                err.to_string()
            })?;
        "#;
        assert_eq!(lossy_map_err_sites(renamed).len(), 1);
    }

    /// No false positives on the shape this PR migrated to, on a bare
    /// `.map_err(|e| e.to_string())` (no log — a different, deferred defect), or
    /// on a binding that merely shares a prefix with another.
    #[test]
    fn the_scan_does_not_flag_correct_or_unrelated_sites() {
        let ok = r#"
            a().map_err(|e| crate::cmd_err!(e, "get_tasks failed"))?;
            b().map_err(|e| crate::cmd_err!(level: error, e, "boom"))?;
            c().map_err(|e| e.to_string())?;
            d().map_err(|error_ctx| {
                tracing::warn!(error = %error_ctx, "kept");
                format!("{error_ctx:#}")
            })?;
        "#;
        assert!(
            lossy_map_err_sites(ok).is_empty(),
            "{:?}",
            lossy_map_err_sites(ok)
        );
    }
}
