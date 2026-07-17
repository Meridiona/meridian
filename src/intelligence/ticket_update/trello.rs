//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Trello write-back. task_key is the card shortLink. Auth is key+token query
// params (Trello's standard). Due date, member (assign self), name, and desc are
// straightforward card edits we apply in-app. Priority/estimate/parent have
// no native Trello concept, and "close" is list-semantics that vary per board —
// those redirect to the card.
//
// Reference: https://developer.atlassian.com/cloud/trello/rest/api-group-cards/

use anyhow::{bail, Context, Result};

use super::statuses::{resolve_choice, SetStatusResult, StatusList, StatusOption};
use super::{ApplyResult, WriteField};
use crate::config::TrelloConfig;
use crate::intelligence::oauth::trello as oauth_trello;
use crate::pm_worklog::trello::parse_short_link;

const TRELLO_BASE: &str = "https://api.trello.com/1";

pub async fn apply(cfg: &TrelloConfig, key: &str, write: &WriteField) -> Result<ApplyResult> {
    let short_link = parse_short_link(key)?;
    let card_url = format!("https://trello.com/c/{short_link}");

    // Fields with no clean Trello mapping → redirect to the card.
    match write {
        WriteField::Priority(_)
        | WriteField::StoryPoints(_)
        | WriteField::Parent(_)
        | WriteField::Close
        | WriteField::Cancel
        | WriteField::Reopen => {
            return Ok(ApplyResult::redirected(
                "trello",
                key,
                field_name(write),
                card_url,
                "set this directly on the Trello card",
            ));
        }
        _ => {}
    }

    let token = oauth_trello::load_token().context("loading Trello OAuth token")?;
    let client = reqwest::Client::new();

    match write {
        WriteField::DueDate(date) => {
            // Trello wants a datetime; anchor a date to mid-afternoon UTC.
            let due = format!("{date}T17:00:00.000Z");
            put_card(&client, cfg, &token, &short_link, &[("due", due.as_str())]).await?;
        }
        WriteField::AssignMe => {
            let me = my_member_id(&client, cfg, &token).await?;
            // idMembers add endpoint is additive. Use append_pair for correct URL encoding.
            let mut url =
                reqwest::Url::parse(&format!("{TRELLO_BASE}/cards/{short_link}/idMembers"))
                    .context("building Trello idMembers URL")?;
            url.query_pairs_mut()
                .append_pair("key", &cfg.app_key)
                .append_pair("token", &token)
                .append_pair("value", &me);
            post(&client, url.as_str()).await?;
        }
        WriteField::Summary(text) => {
            put_card(
                &client,
                cfg,
                &token,
                &short_link,
                &[("name", text.as_str())],
            )
            .await?;
        }
        WriteField::Description(text) => {
            put_card(
                &client,
                cfg,
                &token,
                &short_link,
                &[("desc", text.as_str())],
            )
            .await?;
        }
        _ => unreachable!("redirected above"),
    }

    Ok(ApplyResult::applied("trello", key, field_name(write)))
}

/// List the card's board's lists (Trello's "statuses") + the card's current
/// list. Trello lists have no lifecycle category, so every option is `unknown`.
#[tracing::instrument(skip(cfg))]
pub(super) async fn list_statuses(cfg: &TrelloConfig, key: &str) -> Result<StatusList> {
    let short_link = parse_short_link(key)?;
    let token = oauth_trello::load_token().context("loading Trello OAuth token")?;
    let client = reqwest::Client::new();

    // Current list + board for the card.
    let card_url = format!(
        "{TRELLO_BASE}/cards/{short_link}?fields=idBoard,idList&key={}&token={}",
        cfg.app_key, token
    );
    let card = get_json(&client, &card_url).await.context("GET card")?;
    let id_board = card
        .get("idBoard")
        .and_then(|b| b.as_str())
        .context("card missing idBoard")?;
    let current_id = card
        .get("idList")
        .and_then(|l| l.as_str())
        .map(String::from);

    // All lists on the board.
    let lists_url = format!(
        "{TRELLO_BASE}/boards/{id_board}/lists?fields=id,name&key={}&token={}",
        cfg.app_key, token
    );
    let lists = get_json(&client, &lists_url)
        .await
        .context("GET board lists")?;
    let statuses = list_options(&lists);
    let current_name = current_id.as_deref().and_then(|cid| {
        statuses
            .iter()
            .find(|o| o.id == cid)
            .map(|o| o.name.clone())
    });
    tracing::debug!(key, statuses = statuses.len(), "trello status list");

    Ok(StatusList {
        statuses,
        current_id,
        current_name,
    })
}

/// Move the card to the chosen list (id or name, case-insensitive) via
/// `PUT /cards/{id}` with `idList`. Errors if the choice matches no list.
#[tracing::instrument(skip(cfg))]
pub(super) async fn set_status(
    cfg: &TrelloConfig,
    key: &str,
    choice: &str,
) -> Result<SetStatusResult> {
    let short_link = parse_short_link(key)?;
    let token = oauth_trello::load_token().context("loading Trello OAuth token")?;
    let client = reqwest::Client::new();

    // Resolve the choice against the board's lists.
    let card_url = format!(
        "{TRELLO_BASE}/cards/{short_link}?fields=idBoard&key={}&token={}",
        cfg.app_key, token
    );
    let card = get_json(&client, &card_url).await.context("GET card")?;
    let id_board = card
        .get("idBoard")
        .and_then(|b| b.as_str())
        .context("card missing idBoard")?;
    let lists_url = format!(
        "{TRELLO_BASE}/boards/{id_board}/lists?fields=id,name&key={}&token={}",
        cfg.app_key, token
    );
    let lists = get_json(&client, &lists_url)
        .await
        .context("GET board lists")?;
    let options = list_options(&lists);
    let target = resolve_choice(&options, choice)
        .with_context(|| format!("no Trello list matches {choice:?}"))?
        .clone();

    put_card(
        &client,
        cfg,
        &token,
        &short_link,
        &[("idList", target.id.as_str())],
    )
    .await?;
    Ok(SetStatusResult::applied(target))
}

/// Board lists → status options. Trello has no lifecycle category → `unknown`.
fn list_options(lists: &serde_json::Value) -> Vec<StatusOption> {
    lists
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|l| {
                    let id = l.get("id")?.as_str()?.to_string();
                    let name = l.get("name")?.as_str()?.to_string();
                    Some(StatusOption {
                        id,
                        name,
                        category: "unknown".to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn get_json(client: &reqwest::Client, url: &str) -> Result<serde_json::Value> {
    let resp = client.get(url).send().await.context("GET Trello")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("Trello GET returned {status}: {text}");
    }
    serde_json::from_str(&text).context("parsing Trello response")
}

async fn put_card(
    client: &reqwest::Client,
    cfg: &TrelloConfig,
    token: &str,
    short_link: &str,
    params: &[(&str, &str)],
) -> Result<()> {
    let mut url = reqwest::Url::parse(&format!("{TRELLO_BASE}/cards/{short_link}"))
        .context("building Trello card URL")?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("key", &cfg.app_key);
        q.append_pair("token", token);
        for (k, v) in params {
            q.append_pair(k, v);
        }
    }
    let resp = client
        .put(url.clone())
        .send()
        .await
        .context("PUT Trello card")?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        bail!("Trello PUT card {short_link} returned {status}: {text}");
    }
    Ok(())
}

async fn post(client: &reqwest::Client, url: &str) -> Result<()> {
    let resp = client.post(url).send().await.context("POST Trello")?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        bail!("Trello POST returned {status}: {text}");
    }
    Ok(())
}

async fn my_member_id(client: &reqwest::Client, cfg: &TrelloConfig, token: &str) -> Result<String> {
    let url = format!(
        "{TRELLO_BASE}/members/me?key={}&token={}&fields=id",
        cfg.app_key, token
    );
    let resp = client.get(&url).send().await.context("GET /members/me")?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("Trello /members/me returned {status}: {text}");
    }
    let v: serde_json::Value = serde_json::from_str(&text).context("parsing /members/me")?;
    v.get("id")
        .and_then(|i| i.as_str())
        .map(String::from)
        .context("/members/me missing id")
}

fn field_name(write: &WriteField) -> &'static str {
    match write {
        WriteField::DueDate(_) => "duedate",
        WriteField::AssignMe => "assignee",
        WriteField::Priority(_) => "priority",
        WriteField::StoryPoints(_) => "story_points",
        WriteField::Parent(_) => "parent",
        WriteField::Summary(_) => "summary",
        WriteField::Description(_) => "description",
        WriteField::Close => "close",
        WriteField::Cancel => "cancel",
        WriteField::Reopen => "reopen",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_name_round_trips() {
        let cases: &[(&str, WriteField)] = &[
            ("duedate", WriteField::DueDate("2026-01-01".into())),
            ("assignee", WriteField::AssignMe),
            ("priority", WriteField::Priority("High".into())),
            ("story_points", WriteField::StoryPoints(3.0)),
            ("parent", WriteField::Parent("abc".into())),
            ("summary", WriteField::Summary("t".into())),
            ("description", WriteField::Description("d".into())),
            ("close", WriteField::Close),
            ("cancel", WriteField::Cancel),
            ("reopen", WriteField::Reopen),
        ];
        for (expected, field) in cases {
            assert_eq!(
                field_name(field),
                *expected,
                "field_name mismatch for {expected}"
            );
        }
    }

    // Priority / story_points / parent / close / cancel are redirected for
    // Trello (no native concept). These WriteField variants must still PARSE — if
    // parse returned None, the dispatch would use the "unwritable field" redirect
    // path with a wrong reason string instead of the Trello-specific one.
    #[test]
    fn trello_redirect_fields_parse_correctly() {
        assert!(WriteField::parse("priority", "High").is_some());
        assert!(WriteField::parse("story_points", "3").is_some());
        assert!(WriteField::parse("parent", "abc").is_some());
        assert!(WriteField::parse("close", "").is_some());
        assert!(WriteField::parse("cancel", "").is_some());
        assert!(WriteField::parse("reopen", "").is_some());
    }

    #[test]
    fn builds_list_options_as_unknown_category() {
        let lists = serde_json::json!([
            { "id": "l1", "name": "To Do" },
            { "id": "l2", "name": "Doing" },
        ]);
        let opts = list_options(&lists);
        assert_eq!(opts.len(), 2);
        assert_eq!(opts[0].id, "l1");
        assert_eq!(opts[0].name, "To Do");
        // Trello lists carry no lifecycle signal.
        assert_eq!(opts[0].category, "unknown");
        assert_eq!(opts[1].category, "unknown");
    }

    // These three are the fields Trello DOES write in-app — regression guard.
    #[test]
    fn trello_applicable_fields_parse() {
        assert!(WriteField::parse("duedate", "2026-06-30").is_some());
        assert!(WriteField::parse("assignee", "@me").is_some());
        assert!(WriteField::parse("summary", "New name").is_some());
        assert!(WriteField::parse("description", "New desc").is_some());
    }
}
