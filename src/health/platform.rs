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

/// MLX capability recorded by the installers in `~/.meridian/capabilities`.
/// `mlx=unsupported_intel_hardware` means this Mac can never run MLX (mlx
/// ships arm64-only wheels) — doctor reports that as fact, not as a failure
/// with an unfollowable remedy.
pub fn mlx_unsupported() -> bool {
    std::fs::read_to_string(home().join(".meridian/capabilities"))
        .map(|s| {
            s.lines()
                .any(|l| l.trim() == "mlx=unsupported_intel_hardware")
        })
        .unwrap_or(false)
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
    // Source checkout: <repo>/services/.venv. Bundle install: the venv lives
    // under ~/.meridian/app (no Cargo.toml, so repo_root() is None).
    let (py, remedy) = match repo_root() {
        Some(r) => (
            r.join("services/.venv/bin/python"),
            "bash scripts/setup-services.sh",
        ),
        None => (
            home().join(".meridian/app/services/.venv/bin/python"),
            "meridian update",
        ),
    };
    if !is_exec(&py) {
        return Check::critical("python venv", "system", ".venv missing").with_remedy(remedy);
    }
    // A venv built by a Rosetta/Intel python3 carries x86_64 native extensions
    // an arm64 interpreter can never import — rebuilding is the only fix.
    let arch = Command::new(&py)
        .args(["-c", "import platform; print(platform.machine())"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    match arch.as_deref() {
        Some("arm64") => {}
        Some(other) => {
            return Check::critical(
                "python venv",
                "system",
                format!("venv python is {other}, not arm64 — mixed-architecture venv"),
            )
            .with_remedy(remedy)
        }
        None => {
            return Check::critical("python venv", "system", "venv python failed to run")
                .with_remedy(remedy)
        }
    }
    // Import the module the launchd agent actually execs (`python -m
    // agents.server`) — pulls in pydantic/fastapi/mlx, so it catches broken or
    // mixed-architecture native extensions, not just a present venv dir.
    let ok = Command::new(&py)
        .args(["-c", "import agents.server"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if ok {
        Check::ok("python venv", "system", "agents.server importable")
    } else {
        Check::critical("python venv", "system", "agents.server import failed").with_remedy(remedy)
    }
}

/// Returns a single Info check confirming the dashboard is embedded in the
/// Tauri binary. The `com.meridiona.ui` launchd agent was retired with the
/// Next-fold (PR #298); probing for its plist always produces a false CRITICAL
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

fn disk_check(name: &'static str, path: &Path) -> Check {
    match disk_free_gb(path) {
        Some(gb) if gb < 2.0 => Check::warn(name, "system", format!("{gb:.1} GB free — low"))
            .with_remedy("free disk space"),
        Some(gb) => Check::ok(name, "system", format!("{gb:.0} GB free")),
        None => Check::info(name, "system", "usage unknown"),
    }
}
