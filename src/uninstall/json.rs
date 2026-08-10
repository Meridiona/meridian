//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The `--json` reporting path for `meridian uninstall`, split out of
//! [`super`] to keep that file under the 500-line guideline.
//!
//! Emits a single machine-readable JSON line — a full plan (dry-run) or a full
//! result (executed) — instead of the human plan/result text. The tray's
//! uninstall-wizard commands parse this to drive its checkboxes and show a real
//! result, rather than re-implementing the scan.
//!
//! # Who calls this
//! [`super::run`], when `--json` is present.
//!
//! # Related
//! - [`super`] — the human-text path + the shared [`super::Plan`] this consumes.
//! - `tray/src-tauri/src/commands/uninstall.rs` — the only consumer of this
//!   contract; keep [`JsonReport`]'s shape in sync with it.

use super::{remove_path, reset_tcc_grants, uid_str, Item, Plan};
use serde::Serialize;

/// The `--json` output shape — a full plan (dry-run) or a full result
/// (executed), depending on `executed`. The tray's uninstall commands are the
/// only consumers; keep this contract in sync with
/// `tray/src-tauri/src/commands/uninstall.rs`.
#[derive(Debug, Serialize, Default)]
struct JsonReport {
    dry_run: bool,
    executed: bool,
    remove_data: bool,
    remove_runtime: bool,
    remove_models: bool,
    agents: Vec<Item>,
    staged_binaries: Vec<Item>,
    data: Vec<Item>,
    runtime: Vec<Item>,
    models: Vec<Item>,
    /// Paths successfully removed (only populated when `executed`).
    removed: Vec<String>,
    /// Non-fatal removal failures, `"<path>: <error>"` (only when `executed`).
    errors: Vec<String>,
}

/// The `--json` path: same computation as the human branch, emitted as one
/// JSON line, no prompts (a missing `--yes` under `--json` is treated as a
/// refusal to execute non-interactively — callers must pass both).
pub(super) fn run_json(plan: &Plan) {
    let flags = plan.flags;
    let mut report = JsonReport {
        dry_run: flags.dry_run,
        executed: false,
        remove_data: flags.remove_data,
        remove_runtime: flags.remove_runtime,
        remove_models: flags.remove_models,
        agents: plan
            .agents
            .iter()
            .map(|(label, path)| Item::new(label.clone(), path))
            .collect(),
        staged_binaries: plan
            .staged_binaries
            .iter()
            .map(|p| Item::from_path(p))
            .collect(),
        data: plan.data.iter().map(|p| Item::from_path(p)).collect(),
        runtime: plan.runtime.iter().map(|p| Item::from_path(p)).collect(),
        models: plan.models.iter().map(|p| Item::from_path(p)).collect(),
        removed: Vec::new(),
        errors: Vec::new(),
    };

    if flags.dry_run || !flags.yes {
        println!("{}", serde_json::to_string(&report).unwrap_or_default());
        return;
    }

    report.executed = true;
    let uid = uid_str();
    for (label, plist) in &plan.agents {
        // `bootout` can legitimately return non-zero (the agent isn't currently
        // loaded); it's best-effort, so its exit status isn't an error. The
        // meaningful outcome is whether the plist itself was removed, tracked
        // via `report.removed`/`report.errors` below.
        let _ = std::process::Command::new("launchctl")
            .args(["bootout", &format!("gui/{uid}/{label}")])
            .status();
        match std::fs::remove_file(plist) {
            Ok(()) => report.removed.push(plist.display().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => report.errors.push(format!("{}: {e}", plist.display())),
        }
    }
    for f in plan
        .staged_binaries
        .iter()
        .chain(&plan.data)
        .chain(&plan.runtime)
        .chain(&plan.models)
    {
        match remove_path(f) {
            Ok(()) => report.removed.push(f.display().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => report.errors.push(format!("{}: {e}", f.display())),
        }
    }
    if flags.remove_data {
        // Surface any TCC reset that didn't clearly succeed so automation sees
        // the real outcome instead of assuming a clean reset.
        for f in reset_tcc_grants() {
            report.errors.push(format!("tccutil reset {f}"));
        }
        // The database key in the OS keychain lives on no filesystem path, so
        // it cannot ride the `plan` loops above and has to be removed here too
        // — this is the path the tray's uninstall WIZARD drives, which is how
        // most users uninstall, so omitting it would leave the key behind on
        // exactly the common route.
        match super::remove_db_key_from_keychain() {
            None => report.removed.push("OS keychain: database key".to_string()),
            Some(reason) => report.errors.push(reason),
        }
    }
    // `--purge` only: nuke anything left under ~/.meridian the itemized lists
    // above didn't name. Mirrors the human path's `--purge`-only gate — see the
    // note there; `--remove-data --remove-runtime` must NOT trigger this.
    if flags.purge && plan.meridian_dir.is_dir() {
        match std::fs::remove_dir_all(&plan.meridian_dir) {
            Ok(()) => report.removed.push(plan.meridian_dir.display().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => report
                .errors
                .push(format!("{}: {e}", plan.meridian_dir.display())),
        }
    }
    println!("{}", serde_json::to_string(&report).unwrap_or_default());
}
