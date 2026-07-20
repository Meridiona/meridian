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
                pathext().iter().any(|e| *e == dotted)
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
    let out = Command::new(bin).args(args).output().ok()?;
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

pub fn daemon_service() -> Vec<Check> {
    let bin = [
        PathBuf::from("/usr/local/bin/meridian-daemon"),
        home().join(".local/bin/meridian-daemon"),
    ]
    .into_iter()
    .find(|p| is_exec(p));
    let bin_check = match bin {
        Some(p) => Check::ok("daemon binary", "system", p.display().to_string()),
        None => Check::critical("daemon binary", "system", "not installed")
            .with_remedy("run ./install.sh"),
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
    let built = repo_root()
        .map(|r| r.join("packages/meridian-mcp/dist/index.js").is_file())
        .unwrap_or(false);
    vec![if built {
        Check::ok("mcp built", "system", "dist/index.js present")
    } else {
        Check::warn("mcp built", "system", "not built")
            .with_remedy("cd packages/meridian-mcp && npm run build")
    }]
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
    let env_ok = repo_root()
        .map(|r| r.join(".env").is_file())
        .unwrap_or(false);
    let env_check = if env_ok {
        Check::ok("config (.env)", "system", "present")
    } else {
        Check::warn("config (.env)", "system", "missing").with_remedy("run ./install.sh")
    };
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
