//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Coding-agent indexer + summariser, ported from the Python services
// (the former Python indexer, the former Python summariser) into the
// daemon. Spawned as gated tokio tasks from main.rs: the indexer turns
// Claude/Codex JSONLs into app_sessions segment rows; the summariser turns
// sealed segments into prose summaries; both stay dormant without a coding
// agent present. CLI subcommands (`coding-agent-hook`, `coding-agent-summarise`)
// run one-shot against the same DB.

pub mod cursor_agent_init;
pub mod db;
pub mod hook;
pub mod indexer;
pub mod jsonl;
pub mod segment;
pub mod sources;
pub mod summariser;

use std::path::PathBuf;

pub use segment::{
    iso_utc, norm_iso, parse_iso, parse_session_segments, Segment, SegmentParams, SessionMeta,
};

/// Path to the meridian DB (MERIDIAN_DB env, default `~/.meridian/meridian.db`).
pub fn meridian_db_path() -> PathBuf {
    let raw =
        std::env::var("MERIDIAN_DB").unwrap_or_else(|_| "~/.meridian/meridian.db".to_string());
    PathBuf::from(shellexpand::tilde(&raw).into_owned())
}

/// Open a short-lived pool against the meridian DB (the daemon already created +
/// migrated it; we never migrate here). Used by the one-shot CLI subcommands.
pub async fn open_meridian_pool() -> anyhow::Result<sqlx::SqlitePool> {
    let path = meridian_db_path();
    let uri = format!("sqlite://{}", path.display());
    let key = std::env::var("MERIDIAN_DB_KEY").ok();
    open_meridian_pool_at(&uri, key.as_deref()).await
}

/// The env-free half of [`open_meridian_pool`], so the key handling is testable
/// without mutating process-global environment variables.
///
/// Goes through [`meridian_core::db_crypto::open_pool_with_key`] — the same
/// opener the daemon uses — rather than building the pool by hand. That helper
/// applies the SQLCipher key as the first statement on every new connection,
/// and its `key_unless_plaintext` guard drops the key when the file on disk is
/// plaintext, so a dev/source install (no key, or a key left over from a
/// half-finished `encrypt_in_place`) keeps working unchanged.
///
/// `create_if_missing` is false: the daemon owns creation and migrations, and a
/// one-shot CLI that silently conjured an empty DB would report a nonsense
/// "0 pending" instead of an error.
pub async fn open_meridian_pool_at(
    uri: &str,
    key_hex: Option<&str>,
) -> anyhow::Result<sqlx::SqlitePool> {
    meridian_core::db_crypto::open_pool_with_key(uri, key_hex, false, &[]).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_KEY: &str = "3a15689f73412c29d7ed3b902a01e33dbd5f767dc37b792e19ac9e2366bf2cd2";

    /// The one-shot CLI subcommands (`coding-agent-summarise`,
    /// `coding-agent-hook`) must be able to read an ENCRYPTED meridian.db.
    ///
    /// Regression: this pool was built with a raw `SqlitePool::connect_with`,
    /// which never applies the SQLCipher key, so every query failed on a
    /// packaged install — surfacing as the unactionable
    /// `summarise: ensure column: check summary_source column`. The daemon's own
    /// summariser was unaffected (it uses the keyed opener), so the only casualty
    /// was the CLI — which is the ONLY documented way to drain the historical
    /// summariser backlog, since `drain()` deliberately sweeps yesterday+today.
    #[tokio::test]
    async fn opens_an_encrypted_db() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("meridian.db");
        let uri = db_path.display().to_string();

        // Seed an encrypted DB the way the daemon would.
        let seed = meridian_core::db_crypto::open_pool_with_key(&uri, Some(TEST_KEY), true, &[])
            .await
            .unwrap();
        sqlx::query("CREATE TABLE app_sessions (id INTEGER PRIMARY KEY, summary_source TEXT)")
            .execute(&seed)
            .await
            .unwrap();
        seed.close().await;

        // The CLI opener must read it back. Bounded, because a keyless open of an
        // encrypted file does not fail fast — it stalls until the acquire timeout.
        let pool = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            open_meridian_pool_at(&uri, Some(TEST_KEY)),
        )
        .await
        .expect("opening an encrypted db must not stall")
        .expect("the CLI pool must apply the SQLCipher key");

        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('app_sessions') WHERE name = 'summary_source'",
        )
        .fetch_one(&pool)
        .await
        .expect("the exact query cli_summarise fails on today");
        assert_eq!(n, 1);
        pool.close().await;
    }

    /// Neither one-shot CLI may hand-build its pool again.
    ///
    /// The behavioural test above only covers `open_meridian_pool_at`. This
    /// covers the regression CLASS: `hook.rs` carried an independent copy of the
    /// same raw `SqlitePool::connect_with`, and because the hook exits 0 on every
    /// path it failed in total silence. A unit test cannot reach it (it reads
    /// stdin and the real `MERIDIAN_DB`), so scan the source — the same tactic as
    /// the tray's cfg audit and the UI's `no-native-dialogs` test.
    #[test]
    fn one_shot_clis_do_not_hand_build_a_pool() {
        for (name, src) in [
            ("hook.rs", include_str!("hook.rs")),
            ("mod.rs", include_str!("mod.rs")),
        ] {
            // Scan production code only — this test's own assertion strings
            // contain the very pattern being banned, so a whole-file scan would
            // match itself and fail for the wrong reason.
            let src = src.split("#[cfg(test)]").next().unwrap_or(src);
            let offenders = src
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .filter(|l| {
                    l.contains("SqlitePool::connect_with")
                        || l.contains("SqlitePool::connect(")
                        || l.contains("connect_lazy_with")
                })
                .collect::<Vec<_>>();
            assert!(
                offenders.is_empty(),
                "{name} builds a SQLite pool directly, which skips the SQLCipher key \
                 — route it through open_meridian_pool/open_meridian_pool_at instead. \
                 Offending lines: {offenders:?}"
            );
        }
        // And the seam really does delegate to the keyed opener.
        assert!(
            include_str!("mod.rs").contains("db_crypto::open_pool_with_key("),
            "open_meridian_pool_at must go through meridian_core::db_crypto::open_pool_with_key"
        );
    }
}
