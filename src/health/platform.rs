// meridian — normalises screenpipe activity into structured app sessions
//
// Platform / prerequisite checks: launchd agents, plist validity, process
// liveness, installed binaries, build artifacts, toolchains, and disk. These
// are the "is the environment set up to run at all" rungs, distributed into
// each daemon's table group. Shelling out to launchctl/plutil/pgrep/df/node.

use crate::config::Config;
use crate::health::Check;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A daemon pid younger than this, paired with a non-zero last exit, is taken
/// to be crash-looping (the launchd respawn interval is ~10s).
const CRASH_LOOP_UPTIME_SECS: u64 = 90;

const LABEL_DAEMON: &str = "com.meridiona.daemon";
const LABEL_SCREENPIPE: &str = "com.meridiona.screenpipe";
const LABEL_UI: &str = "com.meridiona.ui";
const LABEL_MLX: &str = "com.meridiona.mlx-server";

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
    launchd_list_field(label, "PID")
}

/// launchd's last exit status for a job (0 = clean exit). Reflects the
/// *previous* run, not the live process — a SIGTERM restart leaves it non-zero
/// for a while — so on its own it is not proof of a crash-loop; the caller
/// pairs it with pid uptime. None if the field is unavailable.
fn launchd_last_exit(label: &str) -> Option<i64> {
    launchd_list_field(label, "LastExitStatus")
}

/// Parse a single `"<field>" = <int>;` line out of `launchctl list <label>`.
fn launchd_list_field(label: &str, field: &str) -> Option<i64> {
    let out = Command::new("launchctl")
        .args(["list", label])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let needle = format!("\"{field}\" = ");
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(rest) = line.trim().strip_prefix(&needle) {
            return rest.trim_end_matches(';').trim().parse().ok();
        }
    }
    None
}

/// Elapsed seconds since `pid` started, via BSD `ps -o etime` (macOS `ps` has
/// no `etimes`). None if the pid is gone or the field can't be parsed.
fn pid_uptime_secs(pid: i64) -> Option<u64> {
    parse_etime(cmd_output("ps", &["-o", "etime=", "-p", &pid.to_string()])?.trim())
}

/// Parse BSD `etime` (`[[DD-]HH:]MM:SS`) into seconds.
fn parse_etime(s: &str) -> Option<u64> {
    if s.is_empty() {
        return None;
    }
    let (days, hms) = match s.split_once('-') {
        Some((d, rest)) => (d.parse::<u64>().ok()?, rest),
        None => (0, s),
    };
    let mut secs = 0u64;
    for part in hms.split(':') {
        secs = secs
            .checked_mul(60)?
            .checked_add(part.parse::<u64>().ok()?)?;
    }
    Some(days * 86_400 + secs)
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

fn process_running(name: &str) -> bool {
    Command::new("pgrep")
        .args(["-x", name])
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
    // A PID alone is not health: during a crash-loop launchd reports the pid of
    // the process that is about to die, so a daemon failing on startup (e.g. a
    // modified migration aborting sqlx) reads as a green "running" line. Catch
    // it by combining two signals — a non-zero LastExitStatus AND a very young
    // current pid. Either alone is benign: LastExitStatus stays non-zero after a
    // SIGTERM restart (the value reflects the *previous* run, not the live one),
    // and a young pid alone is just a fresh start. Only the pair — respawning
    // fast while exiting non-zero — is a crash-loop.
    let run_check = match launchd_pid(LABEL_DAEMON) {
        Some(pid) => {
            let exited_badly = launchd_last_exit(LABEL_DAEMON).is_some_and(|c| c != 0);
            let just_respawned = pid_uptime_secs(pid).is_some_and(|s| s < CRASH_LOOP_UPTIME_SECS);
            if exited_badly && just_respawned {
                Check::critical(
                    "daemon running",
                    "system",
                    format!(
                        "pid {pid} respawned <{CRASH_LOOP_UPTIME_SECS}s ago after a non-zero exit — crash-looping on startup"
                    ),
                )
                .with_remedy(
                    "inspect ~/.meridian/logs/daemon-error.log for a repeating startup error \
                     (often a modified migration); then meridian doctor --fix",
                )
            } else {
                Check::ok("daemon running", "system", format!("pid {pid}"))
            }
        }
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

pub fn screenpipe_service() -> Vec<Check> {
    let run = if process_running("screenpipe") {
        Check::ok("screenpipe service", "L1", "process alive")
    } else {
        Check::critical("screenpipe service", "L1", "not running").with_remedy("meridian start")
    };
    vec![plist_check(LABEL_SCREENPIPE, "screenpipe plist"), run]
}

pub fn mlx_service(_cfg: &Config) -> Vec<Check> {
    let run = match launchd_pid(LABEL_MLX) {
        Some(pid) => Check::ok("mlx service", "L2", format!("pid {pid}")),
        None => Check::warn("mlx service", "L2", "launchd agent not loaded")
            .with_remedy("meridian start"),
    };
    vec![plist_check(LABEL_MLX, "mlx plist"), run, venv_check()]
}

fn venv_check() -> Check {
    let py = match repo_root() {
        Some(r) => r.join("services/.venv/bin/python"),
        None => return Check::info("python venv", "system", "repo root not found"),
    };
    if !is_exec(&py) {
        return Check::critical("python venv", "system", ".venv missing")
            .with_remedy("bash scripts/setup-services.sh");
    }
    let ok = Command::new(&py)
        .args(["-c", "import run_agent"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if ok {
        Check::ok("python venv", "system", "run_agent importable")
    } else {
        Check::critical("python venv", "system", "run_agent import failed")
            .with_remedy("bash scripts/setup-services.sh")
    }
}

pub fn ui_service() -> Vec<Check> {
    let run = match launchd_pid(LABEL_UI) {
        Some(pid) => Check::ok("ui service", "system", format!("pid {pid}")),
        None => Check::warn("ui service", "system", "not loaded — dashboard unavailable")
            .with_remedy("meridian start"),
    };
    let built = repo_root()
        .map(|r| r.join("ui/.next").is_dir())
        .unwrap_or(false);
    let build_check = if built {
        Check::ok("ui built", "system", ".next present")
    } else {
        Check::warn("ui built", "system", "not built")
            .with_remedy("cd ui && npm ci && npm run build")
    };
    vec![plist_check(LABEL_UI, "ui plist"), run, build_check]
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
        disk_check("disk (screenpipe)", &home().join(".screenpipe")),
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

fn disk_check(name: &'static str, path: &Path) -> Check {
    match disk_free_gb(path) {
        Some(gb) if gb < 2.0 => Check::warn(name, "system", format!("{gb:.1} GB free — low"))
            .with_remedy("free disk space"),
        Some(gb) => Check::ok(name, "system", format!("{gb:.0} GB free")),
        None => Check::info(name, "system", "usage unknown"),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_etime;

    #[test]
    fn parses_mm_ss() {
        assert_eq!(parse_etime("07:47"), Some(7 * 60 + 47));
        assert_eq!(parse_etime("00:09"), Some(9));
    }

    #[test]
    fn parses_hh_mm_ss_and_days() {
        assert_eq!(parse_etime("01:02:03"), Some(3600 + 120 + 3));
        assert_eq!(
            parse_etime("2-03:04:05"),
            Some(2 * 86_400 + 3 * 3600 + 4 * 60 + 5)
        );
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_etime(""), None);
        assert_eq!(parse_etime("not-a-time"), None);
    }

    #[test]
    fn young_pid_under_crash_loop_threshold() {
        // a 9-second-old pid is "just respawned"; a 7-minute-old one is not
        assert!(parse_etime("00:09").unwrap() < super::CRASH_LOOP_UPTIME_SECS);
        assert!(parse_etime("07:47").unwrap() >= super::CRASH_LOOP_UPTIME_SECS);
    }
}
