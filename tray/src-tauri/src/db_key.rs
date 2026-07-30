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

/// Length of the SQLite file-header magic. A file shorter than this cannot be
/// probed for it at all — see [`ExistingDb::TooSmallToBeADatabase`].
const SQLITE_MAGIC_LEN: u64 = 16;

/// What the on-disk `meridian.db` looks like, as far as deciding whether it is
/// safe to mint a fresh encryption key is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExistingDb {
    /// No file — a genuine first run.
    Absent,
    /// A confirmed plaintext SQLite file. Nothing is encrypted yet, so
    /// `encrypt_in_place` will migrate it under the newly minted key.
    Plaintext,
    /// Present, but shorter than [`SQLITE_MAGIC_LEN`]. SQLCipher's smallest
    /// page is 512 bytes, so a file this short cannot hold encrypted data
    /// under any key — it is an empty or truncated leftover (a torn write, a
    /// disk-full stub), not a database. Nothing is lost by keying a fresh one
    /// over it, and blocking startup to demand a support ticket for a file
    /// with no recoverable content would be a false alarm. Carries the
    /// observed length so the caller can log it without re-stat'ing (and
    /// without having to invent a value if that second stat were to fail).
    TooSmallToBeADatabase { bytes: u64 },
    /// Present, long enough to be real, and carrying no recognizable SQLite
    /// header — i.e. ciphertext under some key that is not in the keychain.
    /// This is the case that must never be overwritten.
    LooksEncrypted,
}

/// Classify `db_path` into the four states that matter before minting a key.
///
/// Note this cannot be answered by [`meridian_core::db_crypto::plaintext_state`]
/// alone: it collapses *both* "file too short to read a header from" and "file
/// unreadable" into `None`, which is right for its own callers but would lump a
/// harmless empty stub in with real ciphertext here. The file length is what
/// actually separates them, so it is consulted directly.
///
/// An existing file whose metadata cannot be read is deliberately classified
/// [`ExistingDb::LooksEncrypted`] — the conservative direction, since guessing
/// "harmless" about a file we cannot inspect is the one mistake with an
/// irreversible outcome.
pub(crate) fn classify_existing_db(db_path: &std::path::Path) -> ExistingDb {
    if !db_path.exists() {
        return ExistingDb::Absent;
    }
    if meridian_core::db_crypto::is_plaintext_sqlite(db_path) {
        return ExistingDb::Plaintext;
    }
    match std::fs::metadata(db_path) {
        Ok(m) if m.len() < SQLITE_MAGIC_LEN => ExistingDb::TooSmallToBeADatabase { bytes: m.len() },
        _ => ExistingDb::LooksEncrypted,
    }
}

/// Would minting a brand-new key orphan an already-encrypted `meridian.db`?
/// True only for [`ExistingDb::LooksEncrypted`] — a file that exists, is long
/// enough to be a real database, and is not plaintext, i.e. ciphertext under a
/// key this install can no longer find. A missing file, a confirmed-plaintext
/// file, and an empty/truncated stub are all safe to proceed on.
pub(crate) fn would_orphan_existing_db(db_path: &std::path::Path) -> bool {
    matches!(classify_existing_db(db_path), ExistingDb::LooksEncrypted)
}

/// Get-or-create this machine's `meridian.db` encryption key from the OS
/// keychain, and ensure it is mirrored into `env_path` as
/// `MERIDIAN_DB_KEY=<hex>` so the daemon can read it. Returns the key hex.
///
/// Idempotent and safe to call on every tray startup: an existing keychain
/// entry is reused as-is (never regenerated), and re-mirroring the same value
/// into `.env` is a no-op write via `upsert_env`.
///
/// `db_path` exists solely so a missing keychain entry can be checked against
/// `meridian.db` before minting a replacement — see [`would_orphan_existing_db`].
/// Without that check, a keychain entry lost for any reason (a keychain
/// reset, a migration to a new machine, `.env` and the keychain falling out
/// of sync) silently mints and stores a fresh key while the real data stays
/// encrypted under the old, now-unreachable one: every future open then fails
/// with SQLCipher "wrong key" errors (`file is not a database` / `database
/// disk image is malformed`) instead of a clear, actionable one.
pub fn resolve_or_create_key(
    env_path: &std::path::Path,
    db_path: &std::path::Path,
) -> anyhow::Result<String> {
    let entry =
        keyring::Entry::new(SERVICE, ACCOUNT).context("db_key: failed to access OS keychain")?;

    let key_hex = match entry.get_password() {
        Ok(existing) => existing,
        Err(keyring::Error::NoEntry) => {
            match classify_existing_db(db_path) {
                ExistingDb::LooksEncrypted => anyhow::bail!(
                    "no DB encryption key found in the OS keychain, but {} already exists and \
                     does not look like a plaintext SQLite file - it looks encrypted under a \
                     key this install can no longer find. Refusing to generate a replacement \
                     key, which would silently make that data permanently unreadable instead \
                     of surfacing the problem. If this file is known-safe to discard, remove \
                     it and restart.",
                    db_path.display()
                ),
                // Distinct from the bail above on purpose: this file is too
                // short to hold encrypted data, so it is a leftover stub
                // rather than orphaned user data. Proceed (a fresh database
                // gets keyed over it), but say so — silently stepping over a
                // corrupt file is how the original bug stayed invisible.
                ExistingDb::TooSmallToBeADatabase { bytes } => {
                    tracing::warn!(
                        db_bytes = bytes,
                        "keying a fresh database over an empty or truncated meridian.db stub - too short to hold encrypted data, so nothing recoverable is being replaced"
                    );
                }
                ExistingDb::Absent | ExistingDb::Plaintext => {}
            }
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

    #[test]
    fn would_orphan_existing_db_is_false_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("meridian.db");
        assert!(!db_path.exists());
        assert_eq!(classify_existing_db(&db_path), ExistingDb::Absent);
        assert!(!would_orphan_existing_db(&db_path));
    }

    #[test]
    fn would_orphan_existing_db_is_false_when_plaintext() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("meridian.db");
        std::fs::write(&db_path, b"SQLite format 3\0rest-of-a-plaintext-file").unwrap();
        assert_eq!(classify_existing_db(&db_path), ExistingDb::Plaintext);
        assert!(!would_orphan_existing_db(&db_path));
    }

    #[test]
    fn would_orphan_existing_db_is_true_when_not_plaintext() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("meridian.db");
        // No recognizable SQLite header - stands in for SQLCipher ciphertext.
        std::fs::write(&db_path, b"not-a-sqlite-header-at-all-just-ciphertext").unwrap();
        assert_eq!(classify_existing_db(&db_path), ExistingDb::LooksEncrypted);
        assert!(would_orphan_existing_db(&db_path));
    }

    /// A zero-byte `meridian.db` — the shape a torn write or a disk-full stub
    /// leaves behind. It reads as "not plaintext" (there is no header to read),
    /// so it must be told apart from real ciphertext explicitly, or a user with
    /// nothing to lose is sent to support instead of simply being healed.
    #[test]
    fn an_empty_stub_is_not_treated_as_encrypted_data() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("meridian.db");
        std::fs::write(&db_path, b"").unwrap();
        assert!(!meridian_core::db_crypto::is_plaintext_sqlite(&db_path));
        assert_eq!(
            classify_existing_db(&db_path),
            ExistingDb::TooSmallToBeADatabase { bytes: 0 }
        );
        assert!(!would_orphan_existing_db(&db_path));
    }

    /// Truncated part-way through the magic — same reasoning as the empty stub:
    /// too short to hold a SQLCipher page, so there is nothing to orphan.
    #[test]
    fn a_truncated_stub_is_not_treated_as_encrypted_data() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("meridian.db");
        std::fs::write(&db_path, b"SQLite f").unwrap();
        assert_eq!(
            classify_existing_db(&db_path),
            ExistingDb::TooSmallToBeADatabase { bytes: 8 }
        );
        assert!(!would_orphan_existing_db(&db_path));
    }

    /// The boundary itself: exactly `SQLITE_MAGIC_LEN` bytes of non-header
    /// content is long enough to probe, so it stays on the conservative side.
    #[test]
    fn exactly_magic_length_of_non_header_still_looks_encrypted() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("meridian.db");
        let contents = vec![0xABu8; SQLITE_MAGIC_LEN as usize];
        std::fs::write(&db_path, &contents).unwrap();
        assert_eq!(classify_existing_db(&db_path), ExistingDb::LooksEncrypted);
        assert!(would_orphan_existing_db(&db_path));
    }
}
