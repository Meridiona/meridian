// meridian — normalises screenpipe activity into structured app sessions

use anyhow::Result;
use sqlx::SqlitePool;

use crate::config::LinearConfig;

pub async fn refresh_if_stale(
    _pool: &SqlitePool,
    _linear: &LinearConfig,
) -> Result<Option<Vec<String>>> {
    // TODO: implement Linear connector
    Ok(None)
}
