// meridian — normalises screenpipe activity into structured app sessions
//
// UI *readiness* — not just liveness. `ui service` (a launchd PID) and `ui
// built` (`.next` exists) both pass for a running-but-broken dashboard: a stale
// build serves HTML whose _next/static assets 404/500, blanking the page. So we
// functionally probe what is actually served (the ground truth). We also flag a
// latent serve-mode smell — a standalone bundle being served by `next start`
// instead of `node server.js` — as a *warning*, not a failure: `next start`
// still serves `.next/static`, so it is not what blanks the page.

use crate::config::Config;
use crate::health::platform::repo_root;
use crate::health::Check;
use std::path::PathBuf;
use std::time::Duration;

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn ui_port() -> u16 {
    std::env::var("MERIDIAN_UI_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3939)
}

pub async fn checks(_cfg: &Config) -> Vec<Check> {
    let mut out = vec![serve_mode_check()];
    out.extend(serve_health(ui_port()).await);
    out
}

/// Serve-mode smell (warning, not failure). Detect the real build *artifact* —
/// a standalone server bundle at `.next/standalone/server.js` — not the config
/// source text: `next.config.ts` may mention "standalone" in a comment or a
/// build-gated conditional (`MERIDIAN_BUILD_STANDALONE ? 'standalone' : …`)
/// without the build actually emitting one. If a standalone bundle exists but
/// the service runs `next start`, the bundle is simply unused — `next start`
/// still serves `.next/static` correctly, so the functional `ui assets` probe,
/// not this check, is the ground truth for an actually-broken page.
fn serve_mode_check() -> Check {
    let Some(root) = repo_root() else {
        return Check::info("ui serve mode", "ui", "repo root not found");
    };
    let standalone_built = root.join("ui/.next/standalone/server.js").exists();
    if !standalone_built {
        return Check::ok(
            "ui serve mode",
            "ui",
            "normal build, served by `next start`",
        );
    }
    let plist = home().join("Library/LaunchAgents/com.meridiona.ui.plist");
    let runs_next_start = std::fs::read_to_string(&plist)
        .map(|s| s.contains("next start") || s.contains("<string>start</string>"))
        .unwrap_or(false);
    if runs_next_start {
        Check::warn(
            "ui serve mode",
            "ui",
            "standalone bundle built but the service runs `next start` (the bundle is unused)",
        )
        .with_remedy(
            "serve via `node .next/standalone/server.js`, or rebuild without output:'standalone'",
        )
    } else {
        Check::ok(
            "ui serve mode",
            "ui",
            "standalone bundle, served via `node server.js`",
        )
    }
}

/// Functional probe: GET `/`, then fetch a `_next/static` asset the HTML
/// references. The asset fetch is the real signal — a stale/broken build returns
/// 200 on `/` but 404/500 on its chunks, which is what blanks the dashboard.
async fn serve_health(port: u16) -> Vec<Check> {
    let base = format!("http://localhost:{port}");
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return vec![Check::warn(
                "ui serving",
                "ui",
                format!("http client error ({e})"),
            )]
        }
    };

    let html = match client.get(&base).send().await {
        Err(_) => {
            return vec![
                Check::warn("ui serving", "ui", format!("not reachable at {base}"))
                    .with_remedy("meridian start"),
            ]
        }
        Ok(resp) if !resp.status().is_success() => {
            return vec![Check::critical(
                "ui serving",
                "ui",
                format!("/ returned HTTP {}", resp.status().as_u16()),
            )
            .with_remedy("rebuild + restart the UI (cd ui && npm run build)")]
        }
        Ok(resp) => resp.text().await.unwrap_or_default(),
    };

    vec![
        Check::ok("ui serving", "ui", "/ → 200"),
        asset_check(&client, &base, &html).await,
    ]
}

async fn asset_check(client: &reqwest::Client, base: &str, html: &str) -> Check {
    let Some(asset) = first_asset(html) else {
        return Check::info("ui assets", "ui", "no static asset referenced (unusual)");
    };
    match client.get(format!("{base}{asset}")).send().await {
        Ok(resp) if resp.status().is_success() => {
            Check::ok("ui assets", "ui", "static assets load")
        }
        Ok(resp) => Check::critical(
            "ui assets",
            "ui",
            format!(
                "{} → HTTP {} — stale/broken build",
                short(&asset),
                resp.status().as_u16()
            ),
        )
        .with_remedy("rebuild + restart the UI; check output:'standalone' vs `next start`"),
        Err(_) => Check::warn("ui assets", "ui", "could not fetch a static asset"),
    }
}

/// First `/_next/static/...` asset URL in the HTML, preferring CSS then JS.
fn first_asset(html: &str) -> Option<String> {
    [".css", ".js"]
        .into_iter()
        .find_map(|ext| find_asset_with_ext(html, ext))
}

fn find_asset_with_ext(html: &str, ext: &str) -> Option<String> {
    const NEEDLE: &str = "/_next/static/";
    let mut from = 0;
    while let Some(rel) = html[from..].find(NEEDLE) {
        let i = from + rel;
        let rest = &html[i..];
        let end = rest.find(['"', '\'', '>', ' ']).unwrap_or(rest.len());
        let url = &rest[..end];
        if url.ends_with(ext) {
            return Some(url.to_string());
        }
        from = i + NEEDLE.len();
    }
    None
}

/// Tail of an asset path for compact display.
fn short(url: &str) -> String {
    url.rsplit('/').next().unwrap_or(url).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_css_asset_then_js() {
        let html = r#"<link rel="stylesheet" href="/_next/static/chunks/a1b2.css"/><script src="/_next/static/chunks/main.js"></script>"#;
        assert_eq!(
            first_asset(html).as_deref(),
            Some("/_next/static/chunks/a1b2.css")
        );
    }

    #[test]
    fn falls_back_to_js_when_no_css() {
        let html = r#"<script src="/_next/static/chunks/main-xyz.js"></script>"#;
        assert_eq!(
            first_asset(html).as_deref(),
            Some("/_next/static/chunks/main-xyz.js")
        );
    }

    #[test]
    fn no_asset_when_absent() {
        assert!(first_asset("<html><body>hi</body></html>").is_none());
    }

    #[test]
    fn short_takes_basename() {
        assert_eq!(short("/_next/static/chunks/a.css"), "a.css");
    }
}
