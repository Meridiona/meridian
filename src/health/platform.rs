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
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn is_exec(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
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
pub fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(bin))
        .find(|c| c.is_file())
}

fn launchd_pid(label: &str) -> Option<i64> {
    let out = Command::new("launchctl")
        .args(["list", label])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(rest) = line.trim().strip_prefix("\"PID\" = ") {
            return rest.trim_end_matches(';').trim().parse().ok();
        }
    }
    None
}

fn plist_valid(label: &str) -> bool {
    let p = home()
        .join("Library/LaunchAgents")
        .join(format!("{label}.plist"));
    p.is_file()
        && Command::new("plutil")
            .arg("-lint")
            .arg(&p)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
}

fn cmd_output(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn disk_free_gb(path: &Path) -> Option<f64> {
    let out = Command::new("df").arg("-Pk").arg(path).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let avail_kb: f64 = s.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()?;
    Some(avail_kb / 1_048_576.0)
}

fn plist_check(label: &str, name: &'static str) -> Check {
    if plist_valid(label) {
        Check::ok(name, "system", "installed + valid")
    } else {
        Check::critical(name, "system", "missing or invalid").with_remedy("run ./install.sh")
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
    let run_check = match launchd_pid(LABEL_DAEMON) {
        Some(pid) => Check::ok("daemon running", "system", format!("pid {pid}")),
        None => {
            Check::critical("daemon running", "system", "not loaded").with_remedy("meridian start")
        }
    };
    vec![
        bin_check,
        plist_check(LABEL_DAEMON, "daemon plist"),
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
    let os = if cfg!(target_os = "macos") {
        Check::ok("os", "system", "macOS")
    } else {
        Check::warn(
            "os",
            "system",
            "not macOS — the capture stack is macOS-only",
        )
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
    match disk_free_gb(path) {
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
    disk_free_gb(&home().join(".meridian")).filter(|gb| *gb < DISK_LOW_THRESHOLD_GB)
}
