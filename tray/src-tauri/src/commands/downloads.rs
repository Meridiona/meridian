//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Live count of Meridian downloads, for the setup wizard's Welcome screen —
//! a warm, numbered "you're not alone" moment. External HTTP (not a DB
//! read), so per the porting playbook this lives tray-side rather than in
//! `meridian-core`.
//!
//! # What this is
//! Sums `download_count` across every asset on the latest GitHub release
//! (the same release the DMG/Windows installer are themselves published to —
//! see `release.yml` and the website's `/dl` redirect) via the public GitHub
//! Releases API. No API key and no third-party mailing-list dependency:
//! GitHub already counts every asset download for us, for free.
//!
//! # Who calls this
//! [`get_download_count`] → `ui/app/setup/page.tsx`'s `SetupWizard`, which
//! passes the resolved count down to `steps.tsx`'s `Welcome` component.
//!
//! # Related
//! - [`crate::commands::version::get_version`] — the sibling GitHub-releases
//!   check this borrows its process-wide-cache shape from. Hits the same
//!   `releases/latest` endpoint, but cached and called independently — this
//!   isn't worth sharing state with that check over, since the two read
//!   different fields for different screens.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::Instrument;

const GITHUB_LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/Meridiona/meridian/releases/latest";
/// No need to re-check more than once every few minutes — the number only
/// has to be roughly current for a welcome-screen decoration.
const CACHE_TTL: Duration = Duration::from_secs(300);

/// Process-wide cache of the last successful count. `None` until the first
/// successful fetch.
static CACHE: Mutex<Option<(u64, Instant)>> = Mutex::new(None);

#[derive(Deserialize)]
struct ReleaseAsset {
    download_count: u64,
}

#[derive(Deserialize)]
struct ReleaseResponse {
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

/// `{ count }` for the Welcome screen — `None` when GitHub is unreachable
/// and no prior cached value exists, in which case the UI simply omits the
/// line rather than showing a placeholder.
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
            .get(GITHUB_LATEST_RELEASE_URL)
            // GitHub's API rejects requests without a User-Agent (403).
            .header(reqwest::header::USER_AGENT, "meridian-tray")
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("github releases api {}", resp.status()));
        }
        let body: ReleaseResponse = resp.json().await.map_err(|e| e.to_string())?;
        // Sum across every asset (macOS DMG + Windows installer) so the
        // number reflects every platform, not just whichever shipped first.
        Ok(body.assets.iter().map(|a| a.download_count).sum())
    }
    .instrument(tracing::debug_span!("downloads.fetch.github_releases"))
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

/// The Welcome-screen data: how many times Meridian's release assets have
/// been downloaded so far, for the "you're the Nth person to bring Meridian
/// into their day" line.
#[tauri::command]
#[tracing::instrument]
pub async fn get_download_count() -> DownloadCount {
    DownloadCount {
        count: fetch_count().await,
    }
}
