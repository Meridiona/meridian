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

/// Reject a provider name that isn't a plain identifier, so a token/lock path can
/// never escape the oauth dir via `..` or a path separator. Defense-in-depth: every
/// caller today passes a hardcoded literal ("jira"/"trello"), but validating at the
/// boundary means a future untrusted source can't turn this into path traversal.
fn validate_provider(provider: &str) -> Result<()> {
    let ok = !provider.is_empty()
        && provider
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        anyhow::bail!(
            "invalid OAuth provider {provider:?} — only [A-Za-z0-9_-] is allowed; refusing \
             to build a token path that could escape the store dir"
        )
    }
}

/// Path to a provider's token file, e.g. `~/.meridian/oauth/jira.json`. Errors on a
/// provider name that could escape the oauth dir (see [`validate_provider`]).
pub fn path(provider: &str) -> Result<PathBuf> {
    validate_provider(provider)?;
    Ok(oauth_dir().join(format!("{provider}.json")))
}

/// Whether a token file exists for this provider. Used to decide if OAuth is the
/// active auth path before falling back to a static API token. An invalid provider
/// name is treated as "not present" rather than surfacing an error.
pub fn exists(provider: &str) -> bool {
    path(provider).map(|p| p.exists()).unwrap_or(false)
}

/// Load and parse a provider's tokens. Errors if the file is missing or corrupt.
pub fn load(provider: &str) -> Result<OAuthTokens> {
    let p = path(provider)?;
    let raw = std::fs::read_to_string(&p)
        .with_context(|| format!("reading OAuth token file {}", p.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("parsing OAuth token file {}", p.display()))
}

/// Persist tokens atomically-ish (write temp, rename) with `0600` permissions so
/// the refresh/access tokens aren't world-readable. On Unix, sets permissions before
/// writing to avoid a race window where other users could read the file.
pub fn save(tokens: &OAuthTokens) -> Result<()> {
    // Validate the provider up front (also gates the tmp path below, which
    // interpolates it too) before touching the filesystem.
    let final_path = path(&tokens.provider)?;
    let dir = oauth_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating OAuth dir {}", dir.display()))?;
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

/// Held exclusive advisory lock on a provider's token store. The OS `flock` is
/// tied to the open file description, so keeping this guard alive holds the lock;
/// dropping it (here, explicitly, and again when the fd closes) releases it.
pub struct ProviderLock {
    file: std::fs::File,
    path: PathBuf,
}

impl Drop for ProviderLock {
    fn drop(&mut self) {
        // Best-effort: the lock also releases when the fd closes on drop, so a
        // failed explicit unlock is harmless — just note it for debugging.
        if let Err(e) = self.file.unlock() {
            tracing::debug!(path = %self.path.display(), error = %e, "releasing OAuth lock");
        }
    }
}

/// Lock file path for a provider, e.g. `~/.meridian/oauth/jira.lock`. Deliberately
/// SEPARATE from `<provider>.json`: [`save`] renames a temp file over the json, so
/// a lock taken on the json itself would be lost across that rename. Validates the
/// provider so the lock path can't escape the oauth dir (see [`validate_provider`]).
fn lock_path(provider: &str) -> Result<PathBuf> {
    validate_provider(provider)?;
    Ok(oauth_dir().join(format!("{provider}.lock")))
}

/// Acquire the exclusive cross-process advisory lock for `provider`'s token store,
/// blocking (async) until it's free or a 10 s safety timeout elapses.
///
/// This serialises the rotating-refresh-token read-modify-write across EVERY
/// Meridian process — a second daemon, the tray's in-process refresh, the daemon's
/// background sync. Without it, two `ensure_fresh()` calls both load the same
/// expired token, both POST a refresh, and the second 401s because the first
/// already consumed (rotated) the refresh token — permanently invalidating the
/// pair (the FIXME race in `jira.rs`). The caller MUST re-load and re-check expiry
/// AFTER acquiring this, so a process that waited adopts the peer's fresh token
/// instead of refreshing again with the now-dead one.
pub async fn lock_provider(provider: &str) -> Result<ProviderLock> {
    lock_provider_with_timeout(provider, std::time::Duration::from_secs(10)).await
}

/// [`lock_provider`] with an explicit timeout (the public fn fixes it at 10 s;
/// tests pass a short one to exercise contention without a real wait).
async fn lock_provider_with_timeout(
    provider: &str,
    timeout: std::time::Duration,
) -> Result<ProviderLock> {
    use anyhow::bail;

    let path = lock_path(provider)?; // validates provider before any FS work
    let dir = oauth_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating OAuth dir {}", dir.display()))?;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("opening OAuth lock file {}", path.display()))?;

    // Poll a NON-blocking try-lock (std's native `File::try_lock`, stable since
    // Rust 1.89) so the async executor is never parked on flock. Contention is rare
    // and brief (one HTTP refresh) and a crashed holder releases automatically
    // (the lock dies with its fd), so a short poll resolves quickly.
    let step = std::time::Duration::from_millis(50);
    let mut waited = std::time::Duration::ZERO;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(ProviderLock { file, path }),
            Err(std::fs::TryLockError::WouldBlock) => {
                if waited >= timeout {
                    bail!(
                        "timed out after {timeout:?} acquiring the OAuth refresh lock for \
                         {provider} — another Meridian process may be stuck refreshing"
                    );
                }
                tokio::time::sleep(step).await;
                waited += step;
            }
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(e)
                    .with_context(|| format!("acquiring exclusive lock on {}", path.display()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_is_under_meridian_oauth() {
        std::env::set_var("HOME", "/tmp/meridian_oauth_test_home");
        let p = path("jira").unwrap();
        assert!(
            p.ends_with(".meridian/oauth/jira.json"),
            "got {}",
            p.display()
        );
    }

    #[test]
    fn path_rejects_traversal_providers() {
        // A provider name that could escape the oauth dir must be refused at the
        // boundary, for both the token path and the lock path.
        for bad in ["../../etc/passwd", "a/b", "..", "", "jira\0", "jira.json"] {
            assert!(path(bad).is_err(), "path({bad:?}) must be rejected");
            assert!(
                lock_path(bad).is_err(),
                "lock_path({bad:?}) must be rejected"
            );
        }
        // The real providers stay valid.
        for good in ["jira", "trello", "azure_devops", "github"] {
            assert!(path(good).is_ok(), "path({good:?}) must be allowed");
        }
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
            let mode = std::fs::metadata(path("jira").unwrap())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "token file must be 0600");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lock_provider_serialises_then_releases() {
        // flock is per-open-file-description, so two separate opens contend even in
        // one process — enough to prove cross-process serialisation works.
        let dir = std::env::temp_dir().join(format!("meridian_oauth_lock_{}", std::process::id()));
        std::env::set_var("HOME", &dir);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let short = std::time::Duration::from_millis(150);
            let held = lock_provider("jira").await.expect("first lock acquires");

            // A second acquisition while the first is held must time out (contended).
            let contended = lock_provider_with_timeout("jira", short).await;
            assert!(
                contended.is_err(),
                "second lock must not be granted while the first is held"
            );

            // After releasing, the lock is immediately available again.
            drop(held);
            let reacquired = lock_provider_with_timeout("jira", short).await;
            assert!(
                reacquired.is_ok(),
                "lock must be acquirable once the holder drops it"
            );
        });
        std::fs::remove_dir_all(&dir).ok();
    }
}
