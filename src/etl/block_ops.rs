//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

use anyhow::{Context, Result};
use sqlx::SqlitePool;
use tracing::{debug, info, warn};

use crate::db::meridian::{
    close_active_session_with, get_active_session, upsert_active_session, write_session_traceparent,
};
use crate::db::screenpipe::{
    fetch_frame_text_page, get_last_ui_event_for_app, FRAME_TEXT_PAGE_SIZE,
};
use crate::etl::extractor::extract_block_context;
use crate::etl::text_filter::ChromeFreqAccumulator;
use crate::etl::text_merge::SessionTextBuilder;

use super::session_builder::{build_active_session, merge_into_active};

/// Parses two RFC3339 timestamps and returns the difference in whole seconds,
/// or `None` if either timestamp fails to parse.
pub(super) fn timestamp_gap_secs(earlier: &str, later: &str) -> Option<i64> {
    let t0 = chrono::DateTime::parse_from_rfc3339(earlier).ok()?;
    let t1 = chrono::DateTime::parse_from_rfc3339(later).ok()?;
    Some((t1 - t0).num_seconds())
}

/// Rebuilds `session_text` for the frame range `min_frame_id..=max_frame_id` by
/// reading frames from the DB in `FRAME_TEXT_PAGE_SIZE`-sized pages instead of
/// materializing the whole range in one `Vec<FrameText>`.
///
/// Two passes over the paginated read are required: chrome-line detection
/// (`ChromeFreqAccumulator`) needs a global view of line frequencies across the
/// *entire* range before any single frame can be classified (see
/// `crate::etl::text_filter::build_chrome_set`), so pass 1 accumulates
/// frequencies page-by-page and pass 2 re-pages the same range to fold each
/// frame into the session text using the now-final chrome set.
///
/// This is a memory-shape change only: for the same frame range, the returned
/// string is functionally identical to
/// `build_session_text(&get_frame_full_texts(pool, min_frame_id, max_frame_id).await?)` —
/// no data is truncated or dropped, only held in bounded pages rather than all
/// at once.
pub(super) async fn rebuild_session_text_paginated(
    pool: &SqlitePool,
    min_frame_id: i64,
    max_frame_id: i64,
) -> Result<String> {
    // Pass 1: accumulate chrome-line frequencies across every page.
    let mut chrome_acc = ChromeFreqAccumulator::new();
    let mut after_id = min_frame_id - 1;
    loop {
        let page = fetch_frame_text_page(pool, after_id, max_frame_id, FRAME_TEXT_PAGE_SIZE)
            .await
            .context("paginated frame-text read (chrome frequency pass)")?;
        if page.is_empty() {
            break;
        }
        after_id = page.last().map(|f| f.frame_id).unwrap_or(after_id);
        chrome_acc.feed(&page);
    }
    let chrome = chrome_acc.finish();

    // Pass 2: re-page the same range and fold each frame into the session text.
    let mut builder = SessionTextBuilder::new();
    let mut after_id = min_frame_id - 1;
    loop {
        let page = fetch_frame_text_page(pool, after_id, max_frame_id, FRAME_TEXT_PAGE_SIZE)
            .await
            .context("paginated frame-text read (session-text build pass)")?;
        if page.is_empty() {
            break;
        }
        after_id = page.last().map(|f| f.frame_id).unwrap_or(after_id);
        builder.feed(&page, &chrome);
    }
    Ok(builder.finish())
}

/// Groups the positional fields shared by `close_block` / `upsert_open_block`
/// so both stay under clippy's 7-argument limit.
pub(super) struct BlockBounds<'a> {
    pub(super) app: &'a str,
    pub(super) started_at: &'a str,
    pub(super) ended_at: &'a str,
    pub(super) next_frame_ts: Option<&'a str>,
    pub(super) min_frame_id: i64,
    pub(super) max_frame_id: i64,
    pub(super) frame_count: i64,
    pub(super) idle_frame_count: i64,
}

/// Closes a completed block into `app_sessions`.
///
/// Applies Option C (ui_event refines `ended_at`) and Option D
/// (inter-frame gap recovery) before writing.
#[tracing::instrument(
    skip_all,
    fields(
        app_name = %b.app,
        frame_count = b.frame_count,
        idle_frame_count = b.idle_frame_count,
        run_id,
        duration_s = tracing::field::Empty,
        session_id = tracing::field::Empty,
    )
)]
pub(super) async fn close_block(
    meridian: &SqlitePool,
    run_id: i64,
    b: &BlockBounds<'_>,
) -> Result<(i64, i64)> {
    let mut ctx = extract_block_context(
        meridian,
        b.app,
        b.started_at,
        b.ended_at,
        b.min_frame_id,
        b.max_frame_id,
        b.frame_count,
    )
    .await?;

    // Option C: use last ui_event as ended_at if it's more recent than the last frame.
    // Only fires for app-switch closes (next_frame_ts is set); never for gap-closes.
    let option_c_fired = if let Some(next_ts) = b.next_frame_ts {
        debug!(
            app = b.app,
            last_frame_ts = b.ended_at,
            next_frame_ts = next_ts,
            search_window = format!("{}..{}", b.started_at, next_ts),
            "Option C: searching for ui_events (click/key/text)"
        );
        match get_last_ui_event_for_app(meridian, b.app, b.started_at, next_ts).await {
            Ok(Some(ui_ts)) => {
                if ui_ts.as_str() > b.ended_at {
                    debug!(
                        app = b.app,
                        last_frame_ts = b.ended_at,
                        ui_event_ts = &ui_ts,
                        gap_recovered_s = timestamp_gap_secs(b.ended_at, &ui_ts),
                        "Option C FIRED: ended_at refined via ui_event"
                    );
                    ctx.ended_at = ui_ts;
                    true
                } else {
                    debug!(
                        app = b.app,
                        last_frame_ts = b.ended_at,
                        ui_event_ts = &ui_ts,
                        reason = "ui_event is not more recent than last frame",
                        "Option C: no-op"
                    );
                    false
                }
            }
            Ok(None) => {
                debug!(
                    app = b.app,
                    search_window = format!("{}..{}", b.started_at, next_ts),
                    reason = "no ui_events found in window",
                    "Option C: no-op"
                );
                false
            }
            Err(e) => {
                debug!(
                    app = b.app,
                    error = %e,
                    reason = "ui_event query failed",
                    "Option C: no-op"
                );
                false
            }
        }
    } else {
        debug!(
            app = b.app,
            reason = "gap-close (no next_frame_ts)",
            "Option C: skipped"
        );
        false
    };

    // Option D: advance ended_at to next_frame_ts if it is later.
    // Recovers the inter-frame gap between the last captured frame and the
    // first frame of the next app. Never fires for gap-closes.
    if !option_c_fired {
        if let Some(next_ts) = b.next_frame_ts {
            if next_ts > ctx.ended_at.as_str() {
                let gap_s = timestamp_gap_secs(ctx.ended_at.as_str(), next_ts);
                debug!(
                    app = b.app,
                    current_ended_at = ctx.ended_at.as_str(),
                    next_frame_ts = next_ts,
                    gap_recovered_s = gap_s,
                    "Option D FIRED: ended_at advanced to next_frame_ts (inter-frame gap recovery)"
                );
                ctx.ended_at = next_ts.to_string();
            } else {
                debug!(
                    app = b.app,
                    current_ended_at = ctx.ended_at.as_str(),
                    next_frame_ts = next_ts,
                    reason = "next_frame_ts is not later than current ended_at",
                    "Option D: no-op"
                );
            }
        } else {
            debug!(
                app = b.app,
                reason = "gap-close (no next_frame_ts)",
                "Option D: skipped"
            );
        }
    } else {
        debug!(
            app = b.app,
            reason = "Option C already fired (ui_event took precedence)",
            "Option D: skipped"
        );
    }

    let existing = get_active_session(meridian).await?;

    let result: (i64, i64) = match existing {
        Some(ref active) if active.app_name == ctx.app_name => {
            debug!(app = ctx.app_name, "merging and closing continuation block");
            // Re-fetch ALL frames for the session's full lifetime and rebuild session_text
            // from scratch. The additive merge path can leave chrome lines from early
            // sub-threshold batches (< CHROME_FREQ_THRESHOLD frames) permanently in the
            // stored text; a full rebuild sees the complete frame set so the chrome
            // pre-pass correctly identifies and removes them.
            //
            // Paginated rather than a single fetch_all: a session that stays in one app
            // for hours (an IDE, one browser tab) never closes early, so by the time it
            // finally does, `active.min_frame_id..b.max_frame_id` can span many thousands
            // of frames — a single `get_frame_full_texts` there would spike RSS by the
            // whole session's OCR text at once. See `rebuild_session_text_paginated`.
            let rebuilt_text =
                rebuild_session_text_paginated(meridian, active.min_frame_id, b.max_frame_id)
                    .await
                    .context("re-fetch frames for session_text rebuild on close")?;
            let merged = merge_into_active(active, &ctx, b.idle_frame_count, Some(rebuilt_text))?;
            let new_id = close_active_session_with(meridian, &merged, run_id).await?;
            info!(
                app_name = ctx.app_name,
                session_id = new_id,
                "session closed (merged continuation)"
            );
            (new_id, 1)
        }

        Some(ref active) => {
            warn!(
                stale_app = active.app_name,
                new_app = ctx.app_name,
                "stale active_session — closing stale first"
            );
            let stale_id = close_active_session_with(meridian, active, run_id).await?;
            if let Some(tp) = crate::observability::current_traceparent() {
                write_session_traceparent(meridian, stale_id, &tp)
                    .await
                    .context("write traceparent (stale session)")?;
            }
            let new_session = build_active_session(&ctx, b.idle_frame_count)?;
            let new_id = close_active_session_with(meridian, &new_session, run_id).await?;
            info!(
                app_name = ctx.app_name,
                session_id = new_id,
                "session closed (fresh, after evicting stale)"
            );
            (new_id, 2)
        }

        None => {
            let new_session = build_active_session(&ctx, b.idle_frame_count)?;
            let new_id = close_active_session_with(meridian, &new_session, run_id).await?;
            info!(app_name = b.app, session_id = new_id, "session closed");
            (new_id, 1)
        }
    };

    let (new_session_id, closed_count) = result;

    if let (Ok(started), Ok(ended)) = (
        chrono::DateTime::parse_from_rfc3339(&ctx.started_at),
        chrono::DateTime::parse_from_rfc3339(&ctx.ended_at),
    ) {
        let duration_s = (ended - started).num_seconds().max(0);
        tracing::Span::current().record("duration_s", duration_s);
    }
    tracing::Span::current().record("session_id", new_session_id);

    if let Some(tp) = crate::observability::current_traceparent() {
        write_session_traceparent(meridian, new_session_id, &tp)
            .await
            .context("write traceparent")?;
    }

    Ok((new_session_id, closed_count))
}

/// Upserts the still-open block into `active_session` at the end of an ETL pass.
#[tracing::instrument(
    skip_all,
    fields(
        app_name = %b.app,
        frame_count = b.frame_count,
        run_id,
    )
)]
pub(super) async fn upsert_open_block(
    meridian: &SqlitePool,
    run_id: i64,
    b: &BlockBounds<'_>,
) -> Result<i64> {
    let ctx = extract_block_context(
        meridian,
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
        Some(ref active) if active.app_name == ctx.app_name => {
            debug!(
                app = ctx.app_name,
                "merging new frames into existing active_session"
            );
            // Open block — additive merge (no rebuild needed; chrome is re-evaluated on close).
            merge_into_active(active, &ctx, b.idle_frame_count, None)?
        }

        Some(ref active) => {
            warn!(
                stale_app = active.app_name,
                new_app = ctx.app_name,
                "stale active_session while upserting open block"
            );
            close_active_session_with(meridian, active, run_id).await?;
            build_active_session(&ctx, b.idle_frame_count)?
        }

        None => build_active_session(&ctx, b.idle_frame_count)?,
    };

    upsert_active_session(meridian, &session).await?;
    debug!(
        app = b.app,
        max_frame_id = b.max_frame_id,
        "active_session upserted"
    );
    Ok(0)
}
