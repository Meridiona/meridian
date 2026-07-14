//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The in-app uninstall wizard's Tauri commands — plan + execute.
//!
//! Deleting `Meridian.app` from Finder does nothing to the launchd agents or
//! staged binaries the tray installed (see `src/uninstall.rs`'s module doc), and
//! macOS never revokes an Accessibility/Screen Recording/Input Monitoring grant
//! just because the app that requested it was deleted. This module is the tray
//! side of the fix: a wizard the user runs from the app **before** deleting it,
//! which stops everything cleanly and tells them what to do about permissions.
//!
//! Both commands shell out to the staged `meridian uninstall` CLI (via
//! [`crate::install::meridian_bin`]) rather than re-implementing its file scan —
//! the daemon binary is the single source of truth for what Meridian has ever
//! written to disk, and it already has the itemized plan/execute split this
//! wizard needs (`--dry-run`/`--json`, `--remove-data`/`--remove-runtime`/
//! `--remove-models`).
//!
//! # Who calls this
//! [`get_uninstall_plan`] / [`execute_uninstall`] are registered in `lib.rs`'s
//! `invoke_handler!`; invoked from `ui/app/uninstall/page.tsx`.
//!
//! # Related
//! - [`crate::mlx_server::stop_server`] — stopped before deleting the runtime
//!   out from under it.
//! - [`crate::tray::open_uninstall_window`] — the menu entry that opens the wizard.
//! - [`crate::commands::system::open_permission_pane`] — the wizard's "revoke
//!   permissions" step reuses this to deep-link System Settings.

use crate::mlx_server::{self, SharedMlxManager};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::State;

/// One removable item — a human label plus the path it names. Mirrors
/// `src/uninstall.rs`'s `Item`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UninstallItem {
    pub label: String,
    pub path: String,
}

/// The wizard's checkbox plan — every category `meridian uninstall --purge
/// --dry-run --json` reports, regardless of which boxes the user will
/// eventually check (the plan is read-only and always shows the maximal
/// scope). Populated even on failure (empty lists + `error` set) so the wizard
/// still renders — the same resolve-empty contract as
/// [`crate::commands::parents::ParentsResponse`].
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UninstallPlan {
    pub agents: Vec<UninstallItem>,
    pub staged_binaries: Vec<UninstallItem>,
    pub data: Vec<UninstallItem>,
    pub runtime: Vec<UninstallItem>,
    pub models: Vec<UninstallItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The result of actually running the uninstall.
#[derive(Debug, Clone, Serialize, Default)]
pub struct UninstallResult {
    pub removed: Vec<String>,
    pub errors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The raw JSON shape `meridian uninstall --json` prints — see
/// `src/uninstall.rs`'s `JsonReport`. Kept private: [`UninstallPlan`] /
/// [`UninstallResult`] are the stable shapes the frontend sees.
#[derive(Debug, Deserialize, Default)]
struct CliReport {
    #[serde(default)]
    agents: Vec<UninstallItem>,
    #[serde(default)]
    staged_binaries: Vec<UninstallItem>,
    #[serde(default)]
    data: Vec<UninstallItem>,
    #[serde(default)]
    runtime: Vec<UninstallItem>,
    #[serde(default)]
    models: Vec<UninstallItem>,
    #[serde(default)]
    removed: Vec<String>,
    #[serde(default)]
    errors: Vec<String>,
}

/// Fetch the full uninstall plan by shelling out to `meridian uninstall
/// --dry-run --json --purge` — read-only, changes nothing. `--purge` is used
/// here regardless of the user's eventual choice so the wizard can show the
/// real, live item count for all three checkboxes at once (data/runtime/models)
/// rather than re-querying per checkbox toggle.
#[tauri::command]
#[tracing::instrument]
pub async fn get_uninstall_plan() -> UninstallPlan {
    let bin = crate::install::meridian_bin();
    match run_cli(&bin, &["uninstall", "--dry-run", "--json", "--purge"]).await {
        Ok(report) => {
            tracing::info!(
                agents = report.agents.len(),
                data = report.data.len(),
                runtime = report.runtime.len(),
                models = report.models.len(),
                "get_uninstall_plan served"
            );
            UninstallPlan {
                agents: report.agents,
                staged_binaries: report.staged_binaries,
                data: report.data,
                runtime: report.runtime,
                models: report.models,
                error: None,
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "get_uninstall_plan failed");
            UninstallPlan {
                error: Some(e),
                ..Default::default()
            }
        }
    }
}

/// Execute the uninstall for the categories the user checked. Always removes
/// the launchd agents + staged binaries — that base teardown is never optional
/// (see `src/uninstall.rs`'s module doc: it's what stops the a11y-helper and
/// daemon from running forever after the app is trashed). `remove_data`/
/// `remove_runtime`/`remove_models` widen the scope to the wizard's three
/// checkboxes.
///
/// Stops the tray's in-process MLX server FIRST: deleting the runtime out from
/// under a still-running server would leave it holding deleted files open.
#[tauri::command]
#[tracing::instrument(skip(mlx))]
pub async fn execute_uninstall(
    remove_data: bool,
    remove_runtime: bool,
    remove_models: bool,
    mlx: State<'_, SharedMlxManager>,
) -> Result<UninstallResult, String> {
    mlx_server::stop_server(&mlx).await;

    let mut args = vec!["uninstall", "--yes", "--json"];
    if remove_data {
        args.push("--remove-data");
    }
    if remove_runtime {
        args.push("--remove-runtime");
    }
    if remove_models {
        args.push("--remove-models");
    }

    let bin = crate::install::meridian_bin();
    match run_cli(&bin, &args).await {
        Ok(report) => {
            tracing::info!(
                removed = report.removed.len(),
                errors = report.errors.len(),
                "execute_uninstall: done"
            );
            Ok(UninstallResult {
                removed: report.removed,
                errors: report.errors,
                error: None,
            })
        }
        Err(e) => {
            tracing::warn!(error = %e, "execute_uninstall failed");
            Err(e)
        }
    }
}

/// Spawn `<bin> <args>` with a 30 s timeout and parse the last non-empty stdout
/// line as the CLI's `--json` report — the CLI may log before it, same
/// convention as `commands::parents::get_ticket_parents`.
async fn run_cli(bin: &str, args: &[&str]) -> Result<CliReport, String> {
    let child = tokio::process::Command::new(bin)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output();

    let output = match tokio::time::timeout(Duration::from_secs(30), child).await {
        Err(_) => return Err("meridian uninstall timed out".to_string()),
        Ok(Err(e)) => return Err(format!("spawn error ({bin}): {e}")),
        Ok(Ok(o)) => o,
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("meridian uninstall exited {:?}", output.status.code())
        } else {
            stderr
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let last = stdout.lines().rfind(|l| !l.trim().is_empty());
    last.and_then(|l| serde_json::from_str::<CliReport>(l).ok())
        .ok_or_else(|| {
            let s = stdout.trim();
            let skip = s.chars().count().saturating_sub(200);
            let tail: String = s.chars().skip(skip).collect();
            format!("could not parse meridian uninstall output: {tail}")
        })
}
