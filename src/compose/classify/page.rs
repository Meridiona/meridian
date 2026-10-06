//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Reading a host and a path out of the page address the tray hands over.
//!
//! The tray already strips the query and fragment, so this is deliberately tiny: no
//! general URL parser, no new dependency, just enough to decide "which site, and which
//! part of it" for the surface registry.
//!
//! # Who calls this
//! [`super::surface`] matches registry rules against the [`Page`] built here.
//!
//! # Related
//! - [`crate::compose::types::FieldSnapshot::url`] - the raw address this reads.

/// The site and path of the page a browser text field lives on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Page {
    /// Lower-cased host without a leading `www.`, e.g. `mail.google.com`. Empty when
    /// the field is not in a browser or the address is unusable.
    pub host: String,
    /// Lower-cased path starting with `/`, e.g. `/messaging/thread/1`. `/` when absent.
    pub path: String,
}

impl Page {
    /// Parse an address like `https://www.LinkedIn.com/messaging/thread/1`.
    /// Anything without a usable host yields the empty page.
    pub fn parse(url: Option<&str>) -> Page {
        let Some(raw) = url.map(str::trim).filter(|u| !u.is_empty()) else {
            return Page::default();
        };
        let rest = match raw.split_once("://") {
            Some((scheme, rest))
                if matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") =>
            {
                rest
            }
            // file://, chrome://, about:blank and friends are not sites.
            _ => return Page::default(),
        };
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        // Drop credentials and port.
        let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let host = host_port.split(':').next().unwrap_or("");
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
        if host.is_empty() || (!host.contains('.') && host != "localhost") {
            return Page::default();
        }
        Page {
            host,
            path: path.to_ascii_lowercase(),
        }
    }

    /// True when the host equals `domain` or is a subdomain of it.
    pub fn host_is(&self, domain: &str) -> bool {
        !self.host.is_empty()
            && (self.host == domain
                || self
                    .host
                    .strip_suffix(domain)
                    .is_some_and(|p| p.ends_with('.')))
    }

    /// True when the path starts with `prefix` at a segment boundary.
    pub fn path_starts_with(&self, prefix: &str) -> bool {
        match self.path.strip_prefix(prefix) {
            Some("") => true,
            Some(rest) => prefix.ends_with('/') || rest.starts_with('/'),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_host_and_path_lowercased() {
        let p = Page::parse(Some("https://www.LinkedIn.com/Messaging/thread/1"));
        assert_eq!(p.host, "linkedin.com");
        assert_eq!(p.path, "/messaging/thread/1");
    }

    #[test]
    fn missing_path_is_root() {
        assert_eq!(Page::parse(Some("https://x.com")).path, "/");
    }

    #[test]
    fn drops_port_and_credentials() {
        let p = Page::parse(Some("http://user:pw@mail.example.com:8080/a"));
        assert_eq!(p.host, "mail.example.com");
    }

    #[test]
    fn non_web_addresses_are_not_pages() {
        for u in [
            "file:///Users/a/b.txt",
            "chrome://settings",
            "about:blank",
            "",
            "   ",
            "not a url",
        ] {
            assert_eq!(Page::parse(Some(u)), Page::default(), "{u}");
        }
        assert_eq!(Page::parse(None), Page::default());
    }

    #[test]
    fn single_label_hosts_are_rejected_except_localhost() {
        assert_eq!(Page::parse(Some("http://intranet/x")), Page::default());
        assert_eq!(
            Page::parse(Some("http://localhost:3000/x")).host,
            "localhost"
        );
    }

    #[test]
    fn host_is_matches_subdomains_but_not_lookalikes() {
        let p = Page::parse(Some("https://acme.atlassian.net/browse/KAN-1"));
        assert!(p.host_is("atlassian.net"));
        assert!(!Page::parse(Some("https://notatlassian.net/")).host_is("atlassian.net"));
        assert!(!Page::default().host_is("atlassian.net"));
    }

    #[test]
    fn path_prefix_respects_segment_boundaries() {
        let p = Page::parse(Some("https://www.linkedin.com/messaging/thread/1"));
        assert!(p.path_starts_with("/messaging"));
        assert!(p.path_starts_with("/messaging/"));
        assert!(
            !Page::parse(Some("https://linkedin.com/messagingfoo")).path_starts_with("/messaging")
        );
        assert!(Page::parse(Some("https://linkedin.com/messaging")).path_starts_with("/messaging"));
    }
}
