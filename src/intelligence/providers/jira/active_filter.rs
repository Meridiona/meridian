//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! What counts as an active work item: the JQL that asks for them, and the two
//! filters that decide what survives the answer.
//!
//! # Why this is its own file
//! Split out of `jira/fetch.rs` when the portable-JQL fallback pushed that file
//! past the repo's 500-line cap. The seam is a real one: everything here is
//! POLICY - which issue types are work, which are containers, and what to do
//! when a site does not have a type the query names - while `fetch.rs` keeps
//! the HTTP mechanics that carry it.
//!
//! # Who calls this
//! [`super::fetch`], which sends [`ACTIVE_TASK_JQL`], falls back to
//! [`PORTABLE_TASK_JQL`] on [`is_unknown_issue_type_error`], and filters every
//! result through [`is_work_item`] and [`is_excluded_type_name`].

use super::JiraIssue;

/// The active-task scope: everything assigned to you that isn't Done, except
/// Epics.
///
/// # Why this is a denylist, not an allowlist
///
/// This JQL is the ONLY thing that puts a Jira ticket into `pm_tasks`, which
/// makes an omission here invisible rather than noisy. A type left off the list
/// is absent from the Tasks page, absent from the worklog matcher's candidates
/// (so hours spent on it can only ever come back "no match"), and — worst — a
/// ticket of that type filed through the plan composer gets mirrored as a
/// `'local'` shadow row, because `plan_tasks::create` reads "not in pm_tasks
/// after a sync" as "self-assign failed". That shadow is meant to be temporary,
/// healed by the next sync's UPSERT; for an unfetchable type the heal can never
/// arrive, so a real Jira ticket sits in Meridian as a personal task forever.
///
/// That is not hypothetical — it is exactly what `Bug` being missing from the
/// old allowlist did. The list was then `(Task, Feature, Bug)`, which still
/// omitted **`Story`**, the default primary work type in every Jira Scrum
/// project. Any user on a standard Scrum board was therefore syncing almost
/// nothing, silently. Fixing that by appending `Story` would have left the same
/// trap armed for the next custom type someone's board uses.
///
/// So the filter names only what must stay OUT. Epics are excluded because they
/// are containers, not work: they reach Meridian as the `parent_key`/`epic_title`
/// of their children (see `mod.rs`'s upsert), never as rows of their own.
///
/// # `Story` and `Feature` are excluded by product decision, not by principle
///
/// Both are `hierarchyLevel: 0` — the SAME rung as Task and Bug. Neither is a
/// container; Jira treats each as an ordinary work item a person is assigned and
/// works on, and on a Scrum board `Story` is usually the primary one. They are
/// excluded because the product owner asked for it, NOT because they fail any
/// test the included types pass, and NOT because they sit above the work rung —
/// [`is_work_item`] would happily admit both.
///
/// That distinction matters for the next person here, because the two exclusion
/// mechanisms in this module look alike and are not:
///
/// - `type != Epic` is **structural**. Epics are containers; they reach Meridian
///   as the `parent_key`/`epic_title` of their children (see `mod.rs`'s upsert),
///   never as rows of their own. Removing it would be a bug.
/// - `type != Story` / `type != Feature` are **policy**. Nothing breaks if they
///   are removed; the board simply gets more tickets.
///
/// The cost is real and worth restating before anyone reinstates or removes
/// these casually: what is left is Tasks, Bugs, sub-tasks and custom types. On a
/// board that uses Stories or Features as its main work type this is close to an
/// empty board, and an hour spent on one of those tickets can only ever come
/// back "no match", silently. **If a user reports "my tickets do not show up",
/// this line is the first thing to check.**
///
/// Both are excluded SERVER-side, in the JQL, rather than by a name check next
/// to [`is_work_item`]. That is deliberate: [`MAX_RESULTS`] is a hard 100-row
/// ceiling with no pagination, so a type filtered client-side still consumes a
/// slot and is then thrown away, making truncation strictly worse. Filtered in
/// the JQL, they never occupy the budget at all.
///
/// Safe against a site that has neither type: Jira does not error on an
/// unrecognised type name in a `!=` clause (verified against a live Cloud site),
/// unlike an `IN` allowlist.
///
/// # Sub-tasks are IN
///
/// Reversing the earlier "tasks/features and their epics, never subtasks" rule:
/// on many boards the subtask IS the unit of work someone spends a day on, and
/// excluding it meant that day could not be logged against anything. `type !=
/// Epic` admits them without naming them, which matters because the type's name
/// is site-specific — Jira ships both `Subtask` and `Sub-task` as defaults, and
/// custom subtask types are common. (`type IN subtaskIssueTypes()` is the
/// function that resolves them by NAME-independent category if this ever needs
/// to select subtasks specifically.)
///
/// One consequence to know: for a subtask, Jira's `parent` is its Story/Task,
/// not its Epic, so `epic_title` holds the parent task's summary. That is
/// harmless where the value is consumed — `task_triage` uses it only as a
/// "context anchor" presence check, and a subtask always has a parent, so it
/// never trips `NoContextAnchor`.
///
/// # Consistency
///
/// Jira was the only provider that filtered by type at all — Linear, GitHub and
/// Trello fetch everything assigned to you, and Azure's WIQL is `AssignedTo =
/// @me` alone. This brings Jira in line with them.
pub(super) const ACTIVE_TASK_JQL: &str = "assignee = currentUser() AND statusCategory != Done \
     AND type != Epic AND type != Story AND type != Feature ORDER BY updated DESC";

/// The fallback JQL: only clauses EVERY Jira installation can answer.
///
/// [`ACTIVE_TASK_JQL`] names `Story` and `Feature`, and Jira rejects the whole
/// query with HTTP 400 - `The value 'Feature' does not exist for the field
/// 'type'` - when an installation has no issue type by that name. A site that
/// never created them (or renamed them) therefore fails its ENTIRE active-task
/// refresh, not just the exclusion. `type != Epic` carries the same risk in
/// principle; Epic is a system type present in every installation, so it is far
/// less likely, but this fallback drops it too rather than guessing which names
/// are safe.
///
/// Nothing is lost by falling back: [`is_work_item`] already excludes containers
/// by hierarchy LEVEL, and [`is_excluded_type_name`] applies the same name
/// policy client-side. The JQL clauses are a row-count optimisation, not the
/// correctness boundary.
pub(super) const PORTABLE_TASK_JQL: &str =
    "assignee = currentUser() AND statusCategory != Done ORDER BY updated DESC";

/// Issue type names [`ACTIVE_TASK_JQL`] excludes, enforced client-side as well.
///
/// On the primary path this is a no-op - the JQL already dropped them. It earns
/// its keep on the [`PORTABLE_TASK_JQL`] fallback, where the exclusion has to
/// happen here or the documented product policy silently stops applying on
/// exactly the sites that hit the fallback.
pub(super) const EXCLUDED_TYPE_NAMES: &[&str] = &["epic", "story", "feature"];

/// Is this issue one of the type names excluded by policy?
///
/// Case-insensitive: Jira type names are display strings and installations vary
/// in casing.
pub(super) fn is_excluded_type_name(issue: &JiraIssue) -> bool {
    EXCLUDED_TYPE_NAMES
        .iter()
        .any(|n| issue.fields.issuetype.name.eq_ignore_ascii_case(n))
}

/// Does this error mean the JQL referenced an issue type the site does not have?
///
/// Jira answers that with 400 and `The value 'X' does not exist for the field
/// 'type'`. Matched on the field phrase rather than a specific type name so a
/// renamed or absent `Story`, `Feature` or `Epic` all route to the fallback.
///
/// Deliberately narrow: every OTHER 400 (a genuinely malformed query, an
/// unsupported field) must still fail loudly rather than silently widening the
/// result set.
pub(super) fn is_unknown_issue_type_error(err: &anyhow::Error) -> bool {
    err.downcast_ref::<crate::intelligence::providers::http::HttpStatusError>()
        .is_some_and(|e| e.status == 400 && e.body.contains("does not exist for the field 'type'"))
}

/// The rung at and above which a Jira issue type is a CONTAINER rather than
/// work: `1` is Epic, `2`+ are the Premium/Advanced-Roadmaps tiers (Initiative,
/// Capability, and custom ones an org invents).
pub(super) const CONTAINER_HIERARCHY_LEVEL: i64 = 1;

/// Is this issue something a person does work on, as opposed to a bucket that
/// holds such things?
///
/// Filtering on the LEVEL rather than the NAME is the whole point. [`ACTIVE_TASK_JQL`]
/// can only say `type != Epic`, because JQL has no portable way to express
/// hierarchy — and a name is not a reliable proxy for a rung. "Feature" is
/// `hierarchyLevel: 0` (ordinary work) on some boards and a container tier above
/// Story on others; "Initiative" and "Capability" are containers that the JQL
/// clause never mentions at all. Without this check those arrive as work items,
/// which is precisely the category error excluding Epic exists to prevent.
///
/// **An unknown level counts as work.** Older Jira Server/Data Center responses
/// omit `hierarchyLevel` entirely, and the two failure modes are not symmetric:
/// wrongly including a container puts one extra row on the board, which is
/// visible and correctable, while wrongly excluding real work makes it
/// unloggable and silent — the same asymmetry that made the old `type IN (...)`
/// allowlist so expensive. So `None` is admitted.
pub(super) fn is_work_item(issue: &JiraIssue) -> bool {
    issue
        .fields
        .issuetype
        .hierarchy_level
        .is_none_or(|level| level < CONTAINER_HIERARCHY_LEVEL)
}
