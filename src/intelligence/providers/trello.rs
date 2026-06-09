// meridian — normalises screenpipe activity into structured app sessions
//
// Trello card connector. Pulls open cards where the authenticated user is a
// member (GET /1/members/me/cards) into pm_tasks so the classifier can link
// sessions to them and the worklog driver can draft against them.
//
// Auth: key + token as query params — Trello's standard pattern.
// task_key stored as the card shortLink (stable 8-char alphanumeric identifier).
// If TRELLO_BOARD_IDS is set, only cards from those boards are kept.

use anyhow::{Context, Result};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::config::TrelloConfig;
use crate::intelligence::oauth::trello as oauth_trello;

const TRELLO_BASE: &str = "https://api.trello.com/1";
const MAX_RESULTS: usize = 100;
const SYNC_INTERVAL_MINS: i64 = 5;

// ---------------------------------------------------------------------------
// API response shapes
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct TrelloCard {
    #[serde(rename = "shortLink")]
    short_link: String,
    name: String,
    #[serde(default)]
    desc: String,
    #[serde(rename = "idBoard")]
    id_board: String,
    #[serde(rename = "dateLastActivity", default)]
    date_last_activity: String,
    #[serde(rename = "shortUrl", default)]
    short_url: String,
    #[serde(default)]
    closed: bool,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn board_allowed(card: &TrelloCard, board_ids: &[String]) -> bool {
    if board_ids.is_empty() {
        return true;
    }
    board_ids.iter().any(|b| b == &card.id_board)
}

// ---------------------------------------------------------------------------
// Fetch
// ---------------------------------------------------------------------------

#[tracing::instrument(
    skip(trello),
    fields(provider = "trello", status_code = tracing::field::Empty)
)]
async fn fetch(trello: &TrelloConfig) -> Result<Vec<TrelloCard>> {
    let token = oauth_trello::load_token().context("loading Trello OAuth token")?;
    let url = format!(
        "{TRELLO_BASE}/members/me/cards\
         ?filter=open\
         &fields=shortLink,name,desc,idBoard,dateLastActivity,shortUrl,closed\
         &limit={MAX_RESULTS}\
         &key={}&token={}",
        trello.app_key, token,
    );

    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await
        .context("GET Trello /members/me/cards")?;

    let status = resp.status();
    tracing::Span::current().record("status_code", status.as_u16() as i64);
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("Trello API → {}: {}", status, text);
    }

    let cards: Vec<TrelloCard> =
        serde_json::from_str(&text).context("deserialising Trello cards")?;
    tracing::debug!(count = cards.len(), "parsed Trello response");
    Ok(cards)
}

// ---------------------------------------------------------------------------
// Upsert
// ---------------------------------------------------------------------------

async fn upsert(
    pool: &SqlitePool,
    cards: &[TrelloCard],
    trello: &TrelloConfig,
) -> Result<Vec<String>> {
    let mut kept: Vec<String> = Vec::new();
    for card in cards {
        if card.closed || !board_allowed(card, &trello.board_ids) {
            continue;
        }
        sqlx::query(
            "INSERT INTO pm_tasks
               (task_key, provider, title, description_text, status_category,
                issue_type, project_key, url, updated_at, fetched_at)
             VALUES (?, 'trello', ?, ?, 'in_progress', 'Card', ?, ?,
                     ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
             ON CONFLICT(task_key) DO UPDATE SET
               provider         = 'trello',
               title            = excluded.title,
               description_text = excluded.description_text,
               status_category  = excluded.status_category,
               project_key      = excluded.project_key,
               url              = excluded.url,
               updated_at       = excluded.updated_at,
               fetched_at       = excluded.fetched_at",
        )
        .bind(&card.short_link)
        .bind(&card.name)
        .bind(&card.desc)
        .bind(&card.id_board)
        .bind(&card.short_url)
        .bind(&card.date_last_activity)
        .execute(pool)
        .await
        .with_context(|| format!("upserting Trello card {}", card.short_link))?;

        kept.push(card.short_link.clone());
    }
    Ok(kept)
}

// ---------------------------------------------------------------------------
// Prune
// ---------------------------------------------------------------------------

async fn prune(pool: &SqlitePool, fetched_keys: &[String]) -> Result<usize> {
    let placeholders = fetched_keys
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");

    let emb_sql = format!(
        "DELETE FROM pm_task_embeddings WHERE task_key IN \
         (SELECT task_key FROM pm_tasks WHERE provider = 'trello' AND task_key NOT IN ({placeholders}))"
    );
    let mut q = sqlx::query(&emb_sql);
    for key in fetched_keys {
        q = q.bind(key.as_str());
    }
    q.execute(pool)
        .await
        .context("pruning trello pm_task_embeddings")?;

    let task_sql = format!(
        "DELETE FROM pm_tasks WHERE provider = 'trello' AND task_key NOT IN ({placeholders})"
    );
    let mut q = sqlx::query(&task_sql);
    for key in fetched_keys {
        q = q.bind(key.as_str());
    }
    let result = q.execute(pool).await.context("pruning trello pm_tasks")?;
    Ok(result.rows_affected() as usize)
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

#[tracing::instrument(skip(pool, trello))]
pub async fn refresh_if_stale(
    pool: &SqlitePool,
    trello: &TrelloConfig,
) -> Result<Option<Vec<String>>> {
    let threshold = format!("-{SYNC_INTERVAL_MINS} minutes");
    let (is_fresh,): (i64,) = sqlx::query_as(
        "SELECT EXISTS(
             SELECT 1 FROM pm_sync_state
             WHERE provider = 'trello'
               AND last_synced_at > strftime('%Y-%m-%dT%H:%M:%SZ', 'now', ?)
         )",
    )
    .bind(&threshold)
    .fetch_one(pool)
    .await
    .context("checking trello sync state")?;

    if is_fresh != 0 {
        return Ok(None);
    }

    match fetch(trello).await {
        Ok(cards) => {
            let raw_count = cards.len();
            let kept = upsert(pool, &cards, trello).await?;
            let n = kept.len();
            sqlx::query(
                "INSERT INTO pm_sync_state (provider, last_synced_at)
                 VALUES ('trello', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                 ON CONFLICT(provider) DO UPDATE SET last_synced_at = excluded.last_synced_at",
            )
            .execute(pool)
            .await
            .context("updating trello sync state")?;

            if raw_count < MAX_RESULTS {
                if !kept.is_empty() {
                    match prune(pool, &kept).await {
                        Ok(0) => {}
                        Ok(p) => tracing::info!(pruned_count = p, "pruned stale trello tasks"),
                        Err(e) => tracing::warn!(error = %e, "trello prune failed"),
                    }
                } else if let Err(e) = sqlx::query("DELETE FROM pm_tasks WHERE provider = 'trello'")
                    .execute(pool)
                    .await
                {
                    tracing::warn!(error = %e, "trello full-clear failed");
                }
            }
            tracing::info!(upserted_count = n, "trello tasks refreshed");
            Ok(Some(kept))
        }
        Err(e) => {
            tracing::warn!(error = %e, "trello fetch failed — keeping stale cache");
            Ok(None)
        }
    }
}

/// Force an immediate Trello sync regardless of the staleness gate.
pub async fn force_refresh(
    pool: &SqlitePool,
    trello: &TrelloConfig,
) -> Result<Option<Vec<String>>> {
    sqlx::query("DELETE FROM pm_sync_state WHERE provider = 'trello'")
        .execute(pool)
        .await
        .context("clearing trello sync state for force refresh")?;
    refresh_if_stale(pool, trello).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_filter_empty_allows_all() {
        let card = TrelloCard {
            short_link: "abc".into(),
            name: "T".into(),
            desc: String::new(),
            id_board: "board1".into(),
            date_last_activity: String::new(),
            short_url: String::new(),
            closed: false,
        };
        assert!(board_allowed(&card, &[]));
    }

    #[test]
    fn board_filter_matches_id() {
        let card = TrelloCard {
            short_link: "abc".into(),
            name: "T".into(),
            desc: String::new(),
            id_board: "board1".into(),
            date_last_activity: String::new(),
            short_url: String::new(),
            closed: false,
        };
        assert!(board_allowed(&card, &["board1".to_string()]));
        assert!(!board_allowed(&card, &["board2".to_string()]));
    }

    #[test]
    fn parses_card_response() {
        let raw = r#"[
            {"shortLink":"HSkL1pnj","name":"Fix bug","desc":"details",
             "idBoard":"b1","dateLastActivity":"2026-06-01T10:00:00.000Z",
             "shortUrl":"https://trello.com/c/HSkL1pnj","closed":false}
        ]"#;
        let cards: Vec<TrelloCard> = serde_json::from_str(raw).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].short_link, "HSkL1pnj");
        assert!(!cards[0].closed);
    }
}
