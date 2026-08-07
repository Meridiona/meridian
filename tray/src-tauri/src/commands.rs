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
pub mod repair;
pub mod settings;
pub mod setup;
pub mod statuses;
pub mod system;
pub mod tasks;
pub mod triage;
pub mod uninstall;
pub mod version;
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
pub use settings::*;
pub use setup::*;
pub use statuses::*;
pub use system::*;
pub use tasks::*;
pub use triage::*;
pub use uninstall::*;
pub use version::*;
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
    ($e:expr, $($rest:tt)+) => {{
        let rendered = format!("{:#}", $e);
        ::tracing::warn!(error = %rendered, $($rest)+);
        rendered
    }};
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

    /// Source-scanning guard, in the spirit of the dashboard's
    /// `no-native-dialogs.test.ts`: the hand-rolled shape `cmd_err!` replaced is
    /// the obvious thing to type, reads as correct, and fails silently — the
    /// command still returns an error, it is just missing the cause. Nothing at
    /// the call site reveals that, so only a scan keeps the 43 converted sites
    /// from drifting back one paste at a time.
    #[test]
    fn no_command_hand_rolls_the_lossy_error_shape() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("commands");
        let mut offenders = Vec::new();
        let entries = std::fs::read_dir(&dir).expect("commands/ dir must exist");
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("read command source");
            // The exact shape: a `map_err` closure that logs `error = %e` and
            // then returns `e.to_string()` — Display on both halves, so the
            // `anyhow` source chain is dropped twice over.
            for (i, window) in src.lines().collect::<Vec<_>>().windows(3).enumerate() {
                if window[0].contains("tracing::warn!(error = %e")
                    && window[1].trim() == "e.to_string()"
                {
                    offenders.push(format!(
                        "{}:{}",
                        path.file_name().unwrap().to_string_lossy(),
                        i + 1
                    ));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "these sites drop the anyhow source chain - use crate::cmd_err!(e, \"…\") instead: {offenders:?}"
        );
    }
}
