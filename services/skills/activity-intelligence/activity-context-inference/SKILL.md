---
name: activity-context-inference
description: "Use when you need to analyze a stream of raw activity events and infer the user's current project, task, Jira ticket, and intent — producing a structured current_context.json with a confidence score."
version: 1.0.0
author: Hermes Agent
license: MIT
metadata:
  hermes:
    tags: [Context, Inference, Activity, Productivity, AI]
    related_skills: [screenpipe-capture, knowledge-map-update, jira-sync-rovo]
---

# Activity Context Inference

Analyzes recent activity events from the episodic buffer and the existing knowledge map to produce `current_context.json` — the authoritative "what is the user doing right now" state used by downstream agents.

## When to Use

- Each Synthesizer cycle (every 20 minutes) after the Watcher has populated the buffer
- When at least 3 activity events exist in the last 30-minute window
- To decide whether to trigger a Jira sync

## When NOT to Use

- With fewer than 2 activity events (insufficient signal — skip and wait)
- When all events have confidence < 0.3 (user is idle — write a low-confidence context and exit)
- Do NOT make Jira changes directly — that is the Jira Keeper's job

## Input

You receive two inputs injected into your prompt:

1. **Activity buffer** (last 30 min, JSONL format) — each line is an activity event from `screenpipe-capture`
2. **context_map.json** — the persistent knowledge graph of known projects, tasks, and patterns

## Inference Process

### Step 1: Identify Active Work Pattern

Look for:
- Dominant app (> 40% of time) → strong signal for task type
- Window title keywords → map to known projects/tickets
- Consecutive events with same inferred_task → high confidence
- Sudden app switches → lower confidence, may indicate context switch

### Step 2: Map to Known Entities

Cross-reference the context_map:
- Match `active_app` + `active_window` against known project nodes
- If a Jira key pattern (`[A-Z]+-\d+`) appears in window titles → set `jira_key`
- Use `frequency` field in context_map nodes as a prior (frequent patterns are more trustworthy)

### Step 3: Score Confidence

Aggregate confidence from events:
```
avg_confidence = mean(event.confidence for event in last_30min)
pattern_boost = 0.1 if matched a known context_map node else 0.0
final_confidence = min(1.0, avg_confidence + pattern_boost)
```

### Step 4: Decide Jira Sync Trigger

Set `trigger_jira_sync: true` when ALL of these hold:
- `confidence >= 0.65`
- `jira_key` is not null OR `active_project` matches a known Jira project
- The context has changed significantly from the previous `current_context.json`
- At least 10 minutes of focused work was detected in the buffer

## Output: current_context.json

Write this file using `write_file`:

```json
{
  "timestamp": "<ISO8601 UTC>",
  "active_project": "<project name or null>",
  "jira_key": "<JIRA-123 or null>",
  "inferred_task": "<one sentence: what the user is doing>",
  "confidence": 0.78,
  "trigger_jira_sync": true,
  "tags": ["backend", "debugging", "api"],
  "evidence": {
    "dominant_app": "VSCode",
    "window_title_sample": "auth.py - my-project",
    "focused_minutes": 18,
    "event_count": 6
  }
}
```

## Context Change Detection

Compare current inference to previous `current_context.json`:
- **Same project + task**: update timestamp, do NOT trigger sync again for same work
- **New project**: always trigger sync if confidence >= 0.65
- **Same project, new jira_key**: trigger sync
- **Confidence drop below 0.5**: set `trigger_jira_sync: false`, don't discard `jira_key`

## Pitfalls

- **Don't invent Jira keys**: only set `jira_key` if you found a matching pattern in window titles or context_map — never guess
- **Avoid false positives**: YouTube/Netflix/Slack browsing should NOT trigger Jira sync even if high confidence
- **Idle periods don't mean no context**: if the last 5 events are idle but the 30min window has strong signal, use the strong signal
- **Timestamps must be UTC ISO8601** — normalize all event timestamps before comparison

## Checklist

- [ ] Read all events from the last 30 minutes of the buffer
- [ ] Cross-referenced events with context_map.json
- [ ] Confidence scored using the formula above
- [ ] `trigger_jira_sync` set correctly (not defaulted to true)
- [ ] current_context.json written with write_file
- [ ] context_map.json also updated (via knowledge-map-update skill)
