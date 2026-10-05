//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Reading the other windows that are visible on screen beside the one the user is typing in.
//!
//! A reply often depends on something on screen that is not in the focused window: a
//! document open beside a chat, a ticket beside an editor, an email being answered in a
//! second window. The compose pipeline gets that text as "other windows visible on screen".
//!
//! # What is and is not read
//! Only windows that are on screen right now, in the normal window layer, belonging to
//! other apps, and at most [`MAX_WINDOWS`] of them (front-most first). Never windows of
//! this app, system chrome, apps or sites on the user's capture ignore list, password
//! managers, or pages that look like banking, payments or sign-in. Skipping too much is the
//! safe error, so [`is_sensitive`] is deliberately broad. Reading is time-boxed per window
//! and runs in parallel so it adds at most a fraction of a second to a press.
//!
//! # Who calls this
//! [`super::reader`], once per press, unless the user turned it off with
//! `compose_other_windows` in `settings.json`.
//!
//! # Related
//! - [`super::walk`] - the traversal used to collect each window's text.
//! - [`crate::capture_ignore`] - the user's ignore list, applied through `is_ignored`.

use std::time::{Duration, Instant};

use core_foundation::base::CFType;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::window::{
    create_description_from_array, create_window_list, kCGNullWindowID,
    kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly,
};
use meridian::compose::types::WindowText;

use super::ax::Element;
use super::walk::{self, Limits};

/// Most other windows read per press.
pub const MAX_WINDOWS: usize = 3;
/// Time one window's walk may take.
const WINDOW_BUDGET: Duration = Duration::from_millis(400);
/// Most elements visited per window.
const WINDOW_NODES: usize = 600;
/// Most elements visited looking for a window's page address.
const URL_SEARCH_NODES: usize = 1500;

/// Window owners that are system furniture, never content.
const SYSTEM_OWNERS: &[&str] = &[
    "window server",
    "dock",
    "control center",
    "systemuiserver",
    "notification center",
    "spotlight",
    "loginwindow",
    "windowmanager",
    "textinputmenuagent",
    "screenshot",
];

/// A window as the window server describes it, before any filtering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawWindow {
    pub pid: i32,
    pub owner: String,
    pub title: String,
    pub layer: i32,
}

/// An on-screen window chosen to be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub pid: i32,
    pub app: String,
    pub title: String,
}

/// Choose which windows to read, from the on-screen list in front-to-back order.
///
/// One window per app (its front-most), normal layer only, never the focused app or this app,
/// never system furniture, and never a window with no title (helper panels and overlays).
pub fn pick(windows: &[RawWindow], front_pid: i32, own_pid: i32, max: usize) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    for w in windows {
        if out.len() >= max {
            break;
        }
        let owner = w.owner.to_lowercase();
        if w.layer != 0
            || w.pid == front_pid
            || w.pid == own_pid
            || w.title.trim().is_empty()
            || SYSTEM_OWNERS.contains(&owner.as_str())
            || out.iter().any(|c| c.pid == w.pid)
        {
            continue;
        }
        out.push(Candidate {
            pid: w.pid,
            app: w.owner.clone(),
            title: w.title.clone(),
        });
    }
    out
}

/// Apps that exist to hold secrets.
const SENSITIVE_APPS: &[&str] = &[
    "1password",
    "keychain",
    "bitwarden",
    "lastpass",
    "dashlane",
    "keeper",
    "passwords",
    "authenticator",
];
/// Page-address fragments for money, health and identity sites.
const SENSITIVE_URL_PARTS: &[&str] = &[
    "bank",
    "paypal.",
    "wise.com",
    "coinbase.",
    "binance.",
    "robinhood.",
    "schwab.",
    "fidelity.",
    "vanguard.",
    "chase.com",
    "wellsfargo.",
    "americanexpress.",
    "revolut.",
    "stripe.com",
    "razorpay.",
    "mychart",
    "patient",
    "accounts.google.com",
    "login.",
    "signin.",
    "auth.",
    "id.apple.com",
];
/// Window-title fragments for sign-in and payment pages.
const SENSITIVE_TITLE_PARTS: &[&str] = &[
    "sign in",
    "log in",
    "login",
    "password",
    "payment",
    "checkout",
    "bank",
    // Private browsing windows, whatever the browser calls them.
    "incognito",
    "private browsing",
    "inprivate",
    "private window",
];
/// Browsers: a window of one of these whose page address cannot be found is skipped, because
/// the address is what the sensitive-site and ignore-list checks run on.
const BROWSERS: &[&str] = &[
    "chrome",
    "safari",
    "firefox",
    "arc",
    "edge",
    "brave",
    "opera",
    "vivaldi",
    "chromium",
    "duckduckgo",
    "orion",
];

fn is_browser(app: &str) -> bool {
    let app = app.to_lowercase();
    BROWSERS.iter().any(|b| app.contains(b))
}

/// True when a window should not be read at all. Deliberately broad: wrongly skipping a window
/// costs a little context, wrongly reading one sends something private to a model.
pub fn is_sensitive(app: &str, title: &str, url: Option<&str>) -> bool {
    let (app, title) = (app.to_lowercase(), title.to_lowercase());
    if SENSITIVE_APPS.iter().any(|a| app.contains(a)) {
        return true;
    }
    if SENSITIVE_TITLE_PARTS.iter().any(|t| title.contains(t)) {
        return true;
    }
    url.map(str::to_lowercase)
        .is_some_and(|u| SENSITIVE_URL_PARTS.iter().any(|p| u.contains(p)))
}

fn string_of(
    dict: &core_foundation::dictionary::CFDictionary<CFString, CFType>,
    key: &str,
) -> Option<CFType> {
    dict.find(CFString::new(key)).map(|v| v.clone())
}

/// The on-screen windows, front to back.
fn on_screen_windows() -> Vec<RawWindow> {
    let option = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    let Some(ids) = create_window_list(option, kCGNullWindowID) else {
        return Vec::new();
    };
    let Some(descriptions) = create_description_from_array(ids) else {
        return Vec::new();
    };
    descriptions
        .iter()
        .filter_map(|d| {
            let pid = string_of(&d, "kCGWindowOwnerPID")?
                .downcast::<CFNumber>()?
                .to_i32()?;
            let layer = string_of(&d, "kCGWindowLayer")?
                .downcast::<CFNumber>()?
                .to_i32()?;
            let owner = string_of(&d, "kCGWindowOwnerName")?
                .downcast::<CFString>()?
                .to_string();
            let title = string_of(&d, "kCGWindowName")
                .and_then(|t| t.downcast::<CFString>())
                .map(|t| t.to_string())
                .unwrap_or_default();
            Some(RawWindow {
                pid,
                owner,
                title,
                layer,
            })
        })
        .collect()
}

fn limits() -> Limits {
    Limits {
        deadline: Instant::now() + WINDOW_BUDGET,
        subtree_nodes: WINDOW_NODES,
        ancestors: 0,
        above_bytes: 0,
        below_bytes: 0,
    }
}

/// The page address inside a window, if it shows a web page. Gives up at `deadline`: each
/// accessibility call can block on a hung app, so the search is bounded in time as well as size.
fn page_url(window: &Element, deadline: Instant) -> Option<String> {
    let mut stack = vec![window.clone()];
    let mut seen = 0;
    while let Some(el) = stack.pop() {
        seen += 1;
        if seen > URL_SEARCH_NODES || Instant::now() >= deadline {
            break;
        }
        if el.string("AXRole").as_deref() == Some("AXWebArea") {
            return el.url("AXURL");
        }
        stack.extend(el.elements("AXChildren").into_iter().rev());
    }
    None
}

/// The accessibility window of `pid` whose title is exactly the window-server `title`.
///
/// There is deliberately no fallback to "the app's first window": the privacy checks were run on
/// the window the window server described, so reading a different window of the same app (one on
/// another Space, say) would read something that was never vetted.
fn ax_window(pid: i32, title: &str) -> Option<Element> {
    Element::application(pid)?
        .elements("AXWindows")
        .into_iter()
        .find(|w| w.string("AXTitle").is_some_and(|t| t == title))
}

fn read_one(
    c: &Candidate,
    is_ignored: &(dyn Fn(&str, Option<&str>) -> bool + Sync),
) -> Option<WindowText> {
    if is_ignored(&c.app, None) || is_sensitive(&c.app, &c.title, None) {
        return None;
    }
    let window = ax_window(c.pid, &c.title)?;
    let url = page_url(&window, Instant::now() + WINDOW_BUDGET);
    if url.is_none() && is_browser(&c.app) {
        // Without the address the sensitive-site and ignore-list checks cannot run.
        return None;
    }
    // The address is only known after the walk starts, so check again with it.
    if is_ignored(&c.app, url.as_deref()) || is_sensitive(&c.app, &c.title, url.as_deref()) {
        return None;
    }
    let text = walk::whole(&window, None, limits()).join("\n");
    (!text.trim().is_empty()).then(|| WindowText {
        app: c.app.clone(),
        title: c.title.clone(),
        text,
    })
}

/// The text of the other windows visible on screen, front-most first. Empty when there are
/// none, or when everything visible is excluded.
#[tracing::instrument(skip_all, fields(candidates, read))]
pub fn read_other_windows(
    front_pid: i32,
    is_ignored: &(dyn Fn(&str, Option<&str>) -> bool + Sync),
) -> Vec<WindowText> {
    let candidates = pick(
        &on_screen_windows(),
        front_pid,
        std::process::id() as i32,
        MAX_WINDOWS,
    );
    tracing::Span::current().record("candidates", candidates.len());
    let read: Vec<WindowText> = std::thread::scope(|scope| {
        let handles: Vec<_> = candidates
            .iter()
            .map(|c| scope.spawn(move || read_one(c, is_ignored)))
            .collect();
        handles
            .into_iter()
            .filter_map(|h| h.join().ok().flatten())
            .collect()
    });
    tracing::Span::current().record("read", read.len());
    read
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(pid: i32, owner: &str, title: &str, layer: i32) -> RawWindow {
        RawWindow {
            pid,
            owner: owner.into(),
            title: title.into(),
            layer,
        }
    }

    #[test]
    fn the_focused_app_this_app_and_system_furniture_are_never_picked() {
        let windows = vec![
            w(10, "Slack", "platform-eng", 0),
            w(11, "Notion", "Q3 brief", 0),
            w(99, "Meridian", "Dashboard", 0),
            w(5, "Dock", "Dock", 0),
            w(6, "Control Center", "Control Center", 0),
        ];
        let picked = pick(&windows, 10, 99, 3);
        assert_eq!(
            picked,
            vec![Candidate {
                pid: 11,
                app: "Notion".into(),
                title: "Q3 brief".into()
            }]
        );
    }

    #[test]
    fn only_the_frontmost_window_of_each_app_counts() {
        let windows = vec![
            w(11, "Notion", "Page A", 0),
            w(11, "Notion", "Page B", 0),
            w(12, "Mail", "Inbox", 0),
        ];
        let picked = pick(&windows, 1, 99, 3);
        assert_eq!(
            picked.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(),
            ["Page A", "Inbox"]
        );
    }

    #[test]
    fn overlays_and_untitled_windows_are_skipped() {
        let windows = vec![
            w(11, "Some Overlay", "Panel", 25),
            w(12, "Helper", "", 0),
            w(13, "Mail", "Inbox", 0),
        ];
        let picked = pick(&windows, 1, 99, 3);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].app, "Mail");
    }

    #[test]
    fn the_count_is_capped_and_order_is_front_to_back() {
        let windows: Vec<RawWindow> = (0..10)
            .map(|i| w(100 + i, &format!("App{i}"), "t", 0))
            .collect();
        let picked = pick(&windows, 1, 99, 3);
        assert_eq!(
            picked.iter().map(|c| c.app.as_str()).collect::<Vec<_>>(),
            ["App0", "App1", "App2"]
        );
    }

    #[test]
    fn password_managers_and_money_pages_are_sensitive() {
        assert!(is_sensitive("1Password 7", "Vault", None));
        assert!(is_sensitive(
            "Google Chrome",
            "My accounts",
            Some("https://netbanking.hdfcbank.com/x")
        ));
        assert!(is_sensitive("Safari", "Sign in to your account", None));
        assert!(is_sensitive(
            "Chrome",
            "Dashboard",
            Some("https://dashboard.stripe.com/payments")
        ));
    }

    #[test]
    fn ordinary_work_windows_are_not_sensitive() {
        assert!(!is_sensitive("Notion", "Q3 rollout brief", None));
        assert!(!is_sensitive(
            "Google Chrome",
            "Inbox",
            Some("https://mail.google.com/mail/u/0/")
        ));
        assert!(!is_sensitive("Slack", "platform-eng", None));
    }

    #[test]
    fn private_browsing_windows_are_sensitive() {
        assert!(is_sensitive(
            "Google Chrome",
            "New Tab - Google Chrome (Incognito)",
            None
        ));
        assert!(is_sensitive(
            "Safari",
            "Private Browsing - Start Page",
            None
        ));
        assert!(is_sensitive("Microsoft Edge", "Search - InPrivate", None));
        assert!(!is_sensitive(
            "Google Chrome",
            "Pull requests - GitHub",
            Some("github.com/x")
        ));
    }

    #[test]
    fn browsers_are_recognised_by_name() {
        assert!(is_browser("Google Chrome"));
        assert!(is_browser("Arc"));
        assert!(is_browser("Microsoft Edge"));
        assert!(!is_browser("Slack"));
        assert!(!is_browser("Xcode"));
    }
}
