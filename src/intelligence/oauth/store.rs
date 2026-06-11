//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Daemon-writable OAuth token store. Unlike static API tokens (which live in
// `.env`, read-only to the daemon), OAuth access tokens expire and refresh
// tokens ROTATE on every use — so the daemon must persist new tokens back. We
// keep them out of `.env` in their own per-provider JSON file at
// `~/.meridian/oauth/<provider>.json`, written `0600`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Persisted OAuth credentials for one provider. `cloud_id` / `site_url` are
/// Jira-specific (the `accessible-resources` lookup result); they stay empty for
/// providers that don't need them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub provider: String,
    /// The public client_id this token was minted for — needed to refresh, so we
    /// persist it rather than depending on an env var at refresh time.
    pub client_id: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds at which `access_token` expires.
    pub expires_at: i64,
    /// Space-separated granted scopes (informational / debugging).
    #[serde(default)]
    pub scopes: String,
    /// Atlassian cloud id — the `{cloudId}` in `api.atlassian.com/ex/jira/{cloudId}`.
    #[serde(default)]
    pub cloud_id: String,
    /// The site base URL (e.g. `https://acme.atlassian.net`) for building browse links.
    #[serde(default)]
    pub site_url: String,
}

impl OAuthTokens {
    /// True when the access token has expired (or is within `skew_secs` of it).
    /// Refresh-before-use keys off this with a small skew so an in-flight request
    /// never races the expiry boundary.
    pub fn is_expired(&self, now_unix: i64, skew_secs: i64) -> bool {
        now_unix + skew_secs >= self.expires_at
    }
}

fn oauth_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".meridian").join("oauth")
}

/// Path to a provider's token file, e.g. `~/.meridian/oauth/jira.json`.
pub fn path(provider: &str) -> PathBuf {
    oauth_dir().join(format!("{provider}.json"))
}

/// Whether a token file exists for this provider. Used to decide if OAuth is the
/// active auth path before falling back to a static API token.
pub fn exists(provider: &str) -> bool {
    path(provider).exists()
}

/// Load and parse a provider's tokens. Errors if the file is missing or corrupt.
pub fn load(provider: &str) -> Result<OAuthTokens> {
    let p = path(provider);
    let raw = std::fs::read_to_string(&p)
        .with_context(|| format!("reading OAuth token file {}", p.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parsing OAuth token file {}", p.display()))
}

/// Persist tokens atomically-ish (write temp, rename) with `0600` permissions so
/// the refresh/access tokens aren't world-readable. On Unix, sets permissions before
/// writing to avoid a race window where other users could read the file.
pub fn save(tokens: &OAuthTokens) -> Result<()> {
    let dir = oauth_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating OAuth dir {}", dir.display()))?;
    let final_path = path(&tokens.provider);
    let tmp_path = dir.join(format!(".{}.json.tmp", tokens.provider));

    let json = serde_json::to_string_pretty(tokens).context("serialising OAuth tokens")?;

    #[cfg(unix)]
    {
        use std::fs::OpenOptions;
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp_path)
            .with_context(|| {
                format!("creating token file with 0600 mode {}", tmp_path.display())
            })?;
        file.write_all(json.as_bytes())
            .with_context(|| format!("writing token file {}", tmp_path.display()))?;
    }

    #[cfg(not(unix))]
    {
        std::fs::write(&tmp_path, json)
            .with_context(|| format!("writing temp token file {}", tmp_path.display()))?;
    }

    std::fs::rename(&tmp_path, &final_path)
        .with_context(|| format!("renaming token file into place {}", final_path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_is_under_meridian_oauth() {
        std::env::set_var("HOME", "/tmp/meridian_oauth_test_home");
        let p = path("jira");
        assert!(
            p.ends_with(".meridian/oauth/jira.json"),
            "got {}",
            p.display()
        );
    }

    #[test]
    fn is_expired_respects_skew() {
        let t = OAuthTokens {
            provider: "jira".into(),
            client_id: "cid".into(),
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: 1_000,
            scopes: String::new(),
            cloud_id: String::new(),
            site_url: String::new(),
        };
        // 60s before expiry, with a 60s skew → treated as expired.
        assert!(t.is_expired(940, 60));
        // 61s before expiry, with a 60s skew → still fresh.
        assert!(!t.is_expired(939, 60));
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = std::env::temp_dir().join(format!("meridian_oauth_rt_{}", std::process::id()));
        std::env::set_var("HOME", &dir);
        let t = OAuthTokens {
            provider: "jira".into(),
            client_id: "cid".into(),
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            expires_at: 12345,
            scopes: "read:jira-work offline_access".into(),
            cloud_id: "cloud-1".into(),
            site_url: "https://acme.atlassian.net".into(),
        };
        save(&t).unwrap();
        assert!(exists("jira"));
        let loaded = load("jira").unwrap();
        assert_eq!(loaded.access_token, "access");
        assert_eq!(loaded.cloud_id, "cloud-1");
        assert_eq!(loaded.site_url, "https://acme.atlassian.net");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path("jira"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "token file must be 0600");
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
