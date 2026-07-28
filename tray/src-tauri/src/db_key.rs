//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Resolves (or first-run generates) the local SQLCipher encryption key for
//! `meridian.db`, and mirrors it into the canonical `.env` the daemon reads.
//!
//! # Who calls this
//! `lib.rs`'s app-setup path, once, before opening the shared DB pool — the
//! resolved key is used both to open the pool there and (via the `.env`
//! mirror written here) by the daemon's own `setup_db`
//! (`src/db/meridian.rs`), which reads `MERIDIAN_DB_KEY` from its process env.
//!
//! # Storage model
//! Source of truth is the OS keychain (`keyring` crate — macOS Keychain /
//! Windows Credential Manager), matching how the OS already protects other
//! app secrets on this machine. It is mirrored into the `.env` the tray
//! resolves via [`crate::install::InstallMode::env_path`] /
//! [`crate::install::canonical_env_path`] as `MERIDIAN_DB_KEY=<hex>`, using the
//! exact same [`crate::commands::integrations::upsert_env`] mechanism this
//! tray already uses for tracker credentials — because the daemon is a
//! separate, headless launchd process that cannot prompt for Keychain access,
//! it reads the mirrored file instead, exactly like it already does for
//! `MERIDIAN_DB` itself (see `install.rs`'s module doc).
//!
//! # Related
//! - [`meridian_core::db_crypto`] — key format/validation and the migration
//!   this key is used for.
//! - [`crate::install`] — `.env` path resolution this module writes into.

use anyhow::Context;
use rand::RngCore;

const SERVICE: &str = "Meridian";
const ACCOUNT: &str = "db-encryption-key";
const ENV_KEY: &str = "MERIDIAN_DB_KEY";

fn generate_key_hex() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Get-or-create this machine's `meridian.db` encryption key from the OS
/// keychain, and ensure it is mirrored into `env_path` as
/// `MERIDIAN_DB_KEY=<hex>` so the daemon can read it. Returns the key hex.
///
/// Idempotent and safe to call on every tray startup: an existing keychain
/// entry is reused as-is (never regenerated), and re-mirroring the same value
/// into `.env` is a no-op write via `upsert_env`.
pub fn resolve_or_create_key(env_path: &std::path::Path) -> anyhow::Result<String> {
    let entry =
        keyring::Entry::new(SERVICE, ACCOUNT).context("db_key: failed to access OS keychain")?;

    let key_hex = match entry.get_password() {
        Ok(existing) => existing,
        Err(keyring::Error::NoEntry) => {
            let generated = generate_key_hex();
            entry
                .set_password(&generated)
                .context("db_key: failed to store new key in OS keychain")?;
            tracing::info!("generated new local database encryption key");
            generated
        }
        Err(e) => return Err(e).context("db_key: failed to read key from OS keychain"),
    };

    meridian_core::db_crypto::validate_key_hex(&key_hex)
        .context("db_key: key stored in keychain is malformed")?;

    let mut updates = std::collections::BTreeMap::new();
    updates.insert(ENV_KEY.to_string(), key_hex.clone());
    crate::commands::integrations::upsert_env(env_path, &updates)
        .context("db_key: failed to mirror key into .env")?;

    Ok(key_hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_key_hex_produces_valid_keys() {
        let a = generate_key_hex();
        let b = generate_key_hex();
        meridian_core::db_crypto::validate_key_hex(&a).unwrap();
        meridian_core::db_crypto::validate_key_hex(&b).unwrap();
        assert_ne!(a, b, "two generated keys should not collide");
    }
}
