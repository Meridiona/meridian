//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Deciding what kind of text box a press landed in, and whether to refuse it.
//!
//! # The shape of the decision
//! 1. **Refuse first.** A password field, secure input, a private window, a search box
//!    or the address bar never reach the model. These are privacy and safety calls, so
//!    they run before any app knowledge is consulted.
//! 2. **Then match what we know.** A table of apps and sites ([`APP_RULES`]) says what
//!    kind of box each is. Order matters: a more specific rule (LinkedIn messaging)
//!    sits above the general one (LinkedIn the feed).
//! 3. **Then fall back.** A single-line field in a prose surface is a structured field
//!    (a subject line, a calendar title). Anything unknown is [`SurfaceKind::Generic`]
//!    and still works - an unknown box must never be a dead end.
//!
//! The registry is data, so supporting a new app is one row and one test, never a new
//! branch of logic.
//!
//! # Who calls this
//! [`super::classify`], once per press.
//!
//! # Related
//! - [`super::page`] - the host and path matched against site rules.
//! - [`super::intent`] - runs after this, because the intent depends on the surface.

use super::page::Page;
use crate::compose::types::{FieldSnapshot, RefusalReason, SurfaceKind};

/// One site, optionally narrowed to a path.
struct SiteRule {
    host: &'static str,
    path_prefix: Option<&'static str>,
}

const fn site(host: &'static str) -> SiteRule {
    SiteRule {
        host,
        path_prefix: None,
    }
}

const fn site_at(host: &'static str, path_prefix: &'static str) -> SiteRule {
    SiteRule {
        host,
        path_prefix: Some(path_prefix),
    }
}

/// What we know about a family of apps and sites.
struct AppRule {
    kind: SurfaceKind,
    /// Exact bundle ids.
    bundles: &'static [&'static str],
    /// Bundle id prefixes, for families such as `com.jetbrains.`.
    bundle_prefixes: &'static [&'static str],
    sites: &'static [SiteRule],
}

/// Registry, most specific first. The first rule that matches wins.
const APP_RULES: &[AppRule] = &[
    // LinkedIn messaging must beat LinkedIn the feed (below, under PublicComment).
    AppRule {
        kind: SurfaceKind::DirectChat,
        bundles: &[
            "com.tinyspeck.slackmacgap",
            "net.whatsapp.WhatsApp",
            "com.apple.MobileSMS",
            "com.microsoft.teams2",
            "com.microsoft.teams",
            "com.hnc.Discord",
            "ru.keepcoder.Telegram",
            "org.whispersystems.signal-desktop",
        ],
        bundle_prefixes: &[],
        sites: &[
            site_at("linkedin.com", "/messaging"),
            site("app.slack.com"),
            site("web.whatsapp.com"),
            site("discord.com"),
            site("teams.microsoft.com"),
            site("messenger.com"),
            site("web.telegram.org"),
            site_at("facebook.com", "/messages"),
            site_at("x.com", "/messages"),
            site_at("twitter.com", "/messages"),
            site_at("instagram.com", "/direct"),
        ],
    },
    AppRule {
        kind: SurfaceKind::EmailCompose,
        bundles: &[
            "com.apple.mail",
            "com.microsoft.Outlook",
            "com.readdle.SparkDesktop",
            "com.superhuman.electron",
            "com.mimestream.Mimestream",
            "it.bloop.airmail3",
        ],
        bundle_prefixes: &[],
        sites: &[
            site("mail.google.com"),
            site("outlook.office.com"),
            site("outlook.office365.com"),
            site("outlook.live.com"),
            site("mail.proton.me"),
            site("app.hey.com"),
            site("mail.yahoo.com"),
            site("mail.superhuman.com"),
        ],
    },
    AppRule {
        kind: SurfaceKind::AiPrompt,
        bundles: &["com.openai.chat", "com.anthropic.claudefordesktop"],
        bundle_prefixes: &[],
        sites: &[
            site("chatgpt.com"),
            site("chat.openai.com"),
            site("claude.ai"),
            site("gemini.google.com"),
            site("perplexity.ai"),
            site("grok.com"),
            site("copilot.microsoft.com"),
        ],
    },
    AppRule {
        kind: SurfaceKind::Terminal,
        bundles: &[
            "com.apple.Terminal",
            "com.googlecode.iterm2",
            "com.mitchellh.ghostty",
            "net.kovidgoyal.kitty",
            "dev.warp.Warp-Stable",
            "org.alacritty",
            "co.zeit.hyper",
        ],
        bundle_prefixes: &[],
        sites: &[],
    },
    AppRule {
        kind: SurfaceKind::CodeEditor,
        bundles: &[
            "com.microsoft.VSCode",
            "com.todesktop.230313mzl4w4u92",
            "com.apple.dt.Xcode",
            "dev.zed.Zed",
        ],
        bundle_prefixes: &["com.jetbrains.", "com.sublimetext."],
        sites: &[],
    },
    AppRule {
        kind: SurfaceKind::Document,
        bundles: &[
            "com.apple.iWork.Pages",
            "com.microsoft.Word",
            "com.apple.Notes",
            "com.apple.TextEdit",
            "md.obsidian",
            "notion.id",
        ],
        bundle_prefixes: &[],
        sites: &[
            site_at("docs.google.com", "/document"),
            site("notion.so"),
            site("notion.com"),
            site("coda.io"),
            site_at("atlassian.net", "/wiki"),
            site("medium.com"),
        ],
    },
    AppRule {
        kind: SurfaceKind::PublicComment,
        bundles: &[],
        bundle_prefixes: &[],
        sites: &[
            site("linkedin.com"),
            site("x.com"),
            site("twitter.com"),
            site("reddit.com"),
            site("youtube.com"),
            site("news.ycombinator.com"),
            site("github.com"),
            site("gitlab.com"),
            site("atlassian.net"),
            site("linear.app"),
            site("facebook.com"),
            site("instagram.com"),
            site("threads.net"),
            site("bsky.app"),
        ],
    },
];

/// Placeholder or label phrases that mean "this is a chat composer", whatever the site.
/// LinkedIn's floating message box lives on every LinkedIn page, so the page alone cannot
/// say it is a message.
const CHAT_HINTS: &[&str] = &[
    "write a message",
    "type a message",
    "reply to conversation",
    "message #",
];

/// Placeholder or label phrases that mean "this is a public reply box".
const COMMENT_HINTS: &[&str] = &[
    "add a comment",
    "post your reply",
    "add a reply",
    "write a comment",
];

fn lower(s: &str) -> String {
    s.to_ascii_lowercase()
}

/// True when `text` contains `word` as a whole word, so `research` does not match `search`.
fn has_word(text: &str, word: &str) -> bool {
    text.split(|c: char| !c.is_alphanumeric())
        .any(|t| t.eq_ignore_ascii_case(word))
}

/// The reason to refuse this press, if any. Runs before anything else.
pub fn refusal(field: &FieldSnapshot) -> Option<RefusalReason> {
    let subrole = lower(&field.ax_subrole);
    let role = lower(&field.ax_role);
    if subrole.contains("secure") || role.contains("secure") {
        return Some(RefusalReason::SecureField);
    }
    if field.secure_input {
        return Some(RefusalReason::SecureInput);
    }
    if field.private_window {
        return Some(RefusalReason::PrivateWindow);
    }
    if role.contains("searchfield") || subrole.contains("searchfield") {
        return Some(RefusalReason::SearchOrAddressBar);
    }
    if !field.multiline {
        let hint = format!("{} {}", lower(&field.label), lower(&field.placeholder));
        if has_word(&hint, "search") || has_word(&hint, "omnibox") || hint.contains("address bar") {
            return Some(RefusalReason::SearchOrAddressBar);
        }
    }
    None
}

fn app_kind(field: &FieldSnapshot, page: &Page) -> Option<SurfaceKind> {
    let hint = format!("{} {}", lower(&field.label), lower(&field.placeholder));
    if CHAT_HINTS.iter().any(|h| hint.contains(h)) {
        return Some(SurfaceKind::DirectChat);
    }
    if COMMENT_HINTS.iter().any(|h| hint.contains(h)) {
        return Some(SurfaceKind::PublicComment);
    }
    for rule in APP_RULES {
        let by_bundle = !field.bundle_id.is_empty()
            && (rule.bundles.contains(&field.bundle_id.as_str())
                || rule
                    .bundle_prefixes
                    .iter()
                    .any(|p| field.bundle_id.starts_with(p)));
        let by_site = rule.sites.iter().any(|s| {
            page.host_is(s.host)
                && s.path_prefix
                    .is_none_or(|prefix| page.path_starts_with(prefix))
        });
        if by_bundle || by_site {
            return Some(rule.kind);
        }
    }
    None
}

/// The surface for a press that was not refused. Never fails: unknown boxes are
/// [`SurfaceKind::Generic`] (multi-line) or [`SurfaceKind::StructuredField`] (single-line).
pub fn surface_kind(field: &FieldSnapshot, page: &Page) -> SurfaceKind {
    let matched = app_kind(field, page);
    match matched {
        // A short single-line field inside a prose surface is a value, not prose: a
        // subject line, a calendar title, a spreadsheet cell.
        Some(
            SurfaceKind::EmailCompose
            | SurfaceKind::PublicComment
            | SurfaceKind::Document
            | SurfaceKind::Generic,
        )
        | None
            if !field.multiline =>
        {
            SurfaceKind::StructuredField
        }
        Some(kind) => kind,
        None => SurfaceKind::Generic,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(bundle: &str, url: Option<&str>, multiline: bool) -> FieldSnapshot {
        FieldSnapshot {
            bundle_id: bundle.into(),
            url: url.map(str::to_string),
            multiline,
            ax_role: if multiline {
                "AXTextArea".into()
            } else {
                "AXTextField".into()
            },
            ..Default::default()
        }
    }

    fn kind(f: &FieldSnapshot) -> SurfaceKind {
        surface_kind(f, &Page::parse(f.url.as_deref()))
    }

    #[test]
    fn secure_fields_and_states_are_refused() {
        let mut f = field("", None, false);
        f.ax_subrole = "AXSecureTextField".into();
        assert_eq!(refusal(&f), Some(RefusalReason::SecureField));
        let mut f = field("", None, true);
        f.secure_input = true;
        assert_eq!(refusal(&f), Some(RefusalReason::SecureInput));
        let mut f = field("", None, true);
        f.private_window = true;
        assert_eq!(refusal(&f), Some(RefusalReason::PrivateWindow));
    }

    #[test]
    fn search_and_address_bars_are_refused() {
        let mut f = field("", None, false);
        f.ax_role = "AXSearchField".into();
        assert_eq!(refusal(&f), Some(RefusalReason::SearchOrAddressBar));
        let mut f = field("", None, false);
        f.label = "Address and search bar".into();
        assert_eq!(refusal(&f), Some(RefusalReason::SearchOrAddressBar));
    }

    #[test]
    fn the_word_research_is_not_search() {
        let mut f = field("", None, false);
        f.label = "Research topic".into();
        assert_eq!(refusal(&f), None);
    }

    #[test]
    fn a_multiline_field_labelled_search_is_not_refused_by_label_alone() {
        let mut f = field("", None, true);
        f.label = "Search the docs and explain".into();
        assert_eq!(refusal(&f), None);
    }

    #[test]
    fn ordinary_fields_are_not_refused() {
        assert_eq!(
            refusal(&field("com.tinyspeck.slackmacgap", None, true)),
            None
        );
    }

    #[test]
    fn known_apps_resolve_by_bundle_id() {
        let cases = [
            ("com.tinyspeck.slackmacgap", SurfaceKind::DirectChat),
            ("com.apple.mail", SurfaceKind::EmailCompose),
            ("com.microsoft.VSCode", SurfaceKind::CodeEditor),
            ("com.jetbrains.intellij", SurfaceKind::CodeEditor),
            ("com.googlecode.iterm2", SurfaceKind::Terminal),
            ("com.apple.Notes", SurfaceKind::Document),
            ("com.openai.chat", SurfaceKind::AiPrompt),
        ];
        for (bundle, want) in cases {
            assert_eq!(kind(&field(bundle, None, true)), want, "{bundle}");
        }
    }

    #[test]
    fn known_sites_resolve_by_host() {
        let cases = [
            (
                "https://mail.google.com/mail/u/0/",
                SurfaceKind::EmailCompose,
            ),
            (
                "https://app.slack.com/client/T1/C2",
                SurfaceKind::DirectChat,
            ),
            (
                "https://www.reddit.com/r/rust/comments/1",
                SurfaceKind::PublicComment,
            ),
            (
                "https://acme.atlassian.net/browse/KAN-1",
                SurfaceKind::PublicComment,
            ),
            (
                "https://acme.atlassian.net/wiki/spaces/X",
                SurfaceKind::Document,
            ),
            (
                "https://docs.google.com/document/d/1/edit",
                SurfaceKind::Document,
            ),
            ("https://claude.ai/chat/1", SurfaceKind::AiPrompt),
            ("https://example.com/", SurfaceKind::Generic),
        ];
        for (url, want) in cases {
            assert_eq!(
                kind(&field("com.google.Chrome", Some(url), true)),
                want,
                "{url}"
            );
        }
    }

    #[test]
    fn linkedin_messaging_beats_linkedin_feed() {
        let dm = field(
            "com.google.Chrome",
            Some("https://www.linkedin.com/messaging/thread/1"),
            true,
        );
        let feed = field(
            "com.google.Chrome",
            Some("https://www.linkedin.com/feed/"),
            true,
        );
        assert_eq!(kind(&dm), SurfaceKind::DirectChat);
        assert_eq!(kind(&feed), SurfaceKind::PublicComment);
    }

    #[test]
    fn linkedin_floating_message_box_is_chat_on_any_page() {
        let mut f = field(
            "com.google.Chrome",
            Some("https://www.linkedin.com/feed/"),
            true,
        );
        f.placeholder = "Write a message\u{2026}".into();
        assert_eq!(kind(&f), SurfaceKind::DirectChat);
    }

    #[test]
    fn single_line_fields_in_prose_surfaces_are_structured() {
        let subject = field(
            "com.google.Chrome",
            Some("https://mail.google.com/mail/u/0/"),
            false,
        );
        assert_eq!(kind(&subject), SurfaceKind::StructuredField);
        let unknown = field("com.example.app", None, false);
        assert_eq!(kind(&unknown), SurfaceKind::StructuredField);
    }

    #[test]
    fn single_line_chat_and_terminal_keep_their_kind() {
        assert_eq!(
            kind(&field("com.googlecode.iterm2", None, false)),
            SurfaceKind::Terminal
        );
        assert_eq!(
            kind(&field("com.tinyspeck.slackmacgap", None, false)),
            SurfaceKind::DirectChat
        );
    }

    #[test]
    fn unknown_multiline_box_is_generic_not_an_error() {
        assert_eq!(
            kind(&field("com.example.app", None, true)),
            SurfaceKind::Generic
        );
    }

    #[test]
    fn lookalike_domains_do_not_match() {
        let f = field(
            "com.google.Chrome",
            Some("https://notlinkedin.com/messaging"),
            true,
        );
        assert_eq!(kind(&f), SurfaceKind::Generic);
    }
}
