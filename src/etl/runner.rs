// meridian — normalises screenpipe activity into structured app sessions

use anyhow::Result;
use sqlx::SqlitePool;
use tracing::{debug, info, warn};

use crate::db::meridian::{
    close_active_session, complete_etl_run, get_active_session, get_cursor, insert_etl_run,
    insert_gap, update_cursor,
};
use crate::db::screenpipe::{count_frames_in_window, get_frames_since};

use super::block_ops::{close_block, timestamp_gap_secs, upsert_open_block, BlockBounds};
use super::session_builder::{is_browser, url_domain};

const BATCH_SIZE: i64 = 100;
const GAP_THRESHOLD_SECS: i64 = 300;

/// Runs one full ETL cycle:
///   1. Reads frames from screenpipe in batches of 100.
///   2. Groups consecutive frames by `app_name` (strict: every change = new session).
///   3. Closes finished sessions into `app_sessions` and keeps the still-open
///      block in `active_session`.
///   4. Advances the cursor and writes the ETL audit row.
#[tracing::instrument(
    skip_all,
    fields(
        run_id = tracing::field::Empty,
        from_frame_id = tracing::field::Empty,
        to_frame_id = tracing::field::Empty,
        sessions_closed = tracing::field::Empty,
    )
)]
pub async fn run_etl(screenpipe: &SqlitePool, meridian: &SqlitePool) -> Result<()> {
    let cursor = get_cursor(meridian).await?;
    let mut last_processed_id = cursor.last_frame_id;
    let run_start_cursor = last_processed_id;

    tracing::Span::current().record("from_frame_id", run_start_cursor);
    info!(from_frame_id = last_processed_id, "ETL run starting");

    let first_batch = get_frames_since(screenpipe, last_processed_id, BATCH_SIZE).await?;
    if first_batch.is_empty() {
        info!("no new frames — nothing to do");
        return Ok(());
    }

    let approx_to_frame_id = first_batch
        .last()
        .map(|f| f.id)
        .unwrap_or(last_processed_id);

    let run_id = insert_etl_run(meridian, run_start_cursor, approx_to_frame_id).await?;
    tracing::Span::current().record("run_id", run_id);
    tracing::Span::current().record("to_frame_id", approx_to_frame_id);
    info!(run_id, "ETL run row inserted");

    let mut sessions_closed: i64 = 0;

    // TODO: Extract gap classification logic into helper. Currently duplicated at:
    // 1. Cross-run gap check (below, ~line 75-89)
    // 2. Intra-batch gap check (line ~175-179)
    // Both compute: if idle * 2 >= total → "user_idle" else "system_sleep"
    // Helper signature: async fn classify_and_record_gap(screenpipe, meridian, from_ts, to_ts, gap_secs, run_id) -> Result<String>

    // Cross-run gap check: if there's a stale active_session from the previous ETL run,
    // compare its last_seen_at against the first frame we're about to process. A gap
    // > GAP_THRESHOLD_SECS means the machine was asleep or idle between runs.
    if let Ok(Some(stale)) = get_active_session(meridian).await {
        if let Some(first_frame) = first_batch.first() {
            if let Some(gap_secs) = timestamp_gap_secs(&stale.last_seen_at, &first_frame.timestamp)
            {
                if gap_secs < 0 {
                    warn!(
                        gap_secs,
                        last_seen_at = stale.last_seen_at,
                        first_frame_ts = first_frame.timestamp,
                        "cross-run gap is negative — clock drift or NTP sync, skipping"
                    );
                } else if gap_secs > GAP_THRESHOLD_SECS {
                    let (total, idle) = count_frames_in_window(
                        screenpipe,
                        &stale.last_seen_at,
                        &first_frame.timestamp,
                    )
                    .await
                    .unwrap_or_else(|e| {
                        warn!(error = %e, "count_frames_in_window failed — classifying gap as system_sleep");
                        (0, 0)
                    });
                    let kind = if total > 0 && idle > 0 && idle * 2 >= total {
                        "user_idle"
                    } else {
                        "system_sleep"
                    };
                    debug!(
                        gap_secs,
                        gap_kind = kind,
                        app_name = stale.app_name,
                        frame_id = first_frame.id,
                        gap_frame_ids = format!("{}..{}", stale.min_frame_id, first_frame.id),
                        "cross-run gap detected — inserting gap record and closing stale active_session"
                    );
                    insert_gap(
                        meridian,
                        &stale.last_seen_at,
                        &first_frame.timestamp,
                        gap_secs,
                        kind,
                        run_id,
                    )
                    .await?;
                    close_active_session(meridian, run_id).await?;
                    info!(
                        gap_secs,
                        gap_kind = kind,
                        app_name = stale.app_name,
                        "cross-run gap detected — closed stale active_session"
                    );
                }
            }
        }
    }

    let mut current_app: Option<String> = None;
    let mut current_window: Option<String> = None;
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
                let window = frame
                    .browser_url
                    .as_deref()
                    .map(url_domain)
                    .filter(|d| !d.is_empty())
                    .or_else(|| frame.window_name.as_deref().filter(|w| !w.is_empty()))
                    .unwrap_or("");

                if app.is_empty() {
                    if current_app.is_some() {
                        block_frame_count += 1;
                        block_last_ts = frame.timestamp.clone();
                        block_last_frame_id = frame.id;
                    }
                    debug!(frame_id = frame.id, "skipping frame with empty app_name");
                    continue;
                }

                // Gap detection: check for a long pause between the last frame and this one.
                // Runs BEFORE the app-switch state machine so it fires regardless of app change.
                if current_app.is_some() {
                    if let Some(gap) = timestamp_gap_secs(&block_last_ts, &frame.timestamp) {
                        if gap < 0 {
                            warn!(
                                gap_secs = gap,
                                block_last_ts,
                                frame_ts = frame.timestamp,
                                "intra-batch gap is negative — clock drift or NTP sync, skipping"
                            );
                        } else if gap > GAP_THRESHOLD_SECS {
                            let (total, idle) = count_frames_in_window(
                                screenpipe,
                                &block_last_ts,
                                &frame.timestamp,
                            )
                            .await
                            .unwrap_or_else(|e| {
                                warn!(error = %e, "count_frames_in_window failed — classifying gap as system_sleep");
                                (0, 0)
                            });

                            let kind = if total > 0 && idle > 0 && (idle * 2 >= total) {
                                "user_idle"
                            } else {
                                "system_sleep"
                            };
                            debug!(
                                gap_secs = gap,
                                gap_kind = kind,
                                app_name = current_app.as_deref().unwrap_or(""),
                                frame_id = frame.id,
                                gap_frame_ids = format!("{}..{}", block_last_frame_id, frame.id),
                                "intra-batch gap detected — inserting gap record and closing current block"
                            );

                            insert_gap(
                                meridian,
                                &block_last_ts,
                                &frame.timestamp,
                                gap,
                                kind,
                                run_id,
                            )
                            .await?;

                            debug!(gap_secs = gap, gap_kind = kind, from = block_last_ts, to = frame.timestamp, "gap recorded");

                            // Close the pre-gap block so its duration_s ends at block_last_ts,
                            // preventing gap time from inflating the session's focus duration.
                            let closing_app = current_app.take().unwrap();
                            current_window = None;
                            sessions_closed += close_block(
                                screenpipe,
                                meridian,
                                run_id,
                                &BlockBounds {
                                    app: &closing_app,
                                    started_at: &block_start_ts,
                                    ended_at: &block_last_ts,
                                    next_frame_ts: None,
                                    min_frame_id: block_start_frame_id,
                                    max_frame_id: block_last_frame_id,
                                    frame_count: block_frame_count,
                                    idle_frame_count: block_idle_frame_count,
                                },
                            )
                            .await?;
                        }
                    }
                }

                // For browsers, a window/domain change triggers a session split.
                let browser_window_changed = is_browser(app)
                    && !window.is_empty()
                    && current_window.as_deref() != Some(window);

                match current_app.as_deref() {
                    None => {
                        debug!(frame_id = frame.id, app, "first frame — starting block");
                        current_app = Some(app.to_owned());
                        current_window = Some(window.to_owned());
                        block_start_frame_id = frame.id;
                        block_start_ts = frame.timestamp.clone();
                        block_frame_count = 1;
                        block_idle_frame_count =
                            if frame.capture_trigger.as_deref() == Some("idle") { 1 } else { 0 };
                        block_last_ts = frame.timestamp.clone();
                        block_last_frame_id = frame.id;
                    }

                    Some(cur) if cur == app && !browser_window_changed => {
                        block_frame_count += 1;
                        if frame.capture_trigger.as_deref() == Some("idle") {
                            block_idle_frame_count += 1;
                        }
                        block_last_ts = frame.timestamp.clone();
                        block_last_frame_id = frame.id;
                    }

                    Some(cur) => {
                        let old_app = cur.to_owned();
                        if browser_window_changed {
                            debug!(
                                app, old_window = current_window.as_deref(), new_window = window,
                                frame_id = frame.id, "browser window changed — closing block"
                            );
                        } else {
                            debug!(old_app = old_app, new_app = app, frame_id = frame.id, "app changed — closing block");
                        }

                        sessions_closed += close_block(
                            screenpipe,
                            meridian,
                            run_id,
                            &BlockBounds {
                                app: &old_app,
                                started_at: &block_start_ts,
                                ended_at: &block_last_ts,
                                next_frame_ts: Some(&frame.timestamp),
                                min_frame_id: block_start_frame_id,
                                max_frame_id: block_last_frame_id,
                                frame_count: block_frame_count,
                                idle_frame_count: block_idle_frame_count,
                            },
                        )
                        .await?;

                        current_app = Some(app.to_owned());
                        current_window = Some(window.to_owned());
                        block_start_frame_id = frame.id;
                        block_start_ts = frame.timestamp.clone();
                        block_frame_count = 1;
                        block_idle_frame_count =
                            if frame.capture_trigger.as_deref() == Some("idle") { 1 } else { 0 };
                        block_last_ts = frame.timestamp.clone();
                        block_last_frame_id = frame.id;
                    }
                }
            }

            last_processed_id = batch.last().map(|f| f.id).unwrap_or(last_processed_id);

            let next = get_frames_since(screenpipe, last_processed_id, BATCH_SIZE).await?;
            if next.is_empty() {
                break;
            }
            batch = next;
        }

        // Upsert the still-open block into active_session.
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
                        next_frame_ts: None,
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

    if let Err(ref e) = result {
        warn!(error = %e, "ETL run failed — updating cursor to last successful frame");
        complete_etl_run(meridian, run_id, sessions_closed, Some(&e.to_string())).await?;
        update_cursor(meridian, last_processed_id, run_id).await?;
        return result;
    }

    update_cursor(meridian, last_processed_id, run_id).await?;
    complete_etl_run(meridian, run_id, sessions_closed, None).await?;

    tracing::Span::current().record("sessions_closed", sessions_closed);
    info!(
        run_id,
        sessions_closed,
        last_frame_id = last_processed_id,
        "ETL run complete"
    );

    Ok(())
}
