//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The user's capture ignore list, enforced at the frame boundary.
//!
//! # What this is
//! A small, pure matcher over two user-configured lists (`settings.json`'s
//! `ignored_apps` / `ignored_urls`): apps matched by exact case-insensitive
//! `app_name`, websites matched by the focused tab's `browser_url` HOST at
//! domain granularity (a listed domain also matches its subdomains). When a
//! captured frame — a11y OR OCR — matches, the frame consumer drops it BEFORE it
//! is written to `capture_frames`, so nothing about that app/site is ever
//! persisted and it never appears on the timeline. UI events for an ignored app
//! are dropped the same way.
//!
//! **Going forward only.** This gate runs on the live capture stream; it does not
//! touch already-captured history (the product decision — see the field docs on
//! [`meridian_core::settings::RuntimeSettings`]).
//!
//! # Why here, not in the fork walker
//! The forked `screenpipe-a11y` [`TreeWalkerConfig`] already exposes
//! `ignored_windows`/`ignored_urls`, but it is built ONCE at engine start and
//! covers only the a11y path (the OCR fallback carries its own `WindowFilters`).
//! Enforcing at the single frame consumer instead gives one choke point over
//! BOTH capture paths and lets a Settings toggle take effect live (the consumer
//! reads a shared handle), with no capture restart. The privacy result is the
//! same: an ignored frame is never persisted.
//!
//! # Who uses this
//! Held as `AppState::capture_ignore` (a shared, non-feature-gated handle,
//! mirroring `capture_paused`). Seeded from settings in `crate::start_capture`,
//! read by the frame + UI-event consumers there, and refreshed live by the
//! `update_settings` command on save.
//!
//! [`TreeWalkerConfig`]: screenpipe_a11y::tree::TreeWalkerConfig

/// Case-insensitive, normalised ignore lists. Construct via [`CaptureIgnore::new`]
/// so entries are trimmed/lowercased once and empties dropped — matching is then
/// a cheap comparison per frame.
#[derive(Debug, Clone, Default)]
pub struct CaptureIgnore {
    /// Lowercased, trimmed app names to drop (exact match on `app_name`).
    apps: Vec<String>,
    /// Lowercased, trimmed domains to drop (host or any subdomain of it).
    domains: Vec<String>,
}

impl CaptureIgnore {
    /// Build from the raw settings lists. Each entry is trimmed and lowercased;
    /// blank entries are discarded. A domain entry is reduced to its bare host
    /// so a user who pasted a full URL ("https://youtube.com/feed") still gets a
    /// domain match.
    pub fn new(apps: &[String], urls: &[String]) -> Self {
        let apps = apps
            .iter()
            .map(|a| a.trim().to_lowercase())
            .filter(|a| !a.is_empty())
            .collect();
        let domains = urls
            .iter()
            .filter_map(|u| {
                let lowered = u.to_lowercase();
                let host = url_host(&lowered).trim().trim_start_matches("www.");
                (!host.is_empty()).then(|| host.to_string())
            })
            .collect();
        Self { apps, domains }
    }

    /// True when this app should never be captured (exact, case-insensitive).
    /// Used for UI events, which have an app but no URL.
    pub fn should_drop_app(&self, app_name: Option<&str>) -> bool {
        match app_name {
            Some(a) => {
                let a = a.trim().to_lowercase();
                self.apps.contains(&a)
            }
            None => false,
        }
    }

    /// True when a captured frame should be dropped: either its app is ignored,
    /// or (for a browser frame) its URL's host is an ignored domain.
    pub fn should_drop_frame(&self, app_name: Option<&str>, browser_url: Option<&str>) -> bool {
        if self.should_drop_app(app_name) {
            return true;
        }
        match browser_url {
            Some(url) if !self.domains.is_empty() => {
                let lowered = url.to_lowercase();
                let host = url_host(&lowered);
                !host.is_empty() && self.domains.iter().any(|d| host_matches(host, d))
            }
            _ => false,
        }
    }
}

/// Extract the lowercase host from a URL string: drop the scheme, then take up
/// to the first `/`, `?` or `#`, then strip any `userinfo@` and `:port`. Returns
/// `""` for input with no recognisable host. Deliberately dependency-free — a
/// full URL parser is overkill for host-only domain matching.
fn url_host(url: &str) -> &str {
    let after_scheme = match url.split_once("://") {
        Some((_, rest)) => rest,
        None => url,
    };
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    // Strip userinfo (user:pass@host) then the port.
    let host = match authority.rsplit_once('@') {
        Some((_, h)) => h,
        None => authority,
    };
    match host.split_once(':') {
        Some((h, _)) => h,
        None => host,
    }
}

/// A host matches an ignored domain when it IS that domain or a subdomain of it —
/// "youtube.com" matches "youtube.com" and "m.youtube.com", but never
/// "notyoutube.com" (the boundary is a `.`).
fn host_matches(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ig(apps: &[&str], urls: &[&str]) -> CaptureIgnore {
        CaptureIgnore::new(
            &apps.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            &urls.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
    }

    #[test]
    fn app_match_is_exact_and_case_insensitive() {
        let f = ig(&["Messages"], &[]);
        assert!(f.should_drop_frame(Some("Messages"), None));
        assert!(f.should_drop_frame(Some("messages"), None));
        assert!(f.should_drop_frame(Some("  MESSAGES  "), None));
        // Exact, not substring — a different app that merely contains it stays.
        assert!(!f.should_drop_frame(Some("Messages Pro"), None));
        assert!(!f.should_drop_frame(Some("Slack"), None));
        assert!(!f.should_drop_frame(None, None));
    }

    #[test]
    fn domain_match_covers_subdomains_on_a_dot_boundary() {
        let f = ig(&[], &["youtube.com"]);
        assert!(f.should_drop_frame(
            Some("Google Chrome"),
            Some("https://www.youtube.com/watch?v=x")
        ));
        assert!(f.should_drop_frame(Some("Safari"), Some("https://m.youtube.com/")));
        assert!(f.should_drop_frame(Some("Safari"), Some("http://youtube.com")));
        // Not a subdomain — the leading label must end on a dot.
        assert!(!f.should_drop_frame(Some("Safari"), Some("https://notyoutube.com/")));
        // A different site entirely.
        assert!(!f.should_drop_frame(Some("Safari"), Some("https://github.com/")));
        // No URL, app not ignored → kept.
        assert!(!f.should_drop_frame(Some("Safari"), None));
    }

    #[test]
    fn a_pasted_full_url_or_www_prefix_normalises_to_the_domain() {
        let f = ig(&[], &["https://www.reddit.com/r/rust"]);
        assert!(f.should_drop_frame(Some("Arc"), Some("https://old.reddit.com/r/rust")));
        assert!(f.should_drop_frame(Some("Arc"), Some("https://reddit.com/")));
    }

    #[test]
    fn url_host_parsing_handles_ports_userinfo_and_bare_hosts() {
        assert_eq!(
            url_host("https://user:pass@example.com:8443/path?q=1"),
            "example.com"
        );
        assert_eq!(url_host("http://localhost:3000/today"), "localhost");
        assert_eq!(url_host("example.com/foo"), "example.com");
        // Scheme-less, colon-bearing junk degrades to a non-matching token
        // ("about") — never a real domain, so it can't cause a false drop.
        assert_eq!(url_host("about:blank"), "about");
    }

    #[test]
    fn empty_lists_never_drop() {
        let f = CaptureIgnore::default();
        assert!(!f.should_drop_frame(Some("Anything"), Some("https://youtube.com")));
        assert!(!f.should_drop_app(Some("Anything")));
    }

    #[test]
    fn blank_entries_are_discarded() {
        let f = ig(&["", "   "], &["", "  "]);
        assert!(!f.should_drop_frame(Some(""), Some("https://youtube.com")));
        assert!(!f.should_drop_app(Some("")));
    }
}
