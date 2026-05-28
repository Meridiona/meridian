// meridian — normalises screenpipe activity into structured app sessions

//! Backfill task classification for a specific session range.
//! Does not touch the agent_cursor — safe to re-run multiple times.
//! Requires the persistent MLX server to be running on MLX_SERVER_PORT (default 7823).
//!
//! Usage:
//!   backfill-task-classification --today
//!   backfill-task-classification --yesterday
//!   backfill-task-classification --from-date 2025-05-01 --to-date 2025-05-14
//!   backfill-task-classification --from-id 100 --to-id 500
//!   backfill-task-classification --from-id 100           # from 100 onwards
//!   backfill-task-classification --dry-run --today        # print without writing

use anyhow::{bail, Context, Result};
use chrono::{Local, NaiveDate, TimeZone, Utc};
use meridian::{config::Config, intelligence::link_range};
use sqlx::sqlite::SqlitePoolOptions;

// ---------------------------------------------------------------------------
// Arg parsing (no clap dep — keep it simple)
// ---------------------------------------------------------------------------

struct Args {
    from_id: Option<i64>,
    to_id: Option<i64>,
    session: Option<i64>,
    from_date: Option<NaiveDate>,
    to_date: Option<NaiveDate>,
    today: bool,
    yesterday: bool,
    dry_run: bool,
}

fn print_usage() {
    eprintln!(
        "usage: backfill-task-classification [--today | --yesterday \
         | --from-date YYYY-MM-DD [--to-date YYYY-MM-DD] \
         | --from-id N [--to-id N] | --session N] [--dry-run]"
    );
}

fn parse_args() -> Result<Args> {
    let mut args = Args {
        from_id: None,
        to_id: None,
        session: None,
        from_date: None,
        to_date: None,
        today: false,
        yesterday: false,
        dry_run: false,
    };
    let mut argv = std::env::args().skip(1).peekable();
    while let Some(flag) = argv.next() {
        match flag.as_str() {
            "--today" => args.today = true,
            "--yesterday" => args.yesterday = true,
            "--dry-run" => args.dry_run = true,
            "--session" => {
                let v = argv.next().context("--session requires a value")?;
                args.session = Some(v.parse::<i64>().context("--session must be an integer")?);
            }
            "--from-id" => {
                let v = argv.next().context("--from-id requires a value")?;
                args.from_id = Some(v.parse::<i64>().context("--from-id must be an integer")?);
            }
            "--to-id" => {
                let v = argv.next().context("--to-id requires a value")?;
                args.to_id = Some(v.parse::<i64>().context("--to-id must be an integer")?);
            }
            "--from-date" => {
                let v = argv.next().context("--from-date requires YYYY-MM-DD")?;
                args.from_date = Some(
                    NaiveDate::parse_from_str(&v, "%Y-%m-%d")
                        .context("--from-date must be YYYY-MM-DD")?,
                );
            }
            "--to-date" => {
                let v = argv.next().context("--to-date requires YYYY-MM-DD")?;
                args.to_date = Some(
                    NaiveDate::parse_from_str(&v, "%Y-%m-%d")
                        .context("--to-date must be YYYY-MM-DD")?,
                );
            }
            other => bail!("unknown flag: {other}"),
        }
    }
    Ok(args)
}

// ---------------------------------------------------------------------------
// Range → (from_id, to_id) by querying the DB
// ---------------------------------------------------------------------------

async fn resolve_id_range(
    pool: &sqlx::SqlitePool,
    args: &Args,
    min_duration_s: i64,
) -> Result<(i64, Option<i64>)> {
    if let Some(id) = args.session {
        return Ok((id, Some(id)));
    }
    if args.from_id.is_some() || args.to_id.is_some() {
        return Ok((args.from_id.unwrap_or(0), args.to_id));
    }

    let (from_date, to_date_exclusive) = if args.today {
        let d = Local::now().date_naive();
        (d, d.succ_opt())
    } else if args.yesterday {
        let d = Local::now()
            .date_naive()
            .pred_opt()
            .context("date underflow")?;
        (d, d.succ_opt())
    } else if let Some(fd) = args.from_date {
        (fd, args.to_date.and_then(|d| d.succ_opt()))
    } else {
        bail!("specify --today, --yesterday, --from-date, or --from-id");
    };

    let from_utc = Local
        .from_local_datetime(&from_date.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .context("ambiguous local time")?
        .with_timezone(&Utc)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();

    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT MIN(id) FROM app_sessions
         WHERE started_at >= ? AND duration_s >= ?",
    )
    .bind(&from_utc)
    .bind(min_duration_s)
    .fetch_optional(pool)
    .await
    .context("querying min session id")?;

    let from_id = match row {
        Some((id,)) => id,
        None => {
            eprintln!("No sessions found for the specified range.");
            std::process::exit(0);
        }
    };

    let to_id = if let Some(to_date) = to_date_exclusive {
        let to_utc = Local
            .from_local_datetime(&to_date.and_hms_opt(0, 0, 0).unwrap())
            .single()
            .context("ambiguous local time")?
            .with_timezone(&Utc)
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();

        let row: Option<(i64,)> = sqlx::query_as(
            "SELECT MAX(id) FROM app_sessions
             WHERE started_at < ? AND duration_s >= ?",
        )
        .bind(&to_utc)
        .bind(min_duration_s)
        .fetch_optional(pool)
        .await
        .context("querying max session id")?;

        row.map(|(id,)| id)
    } else {
        None
    };

    Ok((from_id, to_id))
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            print_usage();
            std::process::exit(1);
        }
    };

    let cfg = Config::from_env();

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&cfg.meridian_db_uri())
        .await
        .context("opening meridian.db")?;

    sqlx::migrate!("./src/migrations")
        .run(&pool)
        .await
        .context("running migrations")?;

    let (from_id, to_id) =
        resolve_id_range(&pool, &args, cfg.min_classification_duration_s).await?;

    println!(
        "Backfilling task classification: from_id={from_id}  to_id={}  dry_run={}",
        to_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "∞".to_string()),
        args.dry_run,
    );

    let (processed, linked) = link_range(&pool, &cfg, from_id, to_id, args.dry_run).await?;

    println!(
        "Done. processed={processed}  linked={linked}{}",
        if args.dry_run {
            "  (dry run — nothing written)"
        } else {
            ""
        }
    );

    Ok(())
}
