<!-- ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity -->

# Ticket Triage Rules

The deterministic rules the onboarding board-cleanup uses to sort a freshly-fetched
PM board into four buckets **before any classification runs**. No LLM, no session
evidence — only fields already on `pm_tasks`. This document is the human-readable
spec for `rules.rs` (predicates) + `mod.rs` (bucketing). If you change a threshold
or rule, update this file and the fixture dataset (`triage_fixtures.json`) together.

## Why this exists

A local LLM's classification accuracy is capped by **how many** and **how clean**
the candidate tickets are. The fetch is deliberately broad (`assignee = currentUser()
AND statusCategory != Done` — every open ticket, stale or not), so we triage *after*
fetch and let the user clean their board in one fast, worst-first pass. Garbage
candidates → garbage classification → a bad worklog at end of day; this is the gate
that prevents it.

## Safety contract (non-negotiable)

- The engine **only proposes**. It never mutates a ticket, never deletes, never
  excludes on its own. Every removal is a human-confirmed decision.
- A wrong verdict costs the user **one glance**, never lost data. The worst case is
  a session temporarily landing in `untracked`, which is re-bindable later.
- We bias to **KEEP**: better to leave a dead ticket in the pool (runtime evidence
  demotes it later) than to wrongly flag a live one as stale.
- **Stale requires ALL stale signals together.** One signal alone never demotes.

## The four buckets

| Bucket | Meaning | Suggested action |
|---|---|---|
| ✅ `ready` | Looks active **and** detailed enough to attribute work to | keep |
| ✏️ `needs_detail` | Likely active, but too thin for the classifier to match | add detail (user/AI) |
| 🗑️ `looks_stale` | Abandoned signature — propose excluding / closing | review for close |
| ❓ `not_sure` | Open and reasonable, but no signal either way | quick confirm |

## The two axes

Each ticket is judged on two independent questions.

### Axis A — Is it active?

**Active signals** (any one ⇒ leans active):

| Signal | Rule |
|---|---|
| In progress | `status_raw` resolves to **Started** |
| Due soon | `due_date` within `[now − overdue_grace, now + due_soon]` |
| In sprint | `sprint_name` non-empty *(only if the board uses sprints)* |
| Start reached | `start_date` is within `[now − 90d, now]` |

**Stale signature** — the **base** must hold, paired with **at least one** demoting signal:

Base (all required):
1. `status_raw` is **not** Started (NotStarted or Unknown), **and**
2. no live date window (not due-soon and no active start window), **and**
3. *(sprint boards only)* the ticket is **not** in a sprint.

Demoting signal (any one):
- age (`now − updated_at`) **>** `stale_age_days` (abandoned), **or**
- due date is **more than `due_soon_days` in the future** (`far_future` — planned, not
  current work; excluded from the candidate pool until session-evidence rescues it).

So a far-future-dated ticket that isn't started or sprinted is stale **even when recent**;
a ticket with no due date and recent activity is **not** stale (it becomes `not_sure`).

### Axis B — Is it classifiable?

| Signal | Rule |
|---|---|
| Missing description | `description_text` is empty/whitespace |
| Thin description | fewer than `thin_desc_chars` characters |
| Vague title | 1–2 words, all generic fillers ("Fix bug", "Updates", "WIP") |
| No context anchor | no epic **and** no parent *(only counts when also thin)* |
| Missing due date | no due date — **only when the board uses due dates** (some ticket has one). A board that never sets due dates is never flagged for the missing field. |

## Decision order (first match wins)

```
1. is_terminal (done)            → 🗑️ looks_stale  (reason: already_done)
2. stale signature (all of A)    → 🗑️ looks_stale
3. missing | thin | vague (B)    → ✏️ needs_detail
4. has an active signal (A)      → ✅ ready
5. otherwise                     → ❓ not_sure
```

Stale is checked **before** quality — no point enriching a ticket about to be excluded.
Quality is checked **before** ready — a due-soon-but-thin ticket is `needs_detail`, not `ready`.

## Thresholds (defaults — `TriageConfig`)

| Name | Default | Meaning |
|---|---|---|
| `stale_age_days` | 60 | Updated longer ago than this confirms staleness |
| `due_soon_days` | 30 | A due date within this horizon is an active signal |
| `overdue_grace_days` | 14 | Overdue by more than this (and otherwise quiet) reads as abandoned |
| `thin_desc_chars` | 40 | Descriptions shorter than this are too thin to match against |

Defaults are conservative on purpose (bias to KEEP). The real reference board
(32 tickets, no sprints, all detailed, due dates around launch) triages to **all
`ready`** — the clean-board case correctly produces zero nagging.

## Edge cases handled (each proven necessary by real data)

- **Jira colon-less offset timestamps** (`2026-06-11T11:34:51.105+0530`) are not
  valid RFC3339. We fall back to a `%z` parse; otherwise age would read as unknown
  for every Jira ticket and nothing would ever look stale.
- **Unparseable / missing timestamp** ⇒ age is *unknown*, which is treated as
  "not provably old" — never as stale. A ticket can't be demoted on an unreadable date.
- **No-sprint boards** — if **no** ticket has a sprint, sprint rules are disabled
  entirely (else the whole board would wrongly read as stale).
- **Arbitrary status names** ("In Review", "QA", "Selected for Development") are
  matched on **word boundaries**, never substrings (migration 036's lesson:
  "Incomplete" contains "complete"). Multi-word queue columns that share a word
  with an active column ("Selected for **Development**") are matched as whole phrases.
- **Unknown status** (an exotic custom column) is **never** read as Started — that
  would wrongly rescue a stale ticket. It only ever reaches `ready` via a *different*
  active signal (due/sprint/start).
- **The zombie** (recently bumped on the board but no due / sprint / progress) →
  `not_sure`, never `ready` (we don't trust `updated_at` as an active signal) and
  never `looks_stale` (it's too recent) — exactly the ambiguous case to ask the user.
- **Overdue boundary** — overdue *within* grace is still `ready` (due-soon); overdue
  *beyond* grace with no movement contributes to `looks_stale`.
- **Terminal/done tickets** never belong in the active candidate pool → `looks_stale`.
