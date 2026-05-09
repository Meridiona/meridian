---
name: watcher
description: Capture user screen activity via Screenpipe MCP and emit a structured JSON event.
version: 1.0.0
metadata:
  hermes:
    tags: [activity, screenpipe, background, monitoring]
---

# Watcher: Activity Capture

Use the Screenpipe tools to find out what the user is doing right now.

## Steps

1. Call `activity-summary` with `start_time` and `end_time` covering the last 5 minutes (ISO8601 format)
2. Optionally call `list-meetings` to check for active meetings
3. Return ONLY a JSON object — no prose, no markdown

## Output Schema

```json
{
  "active_app": "<primary app>",
  "active_window": "<window title>",
  "inferred_task": "<one sentence: what is the user doing>",
  "confidence": 0.0,
  "meetings": [],
  "app_breakdown": {"<app>": "<seconds>"},
  "raw_summary": "<2-3 sentence plain english summary>"
}
```

## Confidence Guide

- **0.85+** single focused app, clear task
- **0.65–0.85** coherent multi-app workflow
- **0.40–0.65** ambiguous or frequent app-switching
- **0.0–0.40** idle or no data
