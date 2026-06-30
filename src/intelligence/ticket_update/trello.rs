//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Trello write-back. task_key is the card shortLink. Auth is key+token query
// params (Trello's standard). Due date, member (assign self), name, and desc are
// straightforward card edits we apply in-app. Labels are board-scoped (resolving
// a label name needs the board's label catalogue), priority/estimate/parent have
// no native Trello concept, and "close" is list-semantics that vary per board —
// those redirect to the card.
//
// Reference: https://developer.atlassian.com/cloud/trello/rest/api-group-cards/

use anyhow::{bail, Context, Result};

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
        WriteField::AddLabel(_)
        | WriteField::Priority(_)
        | WriteField::StoryPoints(_)
        | WriteField::Parent(_)
        | WriteField::Close
        | WriteField::Cancel => {
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
        WriteField::AddLabel(_) => "labels",
        WriteField::Priority(_) => "priority",
        WriteField::StoryPoints(_) => "story_points",
        WriteField::Parent(_) => "parent",
        WriteField::Summary(_) => "summary",
        WriteField::Description(_) => "description",
        WriteField::Close => "close",
        WriteField::Cancel => "cancel",
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
            ("labels", WriteField::AddLabel("bug".into())),
            ("priority", WriteField::Priority("High".into())),
            ("story_points", WriteField::StoryPoints(3.0)),
            ("parent", WriteField::Parent("abc".into())),
            ("summary", WriteField::Summary("t".into())),
            ("description", WriteField::Description("d".into())),
            ("close", WriteField::Close),
            ("cancel", WriteField::Cancel),
        ];
        for (expected, field) in cases {
            assert_eq!(
                field_name(field),
                *expected,
                "field_name mismatch for {expected}"
            );
        }
    }

    // Labels / priority / story_points / parent / close / cancel are redirected for
    // Trello (no native concept). These WriteField variants must still PARSE — if
    // parse returned None, the dispatch would use the "unwritable field" redirect
    // path with a wrong reason string instead of the Trello-specific one.
    #[test]
    fn trello_redirect_fields_parse_correctly() {
        assert!(WriteField::parse("labels", "bug").is_some());
        assert!(WriteField::parse("priority", "High").is_some());
        assert!(WriteField::parse("story_points", "3").is_some());
        assert!(WriteField::parse("parent", "abc").is_some());
        assert!(WriteField::parse("close", "").is_some());
        assert!(WriteField::parse("cancel", "").is_some());
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
