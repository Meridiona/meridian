//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

pub mod azure_devops;
pub mod github;
pub mod jira;
pub mod linear;
pub mod status;
pub mod trello;

use anyhow::Result;
use sqlx::SqlitePool;

/// Write a sync error for a provider. Writes to both `pm_sync_state.last_error`
/// (for the connect-status indicators) and `system_notices` (for the global
/// UI fault bus that surfaces banners on every page).
pub async fn stamp_sync_error(pool: &SqlitePool, provider: &str, error: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO pm_sync_state (provider, last_synced_at, last_error)
         VALUES (?, '1970-01-01T00:00:00Z', ?)
         ON CONFLICT(provider) DO UPDATE SET last_error = excluded.last_error",
    )
    .bind(provider)
    .bind(error)
    .execute(pool)
    .await?;

    let (title, remedy): (&str, Option<&str>) = match provider {
        "jira"         => ("Jira sync failing",        Some("Set JIRA_API_TOKEN and JIRA_BASE_URL in .env")),
        "linear"       => ("Linear sync failing",       Some("Set LINEAR_API_KEY in .env")),
        "trello"       => ("Trello sync failing",       Some("Run: meridian oauth-login trello")),
        "github"       => ("GitHub sync failing",       Some("Set GITHUB_TOKEN in .env")),
        "azure_devops" => ("Azure DevOps sync failing", Some("Set AZURE_DEVOPS_PAT in .env")),
        _              => ("PM sync failing",            None),
    };
    let _ = crate::notices::raise(
        pool,
        &format!("pm.{provider}"),
        "error",
        title,
        error,
        remedy,
    )
    .await;
    Ok(())
}

/// Clear the last error for a provider after a successful sync.
pub async fn clear_sync_error(pool: &SqlitePool, provider: &str) -> Result<()> {
    sqlx::query(
        "UPDATE pm_sync_state SET last_error = NULL WHERE provider = ?",
    )
    .bind(provider)
    .execute(pool)
    .await?;
    let _ = crate::notices::clear(pool, &format!("pm.{provider}")).await;
    Ok(())
}
