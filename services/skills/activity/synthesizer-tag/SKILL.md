---
name: synthesizer-tag
description: Tag a single completed app session with the right Jira task (or mark it as overhead) and write a short summary. Per-session phase of the synthesizer.
version: 0.1.0
metadata:
  meridian:
    tags: [activity, synthesis, task-matching, per-session]
---

# Synthesizer (tag phase): one session at a time

You are tagging **exactly one** completed app session. The bundle below contains a single `session` plus the candidate task list and prior context. Output two tool calls (plus optional graph upserts) and stop. Do not summarise the cycle, do not write current focus — those happen in the next phase.

## Inputs

- `session` — one row from `app_sessions`:
  - `id`, `app_name`, `started_at`, `ended_at`, `duration_s`
  - `window_titles` — array of `[title, count]`, top first
  - `ocr_samples`, `audio_snippets`
  - `category` — Rust ETL hint (`coding`, `code_review`, `meeting`, `communication`, `design`, `documentation`, `planning`, `deployment_devops`, `research`, `idle_personal`)
  - `confidence` — Rust ETL hint
- `pm_tasks` — open Jira tickets assigned to the user. **The only valid `task_key` values you may emit.**
- `previous_context` — the user's last current-focus snapshot (`active_project`, `jira_key`, `inferred_task`, `confidence`, `tags`). Use as a prior.
- `context_graph_nodes` — known projects/tasks/tools/patterns/tickets the user touches.

## Tools

You have **three** tools. The runtime rejects any other tool here.

1. `match_session_to_task(session_id, task_key, confidence, session_type, routing)` — **mandatory, exactly once**.
2. `write_session_summary(session_id, summary_json)` — **mandatory, exactly once**. `summary_json` is a JSON-encoded string: `{"summary":"...","tags":["..."]}`.
3. `upsert_context_node(node_id, node_type, label)` — optional, zero or more times. `node_type` ∈ {project, task, tool, pattern, ticket}.

The order doesn't matter as long as both mandatory tools are called for the given `session.id`.

## The four valid match shapes

| Outcome | `session_type` | `task_key` | `confidence` | `routing` |
|---|---|---|---|---|
| Matched, dispatch automatically | `"task"` | from `pm_tasks` | ≥ 0.85 | `"auto"` |
| Matched, needs human review | `"task"` | from `pm_tasks` | 0.60 – 0.85 | `"queue"` |
| Looks like work, no candidate fits | `"task"` | `null` | < 0.60 | `"skip"` |
| Not work at all (idle, personal, off-task meeting) | `"overhead"` | `null` | `0.0` | `"skip"` |

`session_type: "unknown"` only for genuinely unintelligible sessions (no titles, no OCR, no audio).

## Confidence rubric

- **0.90 – 1.00** — direct evidence: the `task_key` (e.g. `KAN-86`) appears verbatim in window titles / OCR / audio, OR a branch name like `feat/KAN-86-...` is visible, OR the user is on the Jira ticket page.
- **0.70 – 0.89** — strong contextual match: keywords from the task title or description appear in window/OCR content; `app_name` is consistent with the task type (Coding tasks → IDE, Design tasks → Figma).
- **0.50 – 0.69** — weak contextual match: task is in the candidate list and *plausibly* fits, no direct evidence.
- **< 0.50** — no match. Emit `task_key: null` and pick the right `session_type`.

If `previous_context.jira_key` matches a candidate and the current session is on the same project, you may anchor toward it — cap at 0.70 unless direct evidence is also present.

## Hard rules

- `task_key` MUST be either `null` or a value from `pm_tasks[].task_key`. The runtime rejects any other key.
- Both `match_session_to_task` AND `write_session_summary` MUST be called. Don't skip the summary.
- Use the `session.id` from the bundle exactly — don't invent ids.
- Keep `summary_json.summary` to 2–3 sentences. If the session has no useful content, say so plainly: `"Brief session in <app>. No legible content captured."`
- Treat sessions with `category: "idle_personal"`, `"meeting"`, or `"communication"` as **overhead** unless there's clear evidence the user is doing trackable work (e.g. reviewing a Jira ticket page during a "meeting").

## Worked example

Bundle:
```json
{
  "session": {
    "id": 1431,
    "app_name": "Code",
    "started_at": "2026-05-09T15:50:00Z",
    "ended_at":   "2026-05-09T16:05:00Z",
    "duration_s": 900,
    "window_titles": [["synthesizer.py — meridian", 24], ["KAN-86 — Atlassian", 3]],
    "ocr_samples": ["from agents import db", "match_session_to_task"],
    "audio_snippets": [],
    "category": "coding",
    "confidence": 0.82
  },
  "pm_tasks": [
    {"task_key":"KAN-86","title":"Migrate active-intelligence code to meridian","status":"In Progress","status_category":"indeterminate"},
    {"task_key":"KAN-87","title":"Add logging and observability","status":"To Do","status_category":"new"}
  ],
  "previous_context": {"jira_key":"KAN-86","inferred_task":"Wiring synthesizer","confidence":0.7},
  "context_graph_nodes": [{"node_id":"project_meridian","node_type":"project","label":"meridian"}]
}
```

Correct tool calls:
1. `upsert_context_node({"node_id":"ticket_KAN-86","node_type":"ticket","label":"KAN-86 — Migrate active-intelligence"})`
2. `match_session_to_task({"session_id":1431,"task_key":"KAN-86","confidence":0.95,"session_type":"task","routing":"auto"})`
3. `write_session_summary({"session_id":1431,"summary_json":"{\"summary\":\"Edited synthesizer.py in VS Code while KAN-86 ticket was open in a sibling window. Direct work on the migration task.\",\"tags\":[\"coding\",\"KAN-86\"]}"})`

Then stop. Done.
