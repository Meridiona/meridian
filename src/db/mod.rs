//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
// https://github.com/meridiona/meridian

pub mod cli;
pub mod integrity;
pub mod meridian;
pub mod repair;
pub mod screenpipe;

// Fixtures that build a genuinely byte-corrupt database on disk. Test-only:
// `integrity` and `repair` cannot be exercised against `sqlite::memory:`,
// which has no file to damage.
#[cfg(test)]
pub mod test_corrupt;

// Re-export the pool type so consumers (e.g. the Tauri tray) can name it as
// `meridian::db::SqlitePool` without adding `sqlx` to their own Cargo.toml —
// keeps a single sqlx version across the workspace.
pub use sqlx::SqlitePool;
