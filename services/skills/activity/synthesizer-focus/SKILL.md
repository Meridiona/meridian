---
name: synthesizer-focus
description: Roll up the just-tagged sessions plus the active session into a single current-focus snapshot. Final phase of the synthesizer cycle.
version: 0.1.0
metadata:
  meridian:
    tags: [activity, synthesis, current-focus]
---

# Synthesizer (focus phase): write current focus

Every closed session in this cycle has already been tagged. Your only job is to roll those tags up — together with the user's `active_session` and `previous_context` — into a single `write_current_context` call. Then stop.

## Inputs

- `recent_tags` — what was just decided per session, in cycle order:
  ```
  [{"session_id":..., "task_key":..., "session_type":..., "routing":..., "confidence":...}, ...]
  ```
- `active_session` — the open session right now (the user is in this app), or null. Same shape as a closed session minus `ended_at` / `duration_s`.
- `pm_tasks` — open Jira tickets. **`jira_key` MUST be either null or a value from this list.**
- `previous_context` — last cycle's snapshot.
- `context_graph_nodes` — anchor points.

## Tools

Exactly **two** tools are valid here:

1. `upsert_context_node(node_id, node_type, label)` — optional, zero or more times.
2. `write_current_context(inferred_task, confidence, trigger_jira_sync, active_project?, jira_key?, tags?)` — **mandatory, exactly once, must be your final tool call**.

The runtime rejects every other tool in this phase.

## How to choose the values

- **`jira_key`** — pick the most-tagged `task_key` across `recent_tags` (weighted by confidence and recency), unless `active_session` clearly points at a different ticket (e.g. a Jira ticket page is open) — in which case go with the active one. If everything is `null`, set `jira_key: null`.
- **`active_project`** — derive from `pm_tasks[jira_key].project_key` when known, else from `context_graph_nodes`, else `null`.
- **`inferred_task`** — one short, declarative sentence describing what the user is doing right now (anchor to `active_session`, fall back to the most recent matched session). Avoid creative phrasing — be deterministic.
- **`confidence`** — the confidence of the `recent_tags` row that supplied `jira_key`, decayed slightly if `active_session` doesn't reinforce it. Cap at the highest per-session confidence.
- **`tags`** — a small set of stable slugs aggregated from session summaries (e.g. `["coding","KAN-86","meridian"]`).
- **`trigger_jira_sync`** — set to `true` ONLY if **all four** hold:
  1. `confidence >= 0.65`
  2. The work is Jira-trackable (coding, reviewing, deploying, documenting work tied to a ticket — not meetings, comms, or idle).
  3. `jira_key` is non-null and in `pm_tasks`.
  4. The context **changed** since `previous_context` (different `jira_key`, different `active_project`, or substantially different `inferred_task`).
  Otherwise `false`.

## Empty inputs

If `recent_tags` is empty AND `active_session` is null, write a low-confidence pass-through:

```
{ "inferred_task": "<previous inferred_task or 'No active work'>",
  "confidence": max(0, previous_context.confidence - 0.1),
  "trigger_jira_sync": false,
  "active_project": previous_context.active_project,
  "jira_key": previous_context.jira_key if it's still in pm_tasks else null,
  "tags": previous_context.tags }
```

## Hard rules

- `write_current_context` MUST be your final tool call.
- `jira_key` MUST be either null or a value from `pm_tasks[].task_key`.
- Don't speculate beyond the evidence in `recent_tags` and `active_session`. If most sessions in this cycle were `overhead`, say so plainly and emit `trigger_jira_sync: false`.

## Worked example

Bundle (simplified):
```json
{
  "recent_tags": [
    {"session_id":1430,"task_key":"KAN-86","session_type":"task","routing":"queue","confidence":0.78},
    {"session_id":1431,"task_key":"KAN-86","session_type":"task","routing":"auto","confidence":0.95},
    {"session_id":1432,"task_key":null, "session_type":"overhead","routing":"skip","confidence":0.0}
  ],
  "active_session": {"app_name":"Code","window_titles":[["synthesizer.py — meridian", 8]],"category":"coding"},
  "pm_tasks": [{"task_key":"KAN-86","title":"Migrate active-intelligence code to meridian","project_key":"KAN"}],
  "previous_context": {"jira_key":"KAN-86","active_project":"meridian","inferred_task":"Wiring synthesizer","confidence":0.7,"tags":["coding"]}
}
```

Correct tool call:
```
write_current_context({
  "active_project": "meridian",
  "jira_key": "KAN-86",
  "inferred_task": "Editing synthesizer.py for the meridian active-intelligence migration.",
  "confidence": 0.9,
  "trigger_jira_sync": false,
  "tags": ["coding","KAN-86","meridian"]
})
```

(`trigger_jira_sync` is `false` because the context didn't change — `jira_key` is still `KAN-86`. The keeper already has it.)
