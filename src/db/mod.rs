//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
// https://github.com/meridiona/meridian

pub mod meridian;
pub mod screenpipe;

// Re-export the pool type so consumers (e.g. the Tauri tray) can name it as
// `meridian::db::SqlitePool` without adding `sqlx` to their own Cargo.toml —
// keeps a single sqlx version across the workspace.
pub use sqlx::SqlitePool;
