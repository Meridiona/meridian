//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Looks up (never mints) the local SQLCipher encryption key for an
//! already-encrypted `meridian.db`, for the one-way decrypt-back-to-plaintext
//! migration — and removes it from the OS keychain and `.env` once that
//! migration has run.
//!
//! # New installs are never encrypted
//! This module used to also GENERATE a fresh key and mirror it into `.env` so
//! a brand-new install's `meridian.db` would be created encrypted
//! (`resolve_or_create_key`, removed). Encryption at rest has been removed —
//! see `meridian_core::db_crypto`'s module doc — so a missing keychain entry
//! now means exactly what it says: nothing to migrate, not "generate one".
//! What remains here exists only to find and retire a key that ALREADY
//! encrypted an install before that change.
//!
//! # Who calls this
//! `lib.rs`'s app-setup path, once, before opening the shared DB pool: if
//! [`classify_existing_db`] reports [`ExistingDb::LooksEncrypted`] and
//! [`resolve_existing_key`] finds a key, it runs
//! [`meridian_core::db_crypto::decrypt_in_place`] and then
//! [`remove_key_from_keychain`] + [`crate::commands::integrations::strip_env_keys`]
//! (for `MERIDIAN_DB_KEY`) so no future launch goes looking for it again.
//!
//! # Storage model
//! Source of truth is the OS keychain (`keyring` crate — macOS Keychain /
//! Windows Credential Manager), mirrored into the `.env` the tray resolves via
//! [`crate::install::InstallMode::env_path`] / [`crate::install::canonical_env_path`]
//! as `MERIDIAN_DB_KEY=<hex>` — because the daemon is a separate, headless
//! launchd process that cannot prompt for Keychain access, it reads the
//! mirrored file instead, exactly like it already does for `MERIDIAN_DB`
//! itself (see `install.rs`'s module doc).
//!
//! # Related
//! - [`meridian_core::db_crypto`] — key format/validation and the migration
//!   this key is used for.
//! - [`crate::install`] — `.env` path resolution this module writes into.

const SERVICE: &str = "Meridian";
const ACCOUNT: &str = "db-encryption-key";
/// `.env` key mirroring the keychain entry — `pub(crate)` so `lib.rs` can pass
/// it to [`crate::commands::integrations::strip_env_keys`] once the decrypt
/// migration has run.
pub(crate) const ENV_KEY: &str = "MERIDIAN_DB_KEY";

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

/// Is `key_hex` the key this machine's OS keychain holds for `meridian.db`?
///
/// Read-only and non-minting - deliberately NOT [`resolve_or_create_key`],
/// which generates and stores a key when the entry is missing. The one caller
/// ([`crate::repair_boot`]) is deciding whether it may move a database aside;
/// minting a key there would manufacture the very confidence it is asking
/// about.
///
/// # Why anything needs this
///
/// The key that reaches the tray is usually read straight out of `.env`
/// ([`crate::install::env_key_from_path`]), and the keychain is consulted only
/// when `.env` has none. So "a key resolved" says nothing about whether that
/// key *owns* the database - a stale or hand-edited `.env` resolves perfectly
/// and opens nothing.
///
/// That distinction is invisible at the point it matters, because SQLCipher
/// reports a wrong key and a damaged page 1 identically (code 26, `file is not
/// a database`). Treating the two the same means a wrong `.env` key can be
/// read as unrecoverable corruption and a *healthy encrypted database* moved
/// aside for a fresh one.
///
/// Returns `false` when the keychain has no entry or cannot be reached: this
/// gates a destructive action, so "cannot prove it" must read the same as
/// "no".
pub(crate) fn key_matches_keychain(key_hex: &str) -> bool {
    let entry = match keyring::Entry::new(SERVICE, ACCOUNT) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, "db_key: could not reach the OS keychain to verify the database key");
            return false;
        }
    };
    match entry.get_password() {
        Ok(stored) => stored == key_hex,
        Err(keyring::Error::NoEntry) => false,
        Err(e) => {
            tracing::warn!(error = %e, "db_key: could not read the stored database key for verification");
            false
        }
    }
}

/// Look up this machine's `meridian.db` encryption key WITHOUT minting one.
///
/// Returns `None` when the keychain has no entry, cannot be reached, or holds
/// a malformed value — every one of those means the same thing to the caller:
/// there is no key to run the decrypt migration with, either because this
/// install was never encrypted or because it already went through the
/// migration and had its key removed (see [`remove_key_from_keychain`]).
/// Unlike the retired `resolve_or_create_key`, nothing here ever calls
/// `set_password` — a missing entry is not an occasion to generate one.
pub fn resolve_existing_key() -> Option<String> {
    let entry = match keyring::Entry::new(SERVICE, ACCOUNT) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, "db_key: could not reach the OS keychain to look up the database key");
            return None;
        }
    };
    match entry.get_password() {
        Ok(key_hex) => match meridian_core::db_crypto::validate_key_hex(&key_hex) {
            Ok(()) => Some(key_hex),
            Err(e) => {
                tracing::warn!(error = %e, "db_key: key stored in the OS keychain is malformed - ignoring it");
                None
            }
        },
        Err(keyring::Error::NoEntry) => None,
        Err(e) => {
            tracing::warn!(error = %e, "db_key: could not read the stored database key from the OS keychain");
            None
        }
    }
}

/// Remove this machine's `meridian.db` encryption key from the OS keychain.
///
/// Called once [`meridian_core::db_crypto::decrypt_in_place`] has finished, so
/// no later launch finds a key for a file that is plaintext again. Best-effort
/// and idempotent: a missing entry is not an error, and a failure to reach the
/// keychain just leaves a harmless orphaned entry — nothing reads it once
/// `MERIDIAN_DB_KEY` is also gone from `.env` (the caller's job, via
/// [`crate::commands::integrations::strip_env_keys`]).
pub fn remove_key_from_keychain() {
    let entry = match keyring::Entry::new(SERVICE, ACCOUNT) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, "db_key: could not reach the OS keychain to remove the database key");
            return;
        }
    };
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(e) => {
            tracing::warn!(error = %e, "db_key: failed to remove the database key from the OS keychain");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_existing_db_is_absent_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("meridian.db");
        assert!(!db_path.exists());
        assert_eq!(classify_existing_db(&db_path), ExistingDb::Absent);
    }

    #[test]
    fn classify_existing_db_is_plaintext_when_plaintext() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("meridian.db");
        std::fs::write(&db_path, b"SQLite format 3\0rest-of-a-plaintext-file").unwrap();
        assert_eq!(classify_existing_db(&db_path), ExistingDb::Plaintext);
    }

    #[test]
    fn classify_existing_db_looks_encrypted_when_not_plaintext() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("meridian.db");
        // No recognizable SQLite header - stands in for SQLCipher ciphertext.
        std::fs::write(&db_path, b"not-a-sqlite-header-at-all-just-ciphertext").unwrap();
        assert_eq!(classify_existing_db(&db_path), ExistingDb::LooksEncrypted);
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
    }
}
