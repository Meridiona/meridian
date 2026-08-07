//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Unit tests for the generated-worklog ledger. The schema here is hand-rolled
//! rather than migrated (this crate has no migration runner) — it must mirror
//! migrations 060 + 062 (after 062's DROP COLUMNs) + 065's create_attempt_at.

use super::*;
use sqlx::sqlite::SqlitePoolOptions;

async fn seeded() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(
        "CREATE TABLE day_task_worklogs (\
            day_local TEXT NOT NULL, task_id TEXT NOT NULL, provider TEXT NOT NULL, \
            propose_issue_type TEXT, propose_title TEXT, propose_description TEXT, \
            update_summary TEXT NOT NULL DEFAULT '', update_json TEXT NOT NULL DEFAULT '{}', \
            reasoning TEXT NOT NULL DEFAULT '', state TEXT NOT NULL DEFAULT 'drafted', \
            created_task_key TEXT, last_error TEXT, create_attempt_at TEXT, \
            created_at TEXT NOT NULL, updated_at TEXT NOT NULL, \
            drafted_minutes INTEGER, \
            PRIMARY KEY (day_local, task_id))",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TABLE day_task_worklog_targets (\
            day_local TEXT NOT NULL, task_id TEXT NOT NULL, task_key TEXT NOT NULL, \
            provider TEXT NOT NULL, confidence REAL NOT NULL DEFAULT 0, \
            manual INTEGER NOT NULL DEFAULT 0, position INTEGER NOT NULL DEFAULT 0, \
            posted_comment_id TEXT, browse_url TEXT, posted_at TEXT, last_error TEXT, \
            post_attempt_at TEXT, created_at TEXT NOT NULL, update_json TEXT, \
            PRIMARY KEY (day_local, task_id, task_key))",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("CREATE TABLE pm_tasks (task_key TEXT PRIMARY KEY, title TEXT NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    // Mirrors migrations 058 + 059, less the columns none of this reads. The
    // staleness join needs `minutes`; `title` is what the notifier names.
    sqlx::query(
        "CREATE TABLE day_tasks (\
            day_local TEXT NOT NULL, task_id TEXT NOT NULL, title TEXT NOT NULL, \
            minutes INTEGER NOT NULL DEFAULT 0, \
            PRIMARY KEY (day_local, task_id))",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool
}

/// Put a task on the day with `minutes` measured against it.
async fn put_task(pool: &SqlitePool, task_id: &str, minutes: i64) {
    sqlx::query(
        "INSERT INTO day_tasks (day_local, task_id, title, minutes) VALUES ('2026-08-07', ?, ?, ?) \
         ON CONFLICT(day_local, task_id) DO UPDATE SET minutes = excluded.minutes",
    )
    .bind(task_id)
    .bind(format!("Task {task_id}"))
    .bind(minutes)
    .execute(pool)
    .await
    .unwrap();
}

fn target(key: &str, confidence: f64) -> TargetInput {
    TargetInput {
        task_key: key.into(),
        provider: "jira".into(),
        confidence,
        manual: false,
        update: None,
    }
}

/// A target carrying its own per-ticket update (the multi-match body split).
fn target_with_update(key: &str, summary: &str) -> TargetInput {
    TargetInput {
        task_key: key.into(),
        provider: "jira".into(),
        confidence: 0.9,
        manual: false,
        update: Some(GeneratedWorklogUpdate {
            summary: summary.into(),
            ..Default::default()
        }),
    }
}

fn match_upsert() -> DraftUpsert {
    DraftUpsert {
        provider: "jira".into(),
        targets: vec![target("KAN-12", 0.86)],
        propose: None,
        update: GeneratedWorklogUpdate {
            summary: "Wired the thing".into(),
            sections: vec![WorklogSection {
                heading: "Decisions".into(),
                points: vec!["Chose X".into()],
            }],
            status: "In progress".into(),
        },
        reasoning: "clear advance".into(),
    }
}

fn propose_upsert() -> DraftUpsert {
    DraftUpsert {
        provider: "jira".into(),
        targets: vec![],
        propose: Some(GeneratedWorklogPropose {
            issue_type: "Task".into(),
            title: "Do the thing".into(),
            description: "It needs doing".into(),
        }),
        update: GeneratedWorklogUpdate {
            summary: "Did the thing".into(),
            ..Default::default()
        },
        reasoning: "unplanned work".into(),
    }
}

fn keys(d: &DayTaskWorklogDraft) -> Vec<&str> {
    d.targets.iter().map(|t| t.task_key.as_str()).collect()
}

// ── The target set ────────────────────────────────────────────────────────────

/// The core of the multi-match change: one strand of a day's work routinely
/// advances more than one planned task, and all of them get the update.
#[tokio::test]
async fn a_draft_holds_every_ticket_the_model_matched() {
    let pool = seeded().await;
    let mut up = match_upsert();
    up.targets = vec![target("KAN-1", 0.9), target("KAN-2", 0.82)];

    let d = upsert_draft(&pool, "2026-07-16", "T1", up, "t0")
        .await
        .unwrap();

    assert_eq!(keys(&d), vec!["KAN-1", "KAN-2"]);
    assert!(d.propose.is_none());
    assert!(d.targets.iter().all(|t| !t.posted));
}

/// The multi-match body split: when one day-task advances two tickets, each target
/// carries its OWN update, and both round-trip through the DB distinctly.
#[tokio::test]
async fn each_target_keeps_its_own_update() {
    let pool = seeded().await;
    let mut up = match_upsert();
    up.targets = vec![
        target_with_update("KAN-1", "Wired the auth refresh"),
        target_with_update("KAN-2", "Fixed the settings save race"),
    ];

    let d = upsert_draft(&pool, "2026-07-16", "T1", up, "t0")
        .await
        .unwrap();

    assert_eq!(keys(&d), vec!["KAN-1", "KAN-2"]);
    assert_eq!(
        d.targets[0].update.as_ref().unwrap().summary,
        "Wired the auth refresh"
    );
    assert_eq!(
        d.targets[1].update.as_ref().unwrap().summary,
        "Fixed the settings save race"
    );
}

/// A target with no per-ticket update reads back `None`, so the poster falls back
/// to the draft-level update (the propose branch, a manual retarget, pre-070 rows).
#[tokio::test]
async fn a_target_without_its_own_update_falls_back() {
    let pool = seeded().await;
    upsert_draft(&pool, "2026-07-16", "T1", match_upsert(), "t0")
        .await
        .unwrap();
    let d = get_day_task_worklog(&pool, "2026-07-16", "T1")
        .await
        .unwrap()
        .unwrap();
    assert!(d.targets[0].update.is_none());
}

/// The deterministic adherence signal: every ticket a day's drafted worklogs
/// matched, grouped by ticket, from drafts alone (no approval needed).
#[tokio::test]
async fn matched_tickets_for_day_groups_drafts_by_ticket() {
    let pool = seeded().await;

    // T1's worklog advanced KAN-1 and KAN-2; T2's advanced KAN-1 as well.
    let mut a = match_upsert();
    a.targets = vec![target("KAN-1", 0.9), target("KAN-2", 0.8)];
    upsert_draft(&pool, "2026-07-16", "T1", a, "t0")
        .await
        .unwrap();
    let mut b = match_upsert();
    b.targets = vec![target("KAN-1", 0.7)];
    upsert_draft(&pool, "2026-07-16", "T2", b, "t0")
        .await
        .unwrap();
    // A different day must not bleed in.
    let mut c = match_upsert();
    c.targets = vec![target("KAN-9", 0.9)];
    upsert_draft(&pool, "2026-07-17", "T9", c, "t0")
        .await
        .unwrap();

    let m = targets::matched_tickets_for_day(&pool, "2026-07-16")
        .await
        .unwrap();

    assert_eq!(m.len(), 2, "two distinct tickets matched on the day");
    let mut kan1 = m.get("KAN-1").cloned().unwrap();
    kan1.sort();
    assert_eq!(kan1, vec!["T1", "T2"], "both day-tasks that matched KAN-1");
    assert_eq!(m.get("KAN-2").unwrap(), &vec!["T1"]);
    assert!(!m.contains_key("KAN-9"), "another day's match stays out");
}

/// Drafted OR posted both count - the match is the row's existence, not its
/// delivery. A `propose` draft (new ticket, no target) contributes nothing.
#[tokio::test]
async fn matched_tickets_ignores_propose_only_drafts() {
    let pool = seeded().await;
    upsert_draft(&pool, "2026-07-16", "T1", propose_upsert(), "t0")
        .await
        .unwrap();
    let m = targets::matched_tickets_for_day(&pool, "2026-07-16")
        .await
        .unwrap();
    assert!(m.is_empty(), "a propose branch has no matched ticket");
}

/// Model order is the order the panel lists them in, so the strongest match reads
/// first. `ORDER BY position` and not by key, which would sort KAN-10 above KAN-9.
#[tokio::test]
async fn targets_keep_the_model_s_order() {
    let pool = seeded().await;
    let mut up = match_upsert();
    up.targets = vec![target("KAN-9", 0.95), target("KAN-10", 0.8)];

    let d = upsert_draft(&pool, "2026-07-16", "T1", up, "t0")
        .await
        .unwrap();
    assert_eq!(keys(&d), vec!["KAN-9", "KAN-10"]);
}

/// Dismiss is per-ticket: the model got one of three wrong, and the user drops it
/// without discarding the update or paying for a regenerate.
#[tokio::test]
async fn dismiss_drops_one_ticket_and_keeps_the_rest() {
    let pool = seeded().await;
    let mut up = match_upsert();
    up.targets = vec![
        target("KAN-1", 0.9),
        target("KAN-2", 0.82),
        target("KAN-3", 0.8),
    ];
    upsert_draft(&pool, "2026-07-16", "T1", up, "t0")
        .await
        .unwrap();

    let d = dismiss_target(&pool, "2026-07-16", "T1", "KAN-2")
        .await
        .unwrap();

    assert_eq!(keys(&d), vec!["KAN-1", "KAN-3"]);
    assert_eq!(
        d.update.summary, "Wired the thing",
        "dismiss must not touch the written update"
    );
}

/// Dismissing the last ticket is allowed. It leaves a draft with nothing to post,
/// which is honest - the user can pick a ticket or regenerate. Silently refusing
/// would leave a ticket the user explicitly rejected in the post set.
#[tokio::test]
async fn dismissing_the_last_ticket_leaves_an_empty_draft() {
    let pool = seeded().await;
    upsert_draft(&pool, "2026-07-16", "T1", match_upsert(), "t0")
        .await
        .unwrap();

    let d = dismiss_target(&pool, "2026-07-16", "T1", "KAN-12")
        .await
        .unwrap();
    assert!(d.targets.is_empty());
    assert!(d.propose.is_none());
}

/// A posted comment cannot be un-posted. Deleting the target would not remove the
/// comment from the tracker - it would only make Meridian forget it is there, and
/// the next approve would post a second copy.
#[tokio::test]
async fn a_posted_ticket_cannot_be_dismissed() {
    let pool = seeded().await;
    let mut up = match_upsert();
    up.targets = vec![target("KAN-1", 0.9), target("KAN-2", 0.8)];
    upsert_draft(&pool, "2026-07-16", "T1", up, "t0")
        .await
        .unwrap();
    mark_approved(&pool, "2026-07-16", "T1", "t1")
        .await
        .unwrap();
    mark_posted(&pool, "2026-07-16", "T1", "KAN-1", "c1", None, "t2")
        .await
        .unwrap();

    assert!(
        dismiss_target(&pool, "2026-07-16", "T1", "KAN-1")
            .await
            .is_err(),
        "an approved row is closed to edits"
    );
    let d = get_day_task_worklog(&pool, "2026-07-16", "T1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(keys(&d), vec!["KAN-1", "KAN-2"], "nothing was removed");
}

// ── Partial delivery ──────────────────────────────────────────────────────────

/// The reason targets is a table and not a JSON array. Two of three post, one
/// fails; the row must NOT read `posted`, or the user loses the retry that gets
/// the third one.
#[tokio::test]
async fn a_partly_posted_draft_stays_retryable() {
    let pool = seeded().await;
    let mut up = match_upsert();
    up.targets = vec![target("KAN-1", 0.9), target("KAN-2", 0.8)];
    upsert_draft(&pool, "2026-07-16", "T1", up, "t0")
        .await
        .unwrap();
    mark_approved(&pool, "2026-07-16", "T1", "t1")
        .await
        .unwrap();

    mark_posted(
        &pool,
        "2026-07-16",
        "T1",
        "KAN-1",
        "c1",
        Some("http://x"),
        "t2",
    )
    .await
    .unwrap();
    targets::mark_error(&pool, "2026-07-16", "T1", "KAN-2", "jira said no")
        .await
        .unwrap();

    let d = get_day_task_worklog(&pool, "2026-07-16", "T1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d.state, "approved", "one target still pending - not posted");
    let one = d.targets.iter().find(|t| t.task_key == "KAN-1").unwrap();
    assert!(one.posted);
    assert_eq!(one.browse_url.as_deref(), Some("http://x"));
    let two = d.targets.iter().find(|t| t.task_key == "KAN-2").unwrap();
    assert!(!two.posted, "the retry must know to post to this one");
    assert_eq!(two.error.as_deref(), Some("jira said no"));
}

/// The row goes `posted` only on the LAST target - that is what makes `posted`
/// mean "delivered everywhere" rather than "delivered somewhere".
#[tokio::test]
async fn the_row_posts_only_once_every_ticket_has_landed() {
    let pool = seeded().await;
    let mut up = match_upsert();
    up.targets = vec![target("KAN-1", 0.9), target("KAN-2", 0.8)];
    upsert_draft(&pool, "2026-07-16", "T1", up, "t0")
        .await
        .unwrap();
    mark_approved(&pool, "2026-07-16", "T1", "t1")
        .await
        .unwrap();

    mark_posted(&pool, "2026-07-16", "T1", "KAN-1", "c1", None, "t2")
        .await
        .unwrap();
    let d = get_day_task_worklog(&pool, "2026-07-16", "T1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d.state, "approved");

    mark_posted(&pool, "2026-07-16", "T1", "KAN-2", "c2", None, "t3")
        .await
        .unwrap();
    let d = get_day_task_worklog(&pool, "2026-07-16", "T1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d.state, "posted");
    assert!(d.targets.iter().all(|t| t.posted));
}

/// "Still working on this" after a post: reopening flips a `posted` row back to
/// `drafted` so a fresh regenerate can overwrite it, and the follow-up draft
/// replaces the targets with unposted ones (the old comment stays on the tracker).
#[tokio::test]
async fn reopen_posted_lets_a_posted_row_be_regenerated() {
    let pool = seeded().await;
    upsert_draft(&pool, "2026-07-16", "T1", match_upsert(), "t0")
        .await
        .unwrap();
    mark_approved(&pool, "2026-07-16", "T1", "t1")
        .await
        .unwrap();
    mark_posted(&pool, "2026-07-16", "T1", "KAN-12", "c1", None, "t2")
        .await
        .unwrap();
    let d = get_day_task_worklog(&pool, "2026-07-16", "T1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d.state, "posted");
    assert!(d.targets[0].posted);

    // A drafted regenerate BEFORE reopening is a no-op (the guard protects posted).
    upsert_draft(&pool, "2026-07-16", "T1", match_upsert(), "t3")
        .await
        .unwrap();
    let d = get_day_task_worklog(&pool, "2026-07-16", "T1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d.state, "posted", "upsert must not overwrite a posted row");
    assert!(d.targets[0].posted, "the live comment's record survives");

    // Reopen, then the regenerate lands: state is drafted and the target is fresh.
    assert!(reopen_posted(&pool, "2026-07-16", "T1", "t4")
        .await
        .unwrap());
    upsert_draft(&pool, "2026-07-16", "T1", match_upsert(), "t5")
        .await
        .unwrap();
    let d = get_day_task_worklog(&pool, "2026-07-16", "T1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d.state, "drafted");
    assert!(
        !d.targets[0].posted,
        "the follow-up draft's target is unposted - approve posts a new comment"
    );

    // Reopening a row that is no longer posted is a harmless no-op.
    assert!(!reopen_posted(&pool, "2026-07-16", "T1", "t6")
        .await
        .unwrap());
}

// ── The post claim (migration 063) ────────────────────────────────────────────

/// THE crash-window guard. A process that dies between `post_comment` returning
/// and `mark_posted` committing leaves a live comment nothing recorded. The claim
/// is what remembers the attempt, so the retry refuses instead of posting a second
/// copy onto someone's board.
#[tokio::test]
async fn a_claim_left_unresolved_reads_as_outcome_unknown() {
    let pool = seeded().await;
    upsert_draft(&pool, "2026-07-16", "T1", match_upsert(), "t0")
        .await
        .unwrap();
    mark_approved(&pool, "2026-07-16", "T1", "t1")
        .await
        .unwrap();

    assert!(
        targets::begin_post(&pool, "2026-07-16", "T1", "KAN-12", "t2")
            .await
            .unwrap(),
        "a fresh target can be claimed"
    );
    // ...and here the process dies. Nothing resolves the claim.

    let d = get_day_task_worklog(&pool, "2026-07-16", "T1")
        .await
        .unwrap()
        .unwrap();
    assert!(d.targets[0].outcome_unknown, "the attempt is on the books");
    assert!(!d.targets[0].posted, "and we never learned that it landed");
    assert_ne!(d.state, "posted", "the row must not read as delivered");

    // The retry: the same CAS that guards a concurrent approve also refuses this.
    assert!(
        !targets::begin_post(&pool, "2026-07-16", "T1", "KAN-12", "t3")
            .await
            .unwrap(),
        "re-claiming an unresolved target must fail - that is the double-post guard"
    );
}

/// A returned error means nothing was posted, so the claim is released and a plain
/// retry works. Only a CRASH is unrecoverable - a definite failure must not strand
/// the ticket forever.
#[tokio::test]
async fn a_definite_failure_releases_the_claim_for_a_safe_retry() {
    let pool = seeded().await;
    upsert_draft(&pool, "2026-07-16", "T1", match_upsert(), "t0")
        .await
        .unwrap();
    mark_approved(&pool, "2026-07-16", "T1", "t1")
        .await
        .unwrap();

    assert!(
        targets::begin_post(&pool, "2026-07-16", "T1", "KAN-12", "t2")
            .await
            .unwrap()
    );
    targets::revert_post(&pool, "2026-07-16", "T1", "KAN-12")
        .await
        .unwrap();

    let d = get_day_task_worklog(&pool, "2026-07-16", "T1")
        .await
        .unwrap()
        .unwrap();
    assert!(
        !d.targets[0].outcome_unknown,
        "the attempt resolved: it failed"
    );
    assert!(
        targets::begin_post(&pool, "2026-07-16", "T1", "KAN-12", "t3")
            .await
            .unwrap(),
        "a released target is claimable again"
    );
}

/// Releasing must never touch a target whose comment is live — that would erase
/// the record of a posted comment and let the next approve post a second one.
#[tokio::test]
async fn releasing_cannot_resurrect_a_posted_target() {
    let pool = seeded().await;
    upsert_draft(&pool, "2026-07-16", "T1", match_upsert(), "t0")
        .await
        .unwrap();
    mark_approved(&pool, "2026-07-16", "T1", "t1")
        .await
        .unwrap();
    targets::begin_post(&pool, "2026-07-16", "T1", "KAN-12", "t2")
        .await
        .unwrap();
    mark_posted(&pool, "2026-07-16", "T1", "KAN-12", "c1", None, "t3")
        .await
        .unwrap();

    targets::revert_post(&pool, "2026-07-16", "T1", "KAN-12")
        .await
        .unwrap();

    let d = get_day_task_worklog(&pool, "2026-07-16", "T1")
        .await
        .unwrap()
        .unwrap();
    assert!(d.targets[0].posted, "still posted");
    assert!(
        !targets::begin_post(&pool, "2026-07-16", "T1", "KAN-12", "t4")
            .await
            .unwrap(),
        "a posted target can never be claimed again"
    );
}

/// A successful post resolves the claim, so `outcome_unknown` means exactly one
/// thing: nobody knows. It must not linger on a ticket that plainly succeeded.
#[tokio::test]
async fn a_successful_post_resolves_its_claim() {
    let pool = seeded().await;
    upsert_draft(&pool, "2026-07-16", "T1", match_upsert(), "t0")
        .await
        .unwrap();
    mark_approved(&pool, "2026-07-16", "T1", "t1")
        .await
        .unwrap();
    targets::begin_post(&pool, "2026-07-16", "T1", "KAN-12", "t2")
        .await
        .unwrap();
    mark_posted(&pool, "2026-07-16", "T1", "KAN-12", "c1", None, "t3")
        .await
        .unwrap();

    let d = get_day_task_worklog(&pool, "2026-07-16", "T1")
        .await
        .unwrap()
        .unwrap();
    assert!(!d.targets[0].outcome_unknown);
    assert_eq!(d.state, "posted");
}

/// One unresolved ticket must not hold the others hostage: the two that landed
/// stay posted, and the row stays out of `posted` so the user still sees it.
#[tokio::test]
async fn an_unknown_ticket_does_not_block_its_siblings() {
    let pool = seeded().await;
    let mut up = match_upsert();
    up.targets = vec![target("KAN-1", 0.9), target("KAN-2", 0.8)];
    upsert_draft(&pool, "2026-07-16", "T1", up, "t0")
        .await
        .unwrap();
    mark_approved(&pool, "2026-07-16", "T1", "t1")
        .await
        .unwrap();

    targets::begin_post(&pool, "2026-07-16", "T1", "KAN-1", "t2")
        .await
        .unwrap();
    mark_posted(&pool, "2026-07-16", "T1", "KAN-1", "c1", None, "t3")
        .await
        .unwrap();
    // KAN-2's attempt starts and the process dies.
    targets::begin_post(&pool, "2026-07-16", "T1", "KAN-2", "t4")
        .await
        .unwrap();

    let d = get_day_task_worklog(&pool, "2026-07-16", "T1")
        .await
        .unwrap()
        .unwrap();
    assert!(d.targets[0].posted && !d.targets[0].outcome_unknown);
    assert!(!d.targets[1].posted && d.targets[1].outcome_unknown);
    assert_eq!(d.state, "approved", "not delivered everywhere");
}

// ── Retarget ──────────────────────────────────────────────────────────────────

/// The escape hatch for the matcher's plan-only scope: unplanned work comes back
/// as a proposal, and the user redirects it at a real ticket. The proposal must be
/// CLEARED, or approve would create a ticket AND comment on the picked one.
#[tokio::test]
async fn retarget_turns_a_proposal_into_a_user_chosen_match() {
    let pool = seeded().await;
    upsert_draft(&pool, "2026-07-16", "T1", propose_upsert(), "t0")
        .await
        .unwrap();

    let d = retarget_draft(&pool, "2026-07-16", "T1", "KAN-7", "jira", "t1")
        .await
        .unwrap();

    assert!(d.propose.is_none(), "the proposal must be dropped");
    assert_eq!(keys(&d), vec!["KAN-7"]);
    assert!(d.targets[0].manual, "the user picked this, not the model");
    assert_eq!(
        d.update.summary, "Did the thing",
        "retarget must NOT rewrite the update - it only changes where it lands"
    );
}

/// A user reaching for the picker is correcting the model, not extending it - so
/// the pick REPLACES every ticket the model chose. Adding to them would silently
/// post to tickets the user was in the middle of overriding.
#[tokio::test]
async fn retarget_collapses_a_multi_match_onto_the_one_picked_ticket() {
    let pool = seeded().await;
    let mut up = match_upsert();
    up.targets = vec![target("KAN-1", 0.9), target("KAN-2", 0.8)];
    upsert_draft(&pool, "2026-07-16", "T1", up, "t0")
        .await
        .unwrap();

    let d = retarget_draft(&pool, "2026-07-16", "T1", "KAN-7", "jira", "t1")
        .await
        .unwrap();
    assert_eq!(keys(&d), vec!["KAN-7"]);
}

/// An approved/posted row is human-owned and already delivered. Silently
/// re-pointing one would leave the comment on a ticket the row no longer names.
#[tokio::test]
async fn retarget_refuses_an_approved_draft() {
    let pool = seeded().await;
    upsert_draft(&pool, "2026-07-16", "T1", match_upsert(), "t0")
        .await
        .unwrap();
    mark_approved(&pool, "2026-07-16", "T1", "t1")
        .await
        .unwrap();

    assert!(
        retarget_draft(&pool, "2026-07-16", "T1", "KAN-7", "jira", "t2")
            .await
            .is_err(),
        "an approved worklog must not be silently retargeted"
    );
}

/// Regenerate must clear the manual flag: the new answer is the model's, and
/// carrying it over would credit the user for a choice the model just made.
#[tokio::test]
async fn regenerate_clears_the_manual_flag() {
    let pool = seeded().await;
    upsert_draft(&pool, "2026-07-16", "T1", propose_upsert(), "t0")
        .await
        .unwrap();
    let d = retarget_draft(&pool, "2026-07-16", "T1", "KAN-7", "jira", "t1")
        .await
        .unwrap();
    assert!(d.targets[0].manual);

    let d = upsert_draft(&pool, "2026-07-16", "T1", match_upsert(), "t2")
        .await
        .unwrap();
    assert!(
        !d.targets[0].manual,
        "a fresh model answer is not the user's pick"
    );
    assert_eq!(keys(&d), vec!["KAN-12"]);
}

// ── Lifecycle ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn none_when_absent_and_pre_060() {
    // Absent row.
    let pool = seeded().await;
    assert!(get_day_task_worklog(&pool, "2026-07-16", "T1")
        .await
        .unwrap()
        .is_none());
    // No table at all (pre-060 DB) degrades to None, not an error.
    let bare = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    assert!(get_day_task_worklog(&bare, "2026-07-16", "T1")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn upsert_roundtrips_a_match_draft() {
    let pool = seeded().await;
    sqlx::query("INSERT INTO pm_tasks (task_key, title) VALUES ('KAN-12', 'Daily task planner for solo users')")
        .execute(&pool)
        .await
        .unwrap();
    let d = upsert_draft(&pool, "2026-07-16", "T1", match_upsert(), "t0")
        .await
        .unwrap();
    assert_eq!(d.state, "drafted");
    assert_eq!(d.provider, "jira");
    assert_eq!(d.targets.len(), 1);
    assert_eq!(d.targets[0].task_key, "KAN-12");
    assert_eq!(d.targets[0].confidence, 0.86);
    assert_eq!(
        d.targets[0].task_title.as_deref(),
        Some("Daily task planner for solo users"),
        "title hydrated from pm_tasks via the LEFT JOIN"
    );
    assert!(d.propose.is_none());
    assert_eq!(d.update.sections[0].heading, "Decisions");
    assert_eq!(d.update.sections[0].points, vec!["Chose X"]);
    assert!(d.error.is_none());
}

/// The created ticket becomes an ordinary target, so the posting loop has exactly
/// one thing to read whichever branch the draft took.
#[tokio::test]
async fn an_approved_proposal_becomes_the_draft_s_target() {
    let pool = seeded().await;
    upsert_draft(&pool, "2026-07-16", "T1", propose_upsert(), "t0")
        .await
        .unwrap();
    mark_approved(&pool, "2026-07-16", "T1", "t1")
        .await
        .unwrap();
    mark_created(&pool, "2026-07-16", "T1", "KAN-99", "jira", "t2")
        .await
        .unwrap();

    let d = get_day_task_worklog(&pool, "2026-07-16", "T1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d.created_task_key.as_deref(), Some("KAN-99"));
    assert_eq!(keys(&d), vec!["KAN-99"]);
    assert!(
        d.targets[0].manual,
        "the user approved this proposal - there is no model score to show"
    );
}

/// `mark_created` is a two-step write (record the key, then add the target), and
/// the approve path re-runs it on every attempt so a crash between the steps
/// repairs itself. Re-running must therefore be a no-op, not a duplicate or a
/// reset - and it must not resurrect a target that has since been posted.
#[tokio::test]
async fn mark_created_is_safe_to_re_run() {
    let pool = seeded().await;
    upsert_draft(&pool, "2026-07-16", "T1", propose_upsert(), "t0")
        .await
        .unwrap();
    mark_approved(&pool, "2026-07-16", "T1", "t1")
        .await
        .unwrap();
    mark_created(&pool, "2026-07-16", "T1", "KAN-99", "jira", "t2")
        .await
        .unwrap();
    mark_posted(&pool, "2026-07-16", "T1", "KAN-99", "c1", None, "t3")
        .await
        .unwrap();

    // The retry that a partially-failed approve would make.
    mark_created(&pool, "2026-07-16", "T1", "KAN-99", "jira", "t4")
        .await
        .unwrap();

    let d = get_day_task_worklog(&pool, "2026-07-16", "T1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(keys(&d), vec!["KAN-99"], "no duplicate target");
    assert!(
        d.targets[0].posted,
        "re-running must not clear the live comment's record - that would post a second copy"
    );
}

#[tokio::test]
async fn upsert_overwrites_drafted_only() {
    let pool = seeded().await;
    upsert_draft(&pool, "2026-07-16", "T1", match_upsert(), "t0")
        .await
        .unwrap();

    // A regenerate over a still-drafted row overwrites it (flip to a proposal).
    let d = upsert_draft(&pool, "2026-07-16", "T1", propose_upsert(), "t1")
        .await
        .unwrap();
    assert!(d.targets.is_empty());
    assert_eq!(d.propose.as_ref().unwrap().title, "Do the thing");

    // Now approve+post the row, then attempt a regenerate — it MUST be preserved.
    mark_approved(&pool, "2026-07-16", "T1", "t2")
        .await
        .unwrap();
    mark_created(&pool, "2026-07-16", "T1", "KAN-99", "jira", "t3")
        .await
        .unwrap();
    mark_posted(
        &pool,
        "2026-07-16",
        "T1",
        "KAN-99",
        "c1",
        Some("http://x"),
        "t4",
    )
    .await
    .unwrap();
    let d = upsert_draft(&pool, "2026-07-16", "T1", match_upsert(), "t5")
        .await
        .unwrap();
    assert_eq!(d.state, "posted", "posted row must not be clobbered");
    assert_eq!(
        keys(&d),
        vec!["KAN-99"],
        "the live comment's ticket must survive the regenerate - forgetting it \
         would post a second copy on the next approve"
    );
    assert_eq!(d.targets[0].posted_comment_id.as_deref(), Some("c1"));
    assert_eq!(d.targets[0].browse_url.as_deref(), Some("http://x"));
}

#[tokio::test]
async fn mark_error_is_state_preserving() {
    let pool = seeded().await;
    upsert_draft(&pool, "2026-07-16", "T1", match_upsert(), "t0")
        .await
        .unwrap();
    mark_error(&pool, "2026-07-16", "T1", "boom", "t1")
        .await
        .unwrap();
    let d = get_day_task_worklog(&pool, "2026-07-16", "T1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d.state, "drafted");
    assert_eq!(d.error.as_deref(), Some("boom"));
}

// ── The update wire shape ─────────────────────────────────────────────────────

#[test]
fn legacy_decisions_architecture_rows_still_read_back() {
    // A row written before `sections` existed: the fixed developer-shaped keys.
    // Serde ignores unknown fields, so without the UpdateWire shim these bullets
    // vanished on read and the detail panel showed only summary + status, while
    // the comment already posted to the ticket still had them.
    let legacy = r#"{
        "summary": "Built out the Workstream Builder pipeline",
        "decisions": ["Group hours by shared goal", "Rewrite each summary fresh"],
        "architecture": ["Timeline UI renders workstreams as cards"],
        "status": "In progress"
    }"#;
    let u: GeneratedWorklogUpdate = serde_json::from_str(legacy).unwrap();
    assert_eq!(u.summary, "Built out the Workstream Builder pipeline");
    assert_eq!(u.status, "In progress");
    assert_eq!(u.sections.len(), 2, "both legacy groups lifted");
    assert_eq!(u.sections[0].heading, "Decisions");
    assert_eq!(u.sections[0].points.len(), 2);
    assert_eq!(u.sections[1].heading, "Architecture");
    assert_eq!(
        u.sections[1].points,
        vec!["Timeline UI renders workstreams as cards"]
    );

    // Re-serializing emits the CURRENT shape — the legacy keys don't propagate.
    let json = serde_json::to_string(&u).unwrap();
    assert!(json.contains("sections"));
    assert!(!json.contains("\"decisions\""));
}

#[test]
fn legacy_lift_skips_empty_groups_and_never_overrides_sections() {
    // An empty/absent legacy group must not become a headed-but-empty section.
    let u: GeneratedWorklogUpdate =
        serde_json::from_str(r#"{"summary":"s","decisions":[],"architecture":["  "],"status":""}"#)
            .unwrap();
    assert!(u.sections.is_empty(), "no bullets, no sections");

    // A new-shape row wins outright — legacy keys are not consulted.
    let u: GeneratedWorklogUpdate = serde_json::from_str(
        r#"{"summary":"s","sections":[{"heading":"Edits","points":["cut intro"]}],
            "decisions":["stale"],"status":"WIP"}"#,
    )
    .unwrap();
    assert_eq!(u.sections.len(), 1);
    assert_eq!(u.sections[0].heading, "Edits");
}

// ── begin_create / revert_create: the ticket-create CAS (finding #2b) ─────────

/// Stand up an approved proposal row (no target ticket yet, created_task_key NULL) -
/// the exact state approve() reaches before it would call create_ticket.
async fn approved_proposal() -> SqlitePool {
    let pool = seeded().await;
    upsert_draft(&pool, "2026-07-16", "T1", propose_upsert(), "t0")
        .await
        .unwrap();
    mark_approved(&pool, "2026-07-16", "T1", "t1")
        .await
        .unwrap();
    pool
}

/// #2b: begin_create is a CAS, so two racing approves both try to claim the create
/// but only one wins - create_ticket fires exactly once. The loser is refused by the
/// same predicate a post-crash retry hits. (stale_before is BEFORE the claim, so the
/// second call can't reclaim it.)
#[tokio::test]
async fn begin_create_hands_the_create_to_exactly_one_caller() {
    let pool = approved_proposal().await;
    let claimed = begin_create(
        &pool,
        "2026-07-16",
        "T1",
        "2026-07-16T10:00:00+00:00",
        "2026-07-16T09:00:00+00:00",
    )
    .await
    .unwrap();
    assert!(claimed, "the first approve owns the create");

    let second = begin_create(
        &pool,
        "2026-07-16",
        "T1",
        "2026-07-16T10:00:01+00:00",
        "2026-07-16T09:00:00+00:00",
    )
    .await
    .unwrap();
    assert!(
        !second,
        "a concurrent approve is refused - only one real ticket gets filed"
    );
}

/// A DEFINITE create failure releases the claim so a later retry may try again - the
/// create analog of revert_post.
#[tokio::test]
async fn revert_create_frees_the_claim_for_a_retry() {
    let pool = approved_proposal().await;
    assert!(begin_create(
        &pool,
        "2026-07-16",
        "T1",
        "2026-07-16T10:00:00+00:00",
        "2026-07-16T09:00:00+00:00"
    )
    .await
    .unwrap());

    revert_create(&pool, "2026-07-16", "T1").await.unwrap();

    assert!(
        begin_create(
            &pool,
            "2026-07-16",
            "T1",
            "2026-07-16T10:05:00+00:00",
            "2026-07-16T09:00:00+00:00"
        )
        .await
        .unwrap(),
        "after a definite failure the claim is free again"
    );
}

/// A claim left dangling by a crash mid-create is NOT a permanent dead end: once it
/// is older than the stale window a much-later retry reclaims it automatically (any
/// live create would long since have resolved). This is the recovery path.
#[tokio::test]
async fn a_stale_create_claim_is_reclaimable() {
    let pool = approved_proposal().await;
    // A claim taken long ago whose owner never resolved it (a crash mid-create).
    assert!(begin_create(
        &pool,
        "2026-07-16",
        "T1",
        "2026-07-16T10:00:00+00:00",
        "2026-07-16T00:00:00+00:00"
    )
    .await
    .unwrap());

    // A fresh claim two hours later, with a stale cutoff AFTER the dangling one.
    assert!(
        begin_create(
            &pool,
            "2026-07-16",
            "T1",
            "2026-07-16T12:00:00+00:00",
            "2026-07-16T11:45:00+00:00"
        )
        .await
        .unwrap(),
        "a claim older than the stale window is reclaimable, not stuck forever"
    );
}

/// A successful create resolves its claim: mark_created records the key and clears
/// the claim, so no later approve re-creates - even a stale cutoff can't reclaim a
/// row whose key is already set.
#[tokio::test]
async fn a_successful_create_resolves_its_claim() {
    let pool = approved_proposal().await;
    assert!(begin_create(
        &pool,
        "2026-07-16",
        "T1",
        "2026-07-16T10:00:00+00:00",
        "2026-07-16T09:00:00+00:00"
    )
    .await
    .unwrap());

    mark_created(
        &pool,
        "2026-07-16",
        "T1",
        "KAN-99",
        "jira",
        "2026-07-16T10:00:01+00:00",
    )
    .await
    .unwrap();

    let d = get_day_task_worklog(&pool, "2026-07-16", "T1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d.created_task_key.as_deref(), Some("KAN-99"));
    assert!(
        !begin_create(
            &pool,
            "2026-07-16",
            "T1",
            "2026-07-16T20:00:00+00:00",
            "2026-07-16T19:00:00+00:00"
        )
        .await
        .unwrap(),
        "a recorded create is never re-claimed"
    );
}

// ── Staleness: a draft falling behind the work it describes ─────────────────
//
// The whole feature rests on ONE comparison - the task's measured minutes now
// against the same figure captured when the draft was written - so these pin
// each way that comparison can be wrong, including the two that fail silently
// (no baseline, and a task whose minutes were revised down).

const DAY: &str = "2026-08-07";

#[tokio::test]
async fn a_fresh_draft_is_not_stale() {
    let pool = seeded().await;
    put_task(&pool, "T1", 40).await;
    let d = upsert_draft(&pool, DAY, "T1", match_upsert(), "t0")
        .await
        .unwrap();
    // Written against 40 minutes, read back against the same 40.
    assert_eq!(d.stale_minutes, Some(0));
    assert!(!d.stale);
}

#[tokio::test]
async fn work_since_the_draft_shows_as_minutes_behind() {
    let pool = seeded().await;
    put_task(&pool, "T1", 40).await;
    upsert_draft(&pool, DAY, "T1", match_upsert(), "t0")
        .await
        .unwrap();
    put_task(&pool, "T1", 62).await;

    let d = get_day_task_worklog(&pool, DAY, "T1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d.stale_minutes, Some(22));
    assert!(d.stale);
}

#[tokio::test]
async fn a_little_more_work_is_not_worth_a_warning() {
    // Below the threshold the draft is still an accurate summary, and flagging
    // it would train the user to ignore the flag.
    let pool = seeded().await;
    put_task(&pool, "T1", 40).await;
    upsert_draft(&pool, DAY, "T1", match_upsert(), "t0")
        .await
        .unwrap();
    put_task(&pool, "T1", 40 + WORKLOG_STALE_MINUTES - 1).await;

    let d = get_day_task_worklog(&pool, DAY, "T1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d.stale_minutes, Some(WORKLOG_STALE_MINUTES - 1));
    assert!(!d.stale, "under the threshold must not flag");
    assert!(stale_drafts(&pool, DAY).await.unwrap().is_empty());
}

#[tokio::test]
async fn regenerating_resets_the_baseline() {
    // The entire point of the warning: acting on it must clear it.
    let pool = seeded().await;
    put_task(&pool, "T1", 40).await;
    upsert_draft(&pool, DAY, "T1", match_upsert(), "t0")
        .await
        .unwrap();
    put_task(&pool, "T1", 90).await;
    assert!(
        get_day_task_worklog(&pool, DAY, "T1")
            .await
            .unwrap()
            .unwrap()
            .stale
    );

    let d = upsert_draft(&pool, DAY, "T1", match_upsert(), "t1")
        .await
        .unwrap();
    assert_eq!(d.stale_minutes, Some(0));
    assert!(!d.stale);
}

#[tokio::test]
async fn a_row_with_no_baseline_is_never_stale() {
    // Drafted before migration 077. NULL reads as "cannot know" - the
    // alternative (DEFAULT 0) declares every pre-upgrade draft stale by its
    // task's whole duration on the first read after the update lands.
    let pool = seeded().await;
    put_task(&pool, "T1", 200).await;
    upsert_draft(&pool, DAY, "T1", match_upsert(), "t0")
        .await
        .unwrap();
    sqlx::query("UPDATE day_task_worklogs SET drafted_minutes = NULL")
        .execute(&pool)
        .await
        .unwrap();

    let d = get_day_task_worklog(&pool, DAY, "T1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d.stale_minutes, None);
    assert!(!d.stale);
    assert!(stale_drafts(&pool, DAY).await.unwrap().is_empty());
}

#[tokio::test]
async fn minutes_revised_down_do_not_read_as_negative_work() {
    // The fold can move a segment to another workstream, so a task's minutes
    // genuinely go down. "-8 minutes of new work" is not a thing to show.
    let pool = seeded().await;
    put_task(&pool, "T1", 90).await;
    upsert_draft(&pool, DAY, "T1", match_upsert(), "t0")
        .await
        .unwrap();
    put_task(&pool, "T1", 55).await;

    let d = get_day_task_worklog(&pool, DAY, "T1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d.stale_minutes, Some(0));
    assert!(!d.stale);
}

#[tokio::test]
async fn a_posted_update_is_history_not_a_stale_draft() {
    // It is a record of what went out. Calling it out of date invites the user
    // to overwrite the only copy of something already live on a ticket.
    let pool = seeded().await;
    put_task(&pool, "T1", 40).await;
    upsert_draft(&pool, DAY, "T1", match_upsert(), "t0")
        .await
        .unwrap();
    sqlx::query("UPDATE day_task_worklogs SET state = 'posted'")
        .execute(&pool)
        .await
        .unwrap();
    put_task(&pool, "T1", 300).await;

    let d = get_day_task_worklog(&pool, DAY, "T1")
        .await
        .unwrap()
        .unwrap();
    assert!(!d.stale, "a posted update is never a stale draft");
    // It still reports the growth - the dashboard may want to say "you kept
    // working on this" - but nothing offers to overwrite it.
    assert_eq!(d.stale_minutes, Some(260));
    assert!(stale_drafts(&pool, DAY).await.unwrap().is_empty());
}

#[tokio::test]
async fn the_sweep_names_the_task_and_the_amount() {
    let pool = seeded().await;
    put_task(&pool, "T1", 10).await;
    put_task(&pool, "T2", 10).await;
    upsert_draft(&pool, DAY, "T1", match_upsert(), "t0")
        .await
        .unwrap();
    upsert_draft(&pool, DAY, "T2", propose_upsert(), "t0")
        .await
        .unwrap();
    put_task(&pool, "T1", 10 + WORKLOG_STALE_MINUTES + 3).await;

    let stale = stale_drafts(&pool, DAY).await.unwrap();
    assert_eq!(stale.len(), 1, "only the task that grew");
    assert_eq!(stale[0].task_id, "T1");
    assert_eq!(stale[0].title, "Task T1");
    assert_eq!(stale[0].stale_minutes, WORKLOG_STALE_MINUTES + 3);
}

#[tokio::test]
async fn a_draft_whose_task_is_gone_still_reads() {
    // LEFT JOIN, not INNER: the user may still want to read or delete it.
    let pool = seeded().await;
    put_task(&pool, "T1", 40).await;
    upsert_draft(&pool, DAY, "T1", match_upsert(), "t0")
        .await
        .unwrap();
    sqlx::query("DELETE FROM day_tasks")
        .execute(&pool)
        .await
        .unwrap();

    let d = get_day_task_worklog(&pool, DAY, "T1").await.unwrap();
    assert!(d.is_some(), "the draft must not vanish with its task");
    let d = d.unwrap();
    assert_eq!(d.stale_minutes, None);
    assert!(!d.stale);
}
