// meridian — normalises screenpipe activity into structured app sessions
// https://github.com/meridiona/meridian

use anyhow::Result;
use sqlx::SqlitePool;
use tracing::{debug, info, warn};

use crate::db::meridian::{
    close_active_session, complete_etl_run, get_active_session, get_cursor, insert_etl_run,
    insert_gap, update_cursor, upsert_active_session, ActiveSession,
};
use crate::db::screenpipe::{
    count_frames_in_window, get_frames_since, AudioSnippet, ElementSample, OcrSample, SignalEvent,
    WindowTitleCount,
};
use crate::etl::extractor::extract_block_context;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const BATCH_SIZE: i64 = 500;
const OCR_SAMPLE_CAP: usize = 20;
const GAP_THRESHOLD_SECS: i64 = 300;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Runs one full ETL cycle:
///   1. Reads frames from screenpipe in batches of 500.
///   2. Groups consecutive frames by `app_name` (strict: every change = new session).
///   3. Closes finished sessions into `app_sessions` and keeps the still-open
///      block in `active_session`.
///   4. Advances the cursor and writes the ETL audit row.
pub async fn run_etl(screenpipe: &SqlitePool, meridian: &SqlitePool) -> Result<()> {
    // ------------------------------------------------------------------
    // 1. Read cursor
    // ------------------------------------------------------------------
    let cursor = get_cursor(meridian).await?;
    let mut last_processed_id = cursor.last_frame_id;
    let run_start_cursor = last_processed_id;

    info!(from_frame_id = last_processed_id, "ETL run starting");

    // ------------------------------------------------------------------
    // 2. Peek at whether there is anything to process
    // ------------------------------------------------------------------
    let first_batch = get_frames_since(screenpipe, last_processed_id, BATCH_SIZE).await?;
    if first_batch.is_empty() {
        info!("no new frames — nothing to do");
        return Ok(());
    }

    // We now know the approximate upper bound for the audit row.
    let approx_to_frame_id = first_batch
        .last()
        .map(|f| f.id)
        .unwrap_or(last_processed_id);

    // ------------------------------------------------------------------
    // 3. Insert ETL run (status = running)
    // ------------------------------------------------------------------
    let run_id = insert_etl_run(meridian, run_start_cursor, approx_to_frame_id).await?;
    info!(run_id, "ETL run row inserted");

    // ------------------------------------------------------------------
    // 4. Process frames in batches
    // ------------------------------------------------------------------
    let mut sessions_closed: i64 = 0;

    // Carry over the in-flight block state across batches.
    let mut current_app: Option<String> = None;
    let mut block_start_frame_id: i64 = 0;
    let mut block_start_ts: String = String::new();
    let mut block_frame_count: i64 = 0;
    let mut block_idle_frame_count: i64 = 0;
    let mut block_last_ts: String = String::new();
    let mut block_last_frame_id: i64 = 0;

    let result: Result<()> = async {
        let mut batch = first_batch;

        loop {
            for frame in &batch {
                let app = frame.app_name.trim();
                if app.is_empty() {
                    // Extend whatever block is open without changing the app.
                    if current_app.is_some() {
                        block_frame_count += 1;
                        block_last_ts = frame.timestamp.clone();
                        block_last_frame_id = frame.id;
                    }
                    debug!(frame_id = frame.id, "skipping frame with empty app_name");
                    continue;
                }

                // ----------------------------------------------------------
                // Gap detection: check for a long pause between the last
                // frame we processed and this one.  This runs BEFORE the
                // app-switch state machine so it observes every inter-frame
                // gap regardless of whether the app changed.
                // ----------------------------------------------------------
                if current_app.is_some() {
                    if let Some(gap) = timestamp_gap_secs(&block_last_ts, &frame.timestamp) {
                        if gap > GAP_THRESHOLD_SECS {
                            let (total, idle) = count_frames_in_window(
                                screenpipe,
                                &block_last_ts,
                                &frame.timestamp,
                            )
                            .await
                            .unwrap_or((0, 0));

                            let kind = if total > 0 && idle > 0 && (idle * 2 >= total) {
                                "user_idle"
                            } else {
                                "system_sleep"
                            };

                            insert_gap(
                                meridian,
                                &block_last_ts,
                                &frame.timestamp,
                                gap,
                                kind,
                                run_id,
                            )
                            .await?;

                            debug!(
                                gap_secs = gap,
                                kind,
                                from = block_last_ts,
                                to = frame.timestamp,
                                "gap recorded"
                            );

                            // Close the pre-gap block so its duration_s ends at
                            // block_last_ts (the last real frame before the gap),
                            // not at the first frame after. This prevents the gap
                            // time from inflating the session's focus duration.
                            let closing_app = current_app.take().unwrap();
                            sessions_closed += close_block(
                                screenpipe,
                                meridian,
                                run_id,
                                &BlockBounds {
                                    app: &closing_app,
                                    started_at: &block_start_ts,
                                    ended_at: &block_last_ts,
                                    min_frame_id: block_start_frame_id,
                                    max_frame_id: block_last_frame_id,
                                    frame_count: block_frame_count,
                                    idle_frame_count: block_idle_frame_count,
                                },
                            )
                            .await?;
                            // current_app is now None; the match below will
                            // start a fresh block for the post-gap frame.
                        }
                    }
                }

                match current_app.as_deref() {
                    None => {
                        // Very first frame — start a block.
                        debug!(frame_id = frame.id, app, "first frame — starting block");
                        current_app = Some(app.to_owned());
                        block_start_frame_id = frame.id;
                        block_start_ts = frame.timestamp.clone();
                        block_frame_count = 1;
                        block_idle_frame_count = if frame
                            .capture_trigger
                            .as_deref()
                            == Some("idle")
                        {
                            1
                        } else {
                            0
                        };
                        block_last_ts = frame.timestamp.clone();
                        block_last_frame_id = frame.id;
                    }

                    Some(cur) if cur == app => {
                        // Same app — extend the current block.
                        block_frame_count += 1;
                        if frame.capture_trigger.as_deref() == Some("idle") {
                            block_idle_frame_count += 1;
                        }
                        block_last_ts = frame.timestamp.clone();
                        block_last_frame_id = frame.id;
                    }

                    Some(cur) => {
                        // App changed.
                        let old_app = cur.to_owned();
                        debug!(
                            old_app = old_app,
                            new_app = app,
                            frame_id = frame.id,
                            "app changed — closing block"
                        );

                        // Close the old block.
                        sessions_closed += close_block(
                            screenpipe,
                            meridian,
                            run_id,
                            &BlockBounds {
                                app: &old_app,
                                started_at: &block_start_ts,
                                ended_at: &block_last_ts,
                                min_frame_id: block_start_frame_id,
                                max_frame_id: block_last_frame_id,
                                frame_count: block_frame_count,
                                idle_frame_count: block_idle_frame_count,
                            },
                        )
                        .await?;

                        // Start fresh block for the new app.
                        current_app = Some(app.to_owned());
                        block_start_frame_id = frame.id;
                        block_start_ts = frame.timestamp.clone();
                        block_frame_count = 1;
                        block_idle_frame_count = if frame
                            .capture_trigger
                            .as_deref()
                            == Some("idle")
                        {
                            1
                        } else {
                            0
                        };
                        block_last_ts = frame.timestamp.clone();
                        block_last_frame_id = frame.id;
                    }
                }
            }

            last_processed_id = batch.last().map(|f| f.id).unwrap_or(last_processed_id);

            // Fetch next batch.
            let next = get_frames_since(screenpipe, last_processed_id, BATCH_SIZE).await?;
            if next.is_empty() {
                break;
            }
            batch = next;
        }

        // ------------------------------------------------------------------
        // 5. Upsert the still-open block into active_session
        // ------------------------------------------------------------------
        if let Some(ref open_app) = current_app {
            if block_frame_count > 0 {
                sessions_closed += upsert_open_block(
                    screenpipe,
                    meridian,
                    run_id,
                    &BlockBounds {
                        app: open_app,
                        started_at: &block_start_ts,
                        ended_at: &block_last_ts,
                        min_frame_id: block_start_frame_id,
                        max_frame_id: block_last_frame_id,
                        frame_count: block_frame_count,
                        idle_frame_count: block_idle_frame_count,
                    },
                )
                .await?;
            }
        }

        Ok(())
    }
    .await;

    // ------------------------------------------------------------------
    // 6. Update cursor and finalise ETL run
    // ------------------------------------------------------------------
    if let Err(ref e) = result {
        warn!(error = %e, "ETL run failed — updating cursor to last successful frame");
        complete_etl_run(meridian, run_id, sessions_closed, Some(&e.to_string())).await?;
        update_cursor(meridian, last_processed_id, run_id).await?;
        return result;
    }

    update_cursor(meridian, last_processed_id, run_id).await?;
    complete_etl_run(meridian, run_id, sessions_closed, None).await?;

    info!(
        run_id,
        sessions_closed,
        last_frame_id = last_processed_id,
        "ETL run complete"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// BlockBounds — groups the positional fields shared by close_block /
// upsert_open_block so both stay under clippy's 7-argument limit.
// ---------------------------------------------------------------------------

struct BlockBounds<'a> {
    app: &'a str,
    started_at: &'a str,
    ended_at: &'a str,
    min_frame_id: i64,
    max_frame_id: i64,
    frame_count: i64,
    idle_frame_count: i64,
}

// ---------------------------------------------------------------------------
// timestamp_gap_secs
// ---------------------------------------------------------------------------

/// Parses two RFC3339 timestamps and returns the difference in whole seconds,
/// or `None` if either timestamp fails to parse.
fn timestamp_gap_secs(earlier: &str, later: &str) -> Option<i64> {
    let t0 = chrono::DateTime::parse_from_rfc3339(earlier).ok()?;
    let t1 = chrono::DateTime::parse_from_rfc3339(later).ok()?;
    Some((t1 - t0).num_seconds())
}

// ---------------------------------------------------------------------------
// close_block
// ---------------------------------------------------------------------------

async fn close_block(
    screenpipe: &SqlitePool,
    meridian: &SqlitePool,
    run_id: i64,
    b: &BlockBounds<'_>,
) -> Result<i64> {
    let ctx = extract_block_context(
        screenpipe,
        b.app,
        b.started_at,
        b.ended_at,
        b.min_frame_id,
        b.max_frame_id,
        b.frame_count,
    )
    .await?;

    let existing = get_active_session(meridian).await?;

    match existing {
        Some(ref active) if active.app_name == b.app => {
            debug!(app = b.app, "merging and closing continuation block");
            let merged = merge_into_active(active, &ctx, b.idle_frame_count)?;
            upsert_active_session(meridian, &merged).await?;
            close_active_session(meridian, run_id).await?;
            info!(app = b.app, "session closed (merged continuation)");
            Ok(1)
        }

        Some(ref active) => {
            warn!(
                stale_app = active.app_name,
                new_app = b.app,
                "stale active_session — closing stale first"
            );
            close_active_session(meridian, run_id).await?;
            let new_session = build_active_session(&ctx, b.idle_frame_count)?;
            upsert_active_session(meridian, &new_session).await?;
            close_active_session(meridian, run_id).await?;
            info!(app = b.app, "session closed (fresh, after evicting stale)");
            Ok(2)
        }

        None => {
            let new_session = build_active_session(&ctx, b.idle_frame_count)?;
            upsert_active_session(meridian, &new_session).await?;
            close_active_session(meridian, run_id).await?;
            info!(app = b.app, "session closed");
            Ok(1)
        }
    }
}

// ---------------------------------------------------------------------------
// upsert_open_block
// ---------------------------------------------------------------------------

async fn upsert_open_block(
    screenpipe: &SqlitePool,
    meridian: &SqlitePool,
    run_id: i64,
    b: &BlockBounds<'_>,
) -> Result<i64> {
    let ctx = extract_block_context(
        screenpipe,
        b.app,
        b.started_at,
        b.ended_at,
        b.min_frame_id,
        b.max_frame_id,
        b.frame_count,
    )
    .await?;

    let existing = get_active_session(meridian).await?;

    let session = match existing {
        Some(ref active) if active.app_name == b.app => {
            debug!(app = b.app, "merging new frames into existing active_session");
            merge_into_active(active, &ctx, b.idle_frame_count)?
        }

        Some(ref active) => {
            warn!(
                stale_app = active.app_name,
                new_app = b.app,
                "stale active_session while upserting open block"
            );
            close_active_session(meridian, run_id).await?;
            build_active_session(&ctx, b.idle_frame_count)?
        }

        None => build_active_session(&ctx, b.idle_frame_count)?,
    };

    upsert_active_session(meridian, &session).await?;
    debug!(app = b.app, max_frame_id = b.max_frame_id, "active_session upserted");
    Ok(0)
}

// ---------------------------------------------------------------------------
// Context helpers — build / merge ActiveSession
// ---------------------------------------------------------------------------

use crate::etl::extractor::BlockContext;

/// Builds a brand-new `ActiveSession` from a `BlockContext`.
fn build_active_session(ctx: &BlockContext, idle_frame_count: i64) -> Result<ActiveSession> {
    Ok(ActiveSession {
        id: 1,
        app_name: ctx.app_name.clone(),
        started_at: ctx.started_at.clone(),
        last_seen_at: ctx.ended_at.clone(),
        window_titles: serde_json::to_string(&ctx.window_titles)?,
        ocr_samples: Some(serde_json::to_string(&ctx.ocr_samples)?),
        elements_samples: Some(serde_json::to_string(&ctx.elements_samples)?),
        audio_snippets: Some(serde_json::to_string(&ctx.audio_snippets)?),
        signals: Some(serde_json::to_string(&ctx.signals)?),
        min_frame_id: ctx.min_frame_id,
        max_frame_id: ctx.max_frame_id,
        frame_count: ctx.frame_count,
        idle_frame_count,
    })
}

/// Merges a new `BlockContext` into an existing `ActiveSession` row and
/// returns the updated session.
///
/// Merge rules:
///
/// - `started_at`: kept from the existing session (the block started earlier).
/// - `last_seen_at`: set to now.
/// - `min_frame_id`: kept from the existing session.
/// - `max_frame_id`: updated to the new block's max.
/// - `frame_count`: summed.
/// - `window_titles`: counts from identical titles are incremented; new titles are appended.
/// - `ocr_samples`: appended, capped at `OCR_SAMPLE_CAP` (20) total.
/// - `elements_samples`: appended, capped at `OCR_SAMPLE_CAP` (20) total.
/// - `audio_snippets`: all new snippets appended (no cap — audio is sparse).
/// - `signals`: all new signals appended.
fn merge_into_active(
    existing: &ActiveSession,
    ctx: &BlockContext,
    new_idle_frame_count: i64,
) -> Result<ActiveSession> {
    let now = ctx.ended_at.clone();

    // -- window_titles --
    let mut merged_titles: Vec<WindowTitleCount> =
        serde_json::from_str(&existing.window_titles).unwrap_or_default();
    for new_t in &ctx.window_titles {
        if let Some(existing_t) = merged_titles
            .iter_mut()
            .find(|t| t.window_name == new_t.window_name)
        {
            existing_t.count += new_t.count;
        } else {
            merged_titles.push(new_t.clone());
        }
    }
    // Re-sort descending by count so the JSON stays human-readable.
    merged_titles.sort_by(|a, b| b.count.cmp(&a.count));

    // -- ocr_samples --
    let mut ocr: Vec<OcrSample> = existing
        .ocr_samples
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    for sample in &ctx.ocr_samples {
        if ocr.len() >= OCR_SAMPLE_CAP {
            break;
        }
        ocr.push(sample.clone());
    }

    // -- elements_samples --
    let mut elements: Vec<ElementSample> = existing
        .elements_samples
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    for sample in &ctx.elements_samples {
        if elements.len() >= OCR_SAMPLE_CAP {
            break;
        }
        elements.push(sample.clone());
    }

    // -- audio_snippets --
    let mut audio: Vec<AudioSnippet> = existing
        .audio_snippets
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    audio.extend(ctx.audio_snippets.iter().cloned());

    // -- signals --
    let mut signals: Vec<SignalEvent> = existing
        .signals
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    signals.extend(ctx.signals.iter().cloned());

    Ok(ActiveSession {
        id: 1,
        app_name: existing.app_name.clone(),
        started_at: existing.started_at.clone(),
        last_seen_at: now,
        window_titles: serde_json::to_string(&merged_titles)?,
        ocr_samples: Some(serde_json::to_string(&ocr)?),
        elements_samples: Some(serde_json::to_string(&elements)?),
        audio_snippets: Some(serde_json::to_string(&audio)?),
        signals: Some(serde_json::to_string(&signals)?),
        min_frame_id: existing.min_frame_id,
        max_frame_id: ctx.max_frame_id,
        frame_count: existing.frame_count + ctx.frame_count,
        idle_frame_count: existing.idle_frame_count + new_idle_frame_count,
    })
}
