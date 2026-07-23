//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Live count of people who've downloaded Meridian, for the setup wizard's
//! Welcome screen — a warm, numbered "you're not alone" moment. External
//! HTTP (not a DB read), so per the porting playbook this lives tray-side
//! rather than in `meridian-core`.
//!
//! # What this is
//! Calls the meridiona-website Worker's `/api/downloads-count`, which counts
//! contacts in the Resend "download" audience — the list people join by
//! hitting the marketing site's `/download` page. The website holds the
//! Resend API key; the tray never sees it, only the resulting count.
//!
//! # Who calls this
//! [`get_download_count`] → `ui/app/setup/page.tsx`'s `SetupWizard`, which
//! passes the resolved count down to `steps.tsx`'s `Welcome` component.
//!
//! # Related
//! - `meridiona-website/worker.js`'s `/api/downloads-count` route (the source
//!   of truth this reads from).
//! - [`crate::commands::version::get_version`] — the sibling external-HTTP
//!   check this borrows its process-wide-cache shape from.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::Instrument;

const DOWNLOADS_COUNT_URL: &str = "https://meridiona.com/api/downloads-count";
/// Matches the website's own edge-cache TTL — no point re-checking sooner.
const CACHE_TTL: Duration = Duration::from_secs(300);

/// Process-wide cache of the last successful count. `None` until the first
/// successful fetch.
static CACHE: Mutex<Option<(u64, Instant)>> = Mutex::new(None);

#[derive(Deserialize)]
struct CountResponse {
    count: u64,
}

/// `{ count }` for the Welcome screen — `None` when the website is
/// unreachable and no prior cached value exists, in which case the UI simply
/// omits the line rather than showing a placeholder.
#[derive(Debug, Clone, Serialize)]
pub struct DownloadCount {
    pub count: Option<u64>,
}

/// Fetch the live count, honouring the process-wide cache. Never errors — a
/// network hiccup falls back to any cached value, else `None`; this is a
/// warm decoration, not something setup depends on.
async fn fetch_count() -> Option<u64> {
    if let Some((count, checked)) = CACHE.lock().unwrap().as_ref() {
        if checked.elapsed() < CACHE_TTL {
            tracing::debug!(count, "download count: cache hit");
            return Some(*count);
        }
    }

    let fetched: Result<u64, String> = async {
        let resp = reqwest::Client::new()
            .get(DOWNLOADS_COUNT_URL)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("downloads-count api {}", resp.status()));
        }
        resp.json::<CountResponse>()
            .await
            .map(|b| b.count)
            .map_err(|e| e.to_string())
    }
    .instrument(tracing::debug_span!("downloads.fetch.website"))
    .await;

    match fetched {
        Ok(count) => {
            *CACHE.lock().unwrap() = Some((count, Instant::now()));
            tracing::info!(count, "download count served");
            Some(count)
        }
        Err(e) => {
            tracing::warn!(error = %e, "download count unavailable");
            CACHE.lock().unwrap().as_ref().map(|(c, _)| *c)
        }
    }
}

/// The ported Welcome-screen data: how many people have downloaded Meridian
/// so far, for the "you're the Nth person to give Meridian a shot" line.
#[tauri::command]
#[tracing::instrument]
pub async fn get_download_count() -> DownloadCount {
    DownloadCount {
        count: fetch_count().await,
    }
}
