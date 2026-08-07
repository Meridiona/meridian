//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Platform / prerequisite checks: launchd agents, plist validity, process
// liveness, installed binaries, build artifacts, toolchains, and disk. These
// are the "is the environment set up to run at all" rungs, distributed into
// each daemon's table group. Shelling out to launchctl/plutil/pgrep/df/node.

use crate::config::Config;
use crate::health::Check;
use std::path::{Path, PathBuf};
use std::process::Command;

const LABEL_DAEMON: &str = "com.meridiona.daemon";

// ── shared helpers ──────────────────────────────────────────────────────────

fn home() -> PathBuf {
    meridian_core::paths::home_dir_or_cwd()
}

/// Is `p` a file this OS would actually execute?
///
/// Unix answers with the execute bits. Windows has no such bit — what makes a
/// file runnable there is its extension appearing in `PATHEXT` — so the
/// question becomes "is it a file whose extension is executable".
fn is_exec(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        p.is_file()
            && p.extension().and_then(|e| e.to_str()).is_some_and(|ext| {
                let dotted = format!(".{}", ext.to_ascii_uppercase());
                pathext().contains(&dotted)
            })
    }
}

/// The `PATHEXT` entries, upper-cased, as `.EXE`-style strings.
///
/// Windows-only. The documented default is used when the variable is unset,
/// which happens in stripped service environments.
#[cfg(windows)]
fn pathext() -> Vec<String> {
    std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
        .split(';')
        .filter(|e| !e.is_empty())
        .map(|e| e.to_ascii_uppercase())
        .collect()
}

/// Repo root via the running binary (the CLI wrapper may run from anywhere, so
/// cwd is unreliable). Resolves the symlink then walks up to the Cargo.toml.
pub fn repo_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    exe.ancestors()
        .find(|a| a.join("Cargo.toml").is_file())
        .map(|a| a.to_path_buf())
}

/// Find an executable on PATH (no `which` crate).
///
/// On Windows the bare name is not enough: `node` is installed as `node.exe`
/// and npm-installed CLIs (including the coding-agent CLIs this probes for) as
/// `.cmd` shims. Probing only the bare name — which is what this did before —
/// reports every one of them as missing, so the health checks and provider
/// detection would say "not installed" on a machine where they all are.
pub fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .flat_map(|dir| {
            candidate_names(bin)
                .into_iter()
                .map(move |name| dir.join(name))
        })
        .find(|c| c.is_file())
}

/// The filenames to probe for `bin`, in priority order.
///
/// Unix: the name as given. Windows: each `PATHEXT` suffix first (that is what
/// the shell would actually run), then the bare name as a last resort for the
/// unusual extension-less case.
fn candidate_names(bin: &str) -> Vec<String> {
    #[cfg(unix)]
    {
        vec![bin.to_string()]
    }
    #[cfg(windows)]
    {
        // A name that already carries an extension is taken at face value.
        if Path::new(bin).extension().is_some() {
            return vec![bin.to_string()];
        }
        let mut names: Vec<String> = pathext()
            .iter()
            .map(|e| format!("{bin}{}", e.to_ascii_lowercase()))
            .collect();
        names.push(bin.to_string());
        names
    }
}

fn cmd_output(bin: &str, args: &[&str]) -> Option<String> {
    use meridian_core::proc_ext::NoWindow;
    let out = Command::new(bin).args(args).no_window().output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Report the service's definition — present and valid, broken, or not
/// something this platform can answer for.
///
/// `Unknown` reports as Info, not CRITICAL: see
/// [`meridian_core`]-adjacent reasoning on [`crate::platform::ServiceManifest`]
/// — a health check that asserts a problem it has not actually looked for
/// trains the user to ignore it.
fn service_manifest_check(label: &str, name: &'static str) -> Check {
    use crate::platform::ServiceManifest;
    match crate::platform::service_manifest(label) {
        ServiceManifest::Valid => Check::ok(name, "system", "installed + valid"),
        ServiceManifest::Invalid => {
            Check::critical(name, "system", "missing or invalid").with_remedy("run ./install.sh")
        }
        ServiceManifest::Unknown => Check::info(
            name,
            "system",
            "not applicable on this platform (no service integration yet)",
        ),
    }
}

// ── per-daemon service checks ───────────────────────────────────────────────

/// Every location a daemon binary is installed to, across install types.
///
/// `install.sh` symlinks `meridian-daemon` into `/usr/local/bin` or
/// `~/.local/bin` (source installs). The DMG's tray stages the real binary at
/// `~/.meridian/bin/meridian{EXE_SUFFIX}` — see `backend_install::DAEMON_FILE`
/// in the tray crate, and `observability::install_mode` which derives the same
/// path. Probing only the first pair reported a CRITICAL "not installed" on
/// every packaged install, while that very binary was the one answering the
/// check.
///
/// Deliberately probes ALL candidates rather than branching on install mode:
/// "is THIS PROCESS packaged" is a different question from "is a daemon
/// installed on this system". `cargo run -- doctor` on a machine that also has
/// the DMG installed must still find the staged binary — which is the one
/// launchd is actually running.
///
/// The staged path comes from [`crate::observability::staged_daemon_path`], the
/// single source shared with `is_canonical_install()`. Assembling it here
/// independently (as this first did, from a different root — `home()` rather
/// than `paths::meridian_dir()`) is how the two silently disagree, and a
/// disagreement means a false CRITICAL.
fn daemon_binary_candidates() -> Vec<PathBuf> {
    crate::observability::staged_daemon_path()
        .into_iter()
        .chain([
            PathBuf::from("/usr/local/bin/meridian-daemon"),
            home().join(".local/bin/meridian-daemon"),
        ])
        .collect()
}

pub fn daemon_service() -> Vec<Check> {
    let bin = daemon_binary_candidates().into_iter().find(|p| is_exec(p));
    let bin_check = match bin {
        Some(p) => Check::ok("daemon binary", "system", p.display().to_string()),
        None => Check::critical("daemon binary", "system", "not installed")
            .with_remedy("install the app, or run ./install.sh from a source checkout"),
    };
    use crate::platform::ServiceStatus;
    let run_check = match crate::platform::service_status(LABEL_DAEMON) {
        ServiceStatus::Running(pid) => Check::ok("daemon running", "system", format!("pid {pid}")),
        ServiceStatus::NotRunning => {
            Check::critical("daemon running", "system", "not loaded").with_remedy("meridian start")
        }
        // Not "not running" — we have not looked. See ServiceStatus's docs.
        ServiceStatus::Unknown => Check::info(
            "daemon running",
            "system",
            "service state not queryable on this platform yet",
        ),
    };
    vec![
        bin_check,
        service_manifest_check(LABEL_DAEMON, "daemon plist"),
        run_check,
    ]
}

/// Returns a single Info check confirming the dashboard is embedded in the
/// Tauri binary. The legacy standalone-UI launchd agent was retired with the
/// Next-fold (PR #298); probing for its plist always produced a false CRITICAL
/// on healthy post-fold installs.
pub fn ui_service() -> Vec<Check> {
    vec![Check::info(
        "ui service",
        "system",
        "dashboard embedded in the Tauri binary (no separate service)",
    )]
}

pub fn mcp_service() -> Vec<Check> {
    let repo = repo_root();
    let built = repo
        .as_ref()
        .map(|r| r.join("packages/meridian-mcp/dist/index.js").is_file());
    vec![mcp_verdict(built)]
}

/// The four-way `.env` truth table, split out so each case is testable without
/// a real `$HOME` or repo.
///
/// BOTH locations count, because `main.rs` loads both: step 1 walks up from the
/// working directory (a source run finds `<repo>/.env`), and step 1a then
/// unconditionally loads `~/.meridian/.env` — the canonical file the tray writes
/// the DB key and tracker credentials into. Checking only the repo copy reported
/// "missing" on every packaged install, where no repo exists and the canonical
/// file is the one actually in use.
///
/// When both exist, BOTH are named: step 1a's `from_path` does not override, so
/// for any overlapping key the repo file is the one that wins — reporting only
/// the canonical path would send a dev chasing a stale value to the wrong file.
fn env_verdict(repo_env: bool, home_env: bool) -> Check {
    match (repo_env, home_env) {
        (true, true) => Check::ok(
            "config (.env)",
            "system",
            "<repo>/.env and ~/.meridian/.env present (repo wins for overlapping keys)",
        ),
        (false, true) => Check::ok("config (.env)", "system", "~/.meridian/.env present"),
        (true, false) => Check::ok("config (.env)", "system", "<repo>/.env present"),
        (false, false) => Check::warn("config (.env)", "system", "missing")
            .with_remedy("install the app, or run ./install.sh from a source checkout"),
    }
}

/// The decision half of [`mcp_service`], split out so every branch is testable
/// without a real filesystem. `None` = no source checkout found at all.
///
/// The MCP server is built from the repo, so this question only has an answer in
/// a source checkout. `repo_root()` is None on a packaged install and the old
/// code folded that into `false` — reporting "not built" with a remedy
/// (`cd packages/meridian-mcp && …`) naming a directory the user does not have.
/// A check that asserts a problem it has not looked for trains people to ignore
/// the whole report.
///
/// The Info wording reports what was OBSERVED, not an inferred install type:
/// "no Cargo.toml above `current_exe()`" is also true of a `cargo install`ed
/// binary, a release binary copied into `~/bin`, or an `.app`-embedded one.
/// Claiming "packaged install" would be the same conflation this module
/// deliberately avoids for `daemon binary`.
fn mcp_verdict(built: Option<bool>) -> Check {
    match built {
        None => Check::info(
            "mcp built",
            "system",
            "no source checkout found — the MCP server is built from one",
        ),
        Some(true) => Check::ok("mcp built", "system", "dist/index.js present"),
        Some(false) => Check::warn("mcp built", "system", "not built")
            .with_remedy("cd packages/meridian-mcp && npm run build"),
    }
}

// ── system / toolchain ──────────────────────────────────────────────────────

pub fn system_checks(_cfg: &Config) -> Vec<Check> {
    // Windows is a supported target for the daemon itself; the tray-side
    // capture stack is still macOS-only, so Windows reports as a known-partial
    // rather than either "fine" or "unsupported". Revisit when Windows capture
    // lands in the tray.
    let os = if cfg!(target_os = "macos") {
        Check::ok("os", "system", "macOS")
    } else if cfg!(target_os = "windows") {
        Check::warn(
            "os",
            "system",
            "Windows — daemon supported; capture not yet wired",
        )
    } else {
        Check::warn("os", "system", "unsupported OS — capture is not available")
    };
    let repo_env = repo_root().is_some_and(|r| r.join(".env").is_file());
    let home_env = home().join(".meridian/.env").is_file();
    let env_check = env_verdict(repo_env, home_env);
    vec![
        os,
        env_check,
        node_check(),
        // Capture data lives in meridian.db under ~/.meridian (the in-process
        // cutover retired ~/.screenpipe), so the meridian disk check below
        // already covers the capture volume — no separate screenpipe check.
        disk_check("disk (meridian)", &home().join(".meridian")),
    ]
}

fn node_check() -> Check {
    match cmd_output("node", &["--version"]) {
        Some(v) => {
            let major: u32 = v
                .trim_start_matches('v')
                .split('.')
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            if major >= 18 {
                Check::ok("node", "system", v)
            } else {
                Check::warn("node", "system", format!("{v} (< 18; Next.js needs ≥18)"))
                    .with_remedy("upgrade Node to 18+")
            }
        }
        None => Check::warn("node", "system", "not found on PATH")
            .with_remedy("install Node 18+ for the UI"),
    }
}

/// GB free space below which `~/.meridian`'s volume counts as "low" — shared
/// by [`disk_check`] (the `meridian doctor` sweep) and [`meridian_data_low_gb`]
/// (the daemon's background poll check).
const DISK_LOW_THRESHOLD_GB: f64 = 2.0;

fn disk_check(name: &'static str, path: &Path) -> Check {
    match crate::platform::disk_free_gb(path) {
        Some(gb) if gb < DISK_LOW_THRESHOLD_GB => {
            Check::warn(name, "system", format!("{gb:.1} GB free — low"))
                .with_remedy("free disk space")
        }
        Some(gb) => Check::ok(name, "system", format!("{gb:.0} GB free")),
        None => Check::info(name, "system", "usage unknown"),
    }
}

/// Free space in GB on `~/.meridian`'s volume when it has dropped below
/// [`DISK_LOW_THRESHOLD_GB`], `None` otherwise (including "couldn't read it" —
/// fail open, don't alarm on a transient `df` failure). Used by the daemon's
/// background poll tick to raise/clear a `system.disk_low` notice; the
/// on-demand `meridian doctor` sweep uses [`disk_check`] directly instead.
pub fn meridian_data_low_gb() -> Option<f64> {
    crate::platform::disk_free_gb(&home().join(".meridian"))
        .filter(|gb| *gb < DISK_LOW_THRESHOLD_GB)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The binary every supported OS ships, used as a known-present probe.
    /// Named per-platform so these tests are meaningful on the Windows CI job
    /// rather than silently unix-only.
    #[cfg(unix)]
    const PROBE: &str = "sh";
    #[cfg(windows)]
    const PROBE: &str = "cmd";

    #[test]
    fn which_finds_a_binary_that_is_always_present() {
        assert!(
            which(PROBE).is_some(),
            "expected to resolve {PROBE:?} on PATH"
        );
    }

    #[test]
    fn which_returns_none_for_a_binary_that_does_not_exist() {
        assert!(which("meridian-definitely-not-a-real-binary-xyz").is_none());
    }

    /// The `.env` truth table, all four cases.
    ///
    /// The bug: only `<repo>/.env` was checked, so every packaged install — where
    /// no repo exists and `~/.meridian/.env` is the file actually in use —
    /// reported "missing". `main.rs` loads BOTH (step 1 walks up from cwd, step
    /// 1a loads the canonical path unconditionally), so either one satisfies it.
    #[test]
    fn env_verdict_covers_every_combination() {
        use crate::health::Severity;

        // Packaged install: no repo, canonical file present. The regression.
        let c = env_verdict(false, true);
        assert_eq!(c.severity, Severity::Ok, "{c:?}");
        assert!(c.detail.contains("~/.meridian/.env"), "{c:?}");

        // Source checkout with only a repo .env.
        let c = env_verdict(true, false);
        assert_eq!(c.severity, Severity::Ok, "{c:?}");
        assert!(c.detail.contains("<repo>/.env"), "{c:?}");

        // Both present: BOTH must be named, and the precedence stated. Step 1a's
        // `from_path` does not override, so the repo copy wins for overlapping
        // keys — naming only the canonical one sends a dev to the wrong file.
        let c = env_verdict(true, true);
        assert_eq!(c.severity, Severity::Ok, "{c:?}");
        assert!(
            c.detail.contains("<repo>/.env"),
            "both files must be named: {c:?}"
        );
        assert!(
            c.detail.contains("~/.meridian/.env"),
            "both files must be named: {c:?}"
        );
        assert!(
            c.detail.contains("repo wins"),
            "precedence must be stated or a stale key is chased in the wrong file: {c:?}"
        );

        // Genuinely absent — the only case that should warn.
        let c = env_verdict(false, false);
        assert_eq!(c.severity, Severity::Warn, "{c:?}");
        assert!(
            c.remedy.is_some(),
            "a warn without a remedy is not actionable: {c:?}"
        );
    }

    /// `mcp built` must not assert a problem it has not looked for.
    ///
    /// `repo_root() == None` used to fold into `false`, producing a warn whose
    /// remedy (`cd packages/meridian-mcp && …`) names a directory a packaged
    /// user does not have. Info, and worded as what was OBSERVED — "no source
    /// checkout" is also true of a `cargo install`ed or copied binary, so
    /// claiming "packaged install" would be the conflation this module rejects
    /// for `daemon binary`.
    #[test]
    fn mcp_verdict_is_info_without_a_checkout_and_never_claims_an_install_type() {
        use crate::health::Severity;

        let c = mcp_verdict(None);
        assert_eq!(
            c.severity,
            Severity::Info,
            "no checkout must not be a fault: {c:?}"
        );
        assert!(
            c.remedy.is_none(),
            "an unanswerable check must not hand out a remedy: {c:?}"
        );
        assert!(
            !c.detail.contains("packaged"),
            "reports an inferred install type rather than what was observed: {c:?}"
        );

        assert_eq!(mcp_verdict(Some(true)).severity, Severity::Ok);

        let c = mcp_verdict(Some(false));
        assert_eq!(
            c.severity,
            Severity::Warn,
            "a real checkout without a build should warn"
        );
        assert!(c.remedy.is_some(), "{c:?}");
    }

    /// Regression guard: the candidate list probed ONLY the two `install.sh`
    /// symlink paths, so on every packaged install doctor reported a CRITICAL
    /// "daemon binary — not installed" while the staged binary at
    /// `~/.meridian/bin/meridian` was the very process answering the check.
    /// A false CRITICAL on a healthy machine is worse than no check: it trains
    /// users to ignore the report, and the two real faults sit in the same output.
    #[test]
    fn daemon_binary_candidates_cover_both_install_types() {
        let candidates = daemon_binary_candidates();

        // Assert against the SHARED source, not a recomputed copy of the same
        // expression: the first version of this test rebuilt the path from
        // `home()` + `format!("meridian{EXE_SUFFIX}")`, so if production drifted
        // the test drifted with it and stayed green. Whether that shared value
        // still matches the tray's `DAEMON_FILE` is pinned separately, by
        // `observability::install_mode`'s cross-crate source scan.
        let staged =
            crate::observability::staged_daemon_path().expect("home dir resolves under test");
        assert!(
            candidates.contains(&staged),
            "packaged install path {} missing from {candidates:?}",
            staged.display()
        );
        assert_eq!(
            candidates.first(),
            Some(&staged),
            "the staged binary must be probed FIRST — it is the one launchd runs \
             on a machine that also has source-install symlinks lying around"
        );

        // The source-install symlinks `install.sh` creates must survive.
        assert!(
            candidates.contains(&PathBuf::from("/usr/local/bin/meridian-daemon")),
            "source-install path missing from {candidates:?}"
        );
        assert!(
            candidates.contains(&home().join(".local/bin/meridian-daemon")),
            "per-user source-install path missing from {candidates:?}"
        );
    }

    /// Regression guard for the bug where `which` probed only the bare name:
    /// on Windows the shell resolves `cmd` through PATHEXT to `cmd.exe`, and a
    /// bare `cmd` is not a file — so node and every npm-installed CLI shim was
    /// reported missing on machines where they were all installed.
    #[cfg(windows)]
    #[test]
    fn which_resolves_through_pathext() {
        let resolved = which(PROBE).expect("cmd must resolve on Windows");
        assert_eq!(
            resolved
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase),
            Some("exe".to_string()),
            "expected PATHEXT resolution to land on cmd.exe, got {resolved:?}"
        );
    }

    /// `is_exec` must agree with `which`: anything `which` returns is by
    /// definition something this OS would run.
    #[test]
    fn is_exec_agrees_with_which() {
        let resolved = which(PROBE).expect("probe binary must resolve");
        assert!(
            is_exec(&resolved),
            "{resolved:?} came from which() but is_exec() rejected it"
        );
    }

    #[test]
    fn is_exec_rejects_a_directory_and_a_missing_path() {
        assert!(!is_exec(Path::new("/")));
        assert!(!is_exec(Path::new("/meridian-no-such-path-xyz")));
    }
}
