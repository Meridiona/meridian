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

/// Open a short-lived pool against this machine's `meridian.db`, for the
/// one-shot CLI subcommands (`coding-agent-hook`, `coding-agent-summarise`).
///
/// Resolves the path from `MERIDIAN_DB` via [`meridian_db_path`] and the
/// SQLCipher key from `MERIDIAN_DB_KEY`, then defers to
/// [`open_meridian_pool_at`] - which is where the behaviour worth knowing
/// lives. In particular:
///
/// - **The database is never created.** `create_if_missing` is false, so a
///   missing file is an error rather than an empty database. The daemon owns
///   creation and migrations; a CLI that conjured one would report a nonsense
///   "0 pending" instead of saying the database is not there.
/// - **A missing key is not an error.** An absent `MERIDIAN_DB_KEY` is normal
///   on a dev/source install, and the keyed opener drops the key anyway when
///   the file on disk is plaintext. It only fails later, on first use, when
///   the file *is* encrypted - and not quickly (see the timeout note on
///   [`open_meridian_pool_at`]).
///
/// Returns a pool the caller should treat as short-lived: these are one-shot
/// commands, and the daemon is usually holding the same file.
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
///
/// # Parameters and result
///
/// - `uri` - a full `sqlite://<path>` URI, not a bare path. Callers build it
///   from [`meridian_db_path`]; the shape is exercised directly by
///   `opens_an_encrypted_db`.
/// - `key_hex` - the 64-hex-char SQLCipher key, or `None` on an install with
///   no encryption. `None` against an *encrypted* file does not fail fast:
///   the pool builds, and the first query stalls until the acquire timeout
///   (~10 s). Callers on a latency budget must impose their own deadline -
///   [`crate::coding_agent_session_ingest::hook`] does exactly this, because
///   it runs inside Claude's SessionEnd hook.
/// - Returns a pool whose connections have already had the key applied; an
///   `Err` only for a malformed URI or a missing file.
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
        // The `sqlite://` form `open_meridian_pool` actually builds, so the test
        // exercises the production string shape rather than a bare path.
        let uri = format!("sqlite://{}", db_path.display());

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
    /// The other half of the compatibility claim: a dev/source install must be
    /// unchanged by this fix.
    ///
    /// That claim rests entirely on `open_pool_with_key`'s `key_unless_plaintext`
    /// guard, and nothing else here exercises it. This is a real configuration,
    /// not a contrived one: a dev machine with `MERIDIAN_DB_KEY` still set in a
    /// leftover `.env` while `meridian.db` on disk is not encrypted (also the
    /// state a half-finished `encrypt_in_place` leaves behind). Without the
    /// guard, SQLCipher stalls on the first pragma until the acquire timeout
    /// rather than failing fast — so this asserts a bounded, successful read.
    #[tokio::test]
    async fn opens_a_plaintext_db_even_with_a_key_configured() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("meridian.db");
        let uri = format!("sqlite://{}", db_path.display());

        // Seed WITHOUT a key — a plaintext file.
        let seed = meridian_core::db_crypto::open_pool_with_key(&uri, None, true, &[])
            .await
            .unwrap();
        sqlx::query("CREATE TABLE app_sessions (id INTEGER PRIMARY KEY, summary_source TEXT)")
            .execute(&seed)
            .await
            .unwrap();
        seed.close().await;

        // Open it WITH a key configured. Must drop the key and succeed.
        let pool = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            open_meridian_pool_at(&uri, Some(TEST_KEY)),
        )
        .await
        .expect("a plaintext file with a key set must not stall")
        .expect("the key must be dropped for a plaintext file");

        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('app_sessions') WHERE name = 'summary_source'",
        )
        .fetch_one(&pool)
        .await
        .expect("dev/source installs must keep working");
        assert_eq!(n, 1);
        pool.close().await;
    }

    /// Production half of a source file — everything before its test module.
    ///
    /// Both checks below must scan ONLY this. The test module names the very
    /// patterns being banned (and calls the keyed opener itself, to seed a
    /// fixture), so a whole-file scan either matches itself and fails for the
    /// wrong reason, or passes on its own text no matter what production code
    /// does. `split` always yields at least one item, hence the bare `next()`.
    fn production_half(src: &str) -> &str {
        src.split("#[cfg(test)]").next().expect("split yields one")
    }

    /// The premise behind `hook::DB_OPEN_BUDGET`: a keyless open of an
    /// ENCRYPTED database does not fail fast, it stalls.
    ///
    /// This is the misconfiguration the SessionEnd hook is most likely to meet
    /// (the key never reached the CLI's environment), and if it errored
    /// promptly the timeout wrapper would be pointless. Pinned because the
    /// wrapper's justification rests entirely on this behaviour - if a future
    /// sqlx or db_crypto change made it fail fast, the budget could be dropped,
    /// and if it silently got LONGER the hook would be blocking Claude again.
    #[tokio::test]
    async fn a_keyless_open_of_an_encrypted_db_stalls_rather_than_erroring() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("meridian.db");
        let uri = format!("sqlite://{}", db_path.display());

        let seed = meridian_core::db_crypto::open_pool_with_key(&uri, Some(TEST_KEY), true, &[])
            .await
            .unwrap();
        sqlx::query("CREATE TABLE app_sessions (id INTEGER PRIMARY KEY)")
            .execute(&seed)
            .await
            .unwrap();
        seed.close().await;

        // Deliberately short: this asserts "did NOT resolve quickly", so the
        // bound only has to be well under the acquire timeout. Keeping it at
        // 1 s costs the suite a second rather than the full stall.
        let stalled = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            open_meridian_pool_at(&uri, None),
        )
        .await;
        assert!(
            stalled.is_err(),
            "a keyless open of an encrypted database is expected to stall - if              it now fails fast, hook::DB_OPEN_BUDGET is protecting nothing and              its rationale needs rewriting"
        );
    }

    /// The ban list must reject every pool constructor that skips the key, not
    /// just the ones that happened to be tried. `connect_lazy` was the hole:
    /// it takes the URI directly like `connect(`, and nothing matched it.
    #[test]
    fn the_ban_list_covers_the_lazy_constructor() {
        let banned = |l: &str| {
            l.contains("connect_with(")
                || l.contains("connect_lazy_with(")
                || l.contains("SqlitePool::connect(")
                || l.contains("SqlitePool::connect_lazy(")
        };
        assert!(
            banned("    let pool = SqlitePool::connect_lazy(&uri)?;"),
            "SqlitePool::connect_lazy skips the SQLCipher key and must be banned"
        );
        assert!(banned("SqlitePool::connect(&uri).await?"));
        assert!(banned("SqlitePoolOptions::new().connect_with(opts).await?"));
        assert!(
            !banned("open_meridian_pool_at(&uri, key).await?"),
            "the sanctioned opener must not be caught by the ban list"
        );
    }

    #[test]
    fn one_shot_clis_do_not_hand_build_a_pool() {
        for (name, src) in [
            ("hook.rs", include_str!("hook.rs")),
            ("mod.rs", include_str!("mod.rs")),
        ] {
            let src = production_half(src);
            // Bare-substring bans, deliberately. The first version of this test
            // matched only `SqlitePool::connect_with`, which let the MOST likely
            // regression straight through: `SqlitePoolOptions::new()…
            // .connect_with(opts)` — the shape you land on by copying the
            // daemon's own pool construction (`db_crypto`, `db/meridian.rs`) and
            // forgetting the key. Verified: with such a pool live in hook.rs the
            // old assertions still passed. Post-fix neither file legitimately
            // calls any of these, so banning the bare method names is safe.
            let offenders = src
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .filter(|l| {
                    l.contains("connect_with(")
                        || l.contains("connect_lazy_with(")
                        || l.contains("SqlitePool::connect(")
                        // `connect_lazy` was the hole this ban list still had:
                        // it takes the URI directly, so it skips the key setup
                        // exactly like `connect(` does, and nothing here
                        // matched it. The lazy variant is the easier one to
                        // reach for in a CLI, too - it does not need an await
                        // at construction.
                        || l.contains("SqlitePool::connect_lazy(")
                })
                .collect::<Vec<_>>();
            assert!(
                offenders.is_empty(),
                "{name} builds a SQLite pool directly, which skips the SQLCipher key \
                 — route it through open_meridian_pool/open_meridian_pool_at instead. \
                 Offending lines: {offenders:?}"
            );
        }
        // And the seam really does delegate to the keyed opener. Scanned against
        // the production half for the same reason: `opens_an_encrypted_db` calls
        // `db_crypto::open_pool_with_key` to seed its fixture, so a whole-file
        // read finds this substring even if `open_meridian_pool_at` stops
        // delegating entirely.
        assert!(
            production_half(include_str!("mod.rs")).contains("db_crypto::open_pool_with_key("),
            "open_meridian_pool_at must go through meridian_core::db_crypto::open_pool_with_key"
        );
    }
}
