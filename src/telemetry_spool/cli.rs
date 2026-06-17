//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// CLI subcommands for `meridian telemetry`.
//
//   meridian telemetry status
//     Print pending/sent file counts + bytes, and whether a ship target is configured.
//
//   meridian telemetry export [--out <path.tar.gz>] [--since <RFC3339>]
//     Bundle pending/ + sent/ .otlp files into a tar.gz for remote debugging.
//     --since filters by filename timestamp >= that instant.
//     Prints the output path + file count.
//
//   meridian telemetry import <bundle.tar.gz> [--endpoint <url>] [--auth <base64>]
//     Extract the bundle to a temp dir and POST each .otlp to the target OO.
//     Endpoint defaults to resolve_otlp_endpoint(), auth defaults to resolve_otlp_target().
//     Prints success/fail counts.

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};

use crate::{
    observability::{resolve_otlp_endpoint, resolve_otlp_target},
    telemetry_spool::{
        derive_base_url, ship_one,
        writer::{
            micros_from_filename, pending_dir, resolve_telemetry_dir, sent_dir, signal_from_filename,
        },
    },
};

// ─────────────────────────────────────────────────────────────────────────────
// Public entry point
// ─────────────────────────────────────────────────────────────────────────────

/// Dispatch `meridian telemetry <subcommand> [args...]`.
///
/// `args` is `std::env::args().collect()` — the full argv starting at argv[0].
pub async fn run(args: &[String]) {
    // args[1] == "telemetry", args[2] == subcommand (if present)
    match args.get(2).map(String::as_str) {
        Some("status") => {
            if let Err(e) = cmd_status() {
                eprintln!("telemetry status: {e}");
                std::process::exit(1);
            }
        }
        Some("export") => {
            let out = flag_value(args, "--out");
            let since = flag_value(args, "--since");
            if let Err(e) = cmd_export(out.as_deref(), since.as_deref()) {
                eprintln!("telemetry export: {e}");
                std::process::exit(1);
            }
        }
        Some("import") => {
            let bundle = match args.get(3) {
                Some(p) => PathBuf::from(p),
                None => {
                    eprintln!("usage: meridian telemetry import <bundle.tar.gz> [--endpoint <url>] [--auth <base64>]");
                    std::process::exit(2);
                }
            };
            let endpoint = flag_value(args, "--endpoint");
            let auth = flag_value(args, "--auth");
            if let Err(e) = cmd_import(&bundle, endpoint.as_deref(), auth.as_deref()).await {
                eprintln!("telemetry import: {e}");
                std::process::exit(1);
            }
        }
        Some(other) => {
            eprintln!("telemetry: unknown subcommand {other:?}  (known: status, export, import)");
            std::process::exit(2);
        }
        None => {
            eprintln!("usage: meridian telemetry <status|export|import> [flags]");
            std::process::exit(2);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// status
// ─────────────────────────────────────────────────────────────────────────────

fn cmd_status() -> Result<()> {
    let base = resolve_telemetry_dir()?;
    let (p_count, p_bytes) = dir_stats(&pending_dir(&base));
    let (s_count, s_bytes) = dir_stats(&sent_dir(&base));

    let configured = if resolve_otlp_target().is_some() {
        "yes"
    } else if resolve_otlp_endpoint().is_some() {
        "endpoint-only (no credentials)"
    } else {
        "no"
    };

    println!("Telemetry spool: {}", base.display());
    println!("  pending:  {p_count} files  ({} bytes)", p_bytes);
    println!("  sent:     {s_count} files  ({} bytes)", s_bytes);
    println!("  ship target configured: {configured}");
    Ok(())
}

fn dir_stats(dir: &Path) -> (usize, u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    entries.flatten().fold((0, 0), |(count, bytes), entry| {
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        (count + 1, bytes + size)
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// export
// ─────────────────────────────────────────────────────────────────────────────

fn cmd_export(out: Option<&str>, since: Option<&str>) -> Result<()> {
    let base = resolve_telemetry_dir()?;

    // Parse --since as RFC3339 → unix micros threshold.
    let since_micros: Option<u64> = if let Some(s) = since {
        let dt = chrono::DateTime::parse_from_rfc3339(s)
            .with_context(|| format!("parse --since value {s:?} as RFC3339"))?;
        Some(dt.timestamp_micros() as u64)
    } else {
        None
    };

    // Collect .otlp files from both pending/ and sent/.
    let mut all_files: Vec<PathBuf> = Vec::new();
    for dir in [pending_dir(&base), sent_dir(&base)] {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().is_none_or(|e| e != "otlp") {
                    continue;
                }
                if let Some(thresh) = since_micros {
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    let file_micros = micros_from_filename(name).unwrap_or(0);
                    if file_micros < thresh {
                        continue;
                    }
                }
                all_files.push(p);
            }
        }
    }

    if all_files.is_empty() {
        println!("telemetry export: no files found");
        return Ok(());
    }

    // Build output path.
    let out_path = if let Some(p) = out {
        PathBuf::from(p)
    } else {
        let micros = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;
        base.join(format!("export-{micros}.tar.gz"))
    };

    // Write tar.gz.
    let file = std::fs::File::create(&out_path)
        .with_context(|| format!("create {}", out_path.display()))?;
    let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut tar = tar::Builder::new(enc);

    let file_count = all_files.len();
    for file_path in &all_files {
        let name = file_path.file_name().unwrap_or_default();
        tar.append_path_with_name(file_path, name)
            .with_context(|| format!("add {} to archive", file_path.display()))?;
    }

    tar.finish().context("finish tar archive")?;

    println!("{}", out_path.display());
    println!("Exported {file_count} files");
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// import
// ─────────────────────────────────────────────────────────────────────────────

async fn cmd_import(
    bundle: &Path,
    endpoint_override: Option<&str>,
    auth_override: Option<&str>,
) -> Result<()> {
    // Resolve endpoint + auth.
    let base_url = if let Some(ep) = endpoint_override {
        derive_base_url(ep)
    } else if let Some(ep) = resolve_otlp_endpoint() {
        derive_base_url(&ep)
    } else {
        return Err(anyhow::anyhow!(
            "no OO endpoint configured — pass --endpoint <url>"
        ));
    };

    let auth = if let Some(a) = auth_override {
        a.to_string()
    } else if let Some(t) = resolve_otlp_target() {
        t.auth
    } else {
        return Err(anyhow::anyhow!(
            "no OO credentials configured — pass --auth <base64>"
        ));
    };

    // Extract to a temp dir.
    let tmp = tempfile::tempdir().context("create temp dir for import")?;
    let file =
        std::fs::File::open(bundle).with_context(|| format!("open bundle {}", bundle.display()))?;
    let dec = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(dec);
    archive
        .unpack(tmp.path())
        .context("extract tar.gz bundle")?;

    // POST each .otlp file.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("build reqwest client")?;

    let entries: Vec<PathBuf> = std::fs::read_dir(tmp.path())
        .context("read extracted bundle")?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "otlp"))
        .collect();

    let mut ok = 0usize;
    let mut fail = 0usize;

    for path in &entries {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let signal = signal_from_filename(name).unwrap_or("traces");
        let endpoint = format!("{base_url}/v1/{signal}");

        let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;

        match ship_one(&client, &endpoint, &auth, bytes).await {
            Ok(()) => ok += 1,
            Err(e) => {
                eprintln!("  FAIL {name}: {e}");
                fail += 1;
            }
        }
    }

    println!("import: {ok} succeeded, {fail} failed");
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}
