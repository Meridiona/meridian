//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The `meridian db <check|repair>` terminal UI.
//!
//! Presentation only - every decision lives in [`crate::db::integrity`] and
//! [`crate::db::repair`]. Kept out of `main.rs`, which is already far past the
//! repo's 500-line ceiling and has nothing to do with printing a report.
//!
//! Output goes to stdout rather than `tracing`: this is a command an operator
//! runs and reads in a terminal, and the telemetry spool is the wrong channel
//! for something whose whole purpose is to be answered right now (see
//! `observability.rs` on why the spool has no stdout mirror).
//!
//! # Who calls this
//!
//! `src/main.rs`'s argv dispatch, ahead of the daemon's single-instance guard.
//!
//! # Related
//!
//! - [`crate::db::repair::ensure_no_writers`] - the precondition enforced here
//!   rather than inside `repair()`, so the operation itself stays testable.

use crate::config::Config;

/// Backs `meridian db <check|repair>`. Returns the process exit code.
///
/// Output goes to stdout rather than `tracing`, because this is a command an
/// operator runs and reads in a terminal - the telemetry spool is the wrong
/// channel for something whose whole purpose is to be answered right now (see
/// `observability.rs` on why the spool has no stdout mirror).
///
/// Exit codes are meaningful so this can be scripted: 0 clean, 1 damage found
/// or repair failed, 2 bad usage.
pub async fn run_db_command(sub: &str) -> i32 {
    let cfg = Config::from_env();
    let db_path = std::path::PathBuf::from(&cfg.meridian_db);
    let key = std::env::var("MERIDIAN_DB_KEY").ok();

    match sub {
        "check" => match crate::db::repair::inspect(&db_path, key.as_deref()).await {
            Ok((problems, scan)) if problems.is_empty() && scan.is_healthy() => {
                println!(
                    "meridian.db is healthy ({} tables scanned).",
                    scan.healthy.len()
                );
                0
            }
            Ok((problems, scan)) => {
                println!("meridian.db is DAMAGED.\n");
                if !problems.is_empty() {
                    let shown = problems.len().min(10);
                    println!("Structural problems ({shown} of {} shown):", problems.len());
                    for p in problems.iter().take(shown) {
                        println!("  {p}");
                    }
                    println!();
                }
                if !scan.corrupt.is_empty() {
                    println!("Unreadable tables:");
                    for t in &scan.corrupt {
                        let kind = if crate::db::integrity::DISPOSABLE_TABLES.contains(&t.as_str())
                        {
                            "capture scratch - safe to lose"
                        } else {
                            "contains your data"
                        };
                        println!("  {t}  ({kind})");
                    }
                    println!();
                }
                println!(
                    "Run 'meridian db repair' to rebuild it, keeping every row that still reads."
                );
                println!("Quit the Meridian app and stop the daemon first.");
                1
            }
            Err(e) => {
                eprintln!("could not inspect the database: {e:#}");
                1
            }
        },
        "repair" => {
            // Enforced here, not inside repair(): see that function's doc for
            // why the precondition lives at the boundary.
            if let Err(e) = crate::db::repair::ensure_no_writers().await {
                eprintln!("cannot repair while the database is in use:\n  {e}");
                return 1;
            }
            println!(
                "Rebuilding {} - this keeps every row that can still be read.",
                db_path.display()
            );
            match crate::db::repair::repair_default(&db_path).await {
                Ok(report) => {
                    println!("\nDone.");
                    println!("  rows recovered : {}", report.rows_copied());
                    println!("  rows lost      : {}", report.rows_unreadable());
                    if report.rows_rejected() > 0 {
                        println!(
                            "  rows dropped by the current schema (not corruption): {}",
                            report.rows_rejected()
                        );
                    }
                    let lossy = report.lossy_tables();
                    if !lossy.is_empty() {
                        println!("\nTables that lost data:");
                        for t in lossy {
                            if t.left_empty {
                                println!("  {} - emptied (capture scratch)", t.table);
                            } else {
                                println!("  {} - {} row(s) unreadable", t.table, t.rows_unreadable);
                            }
                        }
                    }
                    println!(
                        "\n  before : {} MB\n  after  : {} MB",
                        report.before_bytes / 1_000_000,
                        report.after_bytes / 1_000_000
                    );
                    println!(
                        "\nThe damaged original is kept at:\n  {}\nDelete it once you are satisfied with the result.",
                        report.original_backup.display()
                    );
                    0
                }
                Err(e) => {
                    eprintln!("\nrepair failed: {e:#}");
                    eprintln!("Your database has NOT been modified.");
                    1
                }
            }
        }
        other => {
            eprintln!("usage: meridian db <check|repair>");
            if !other.is_empty() {
                eprintln!("unknown subcommand: {other}");
            }
            2
        }
    }
}
