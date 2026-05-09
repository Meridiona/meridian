// meridian — normalises screenpipe activity into structured app sessions

use anyhow::Result;
use sqlx::SqlitePool;

use crate::config::GitHubConfig;

pub async fn refresh_if_stale(_pool: &SqlitePool, _github: &GitHubConfig) -> Result<()> {
    // TODO: implement GitHub Issues connector
    Ok(())
}
