---
name: screenpipe-capture
description: "Use when you need to capture what the user is currently doing via Screenpipe MCP tools — returns structured activity events including active app, window title, inferred task, and meeting data."
version: 1.0.0
author: Hermes Agent
license: MIT
metadata:
  hermes:
    tags: [Screenpipe, Activity, Capture, Context, Productivity]
    related_skills: [activity-context-inference, knowledge-map-update]
---

# Screenpipe Capture

Captures user screen activity using the Screenpipe MCP server (stdio transport via `npx -y screenpipe-mcp`). All tools are available after configuring `mcp_servers.screenpipe` in `~/.hermes/config.yaml`.

## When to Use

- At the start of each Watcher cycle to record what the user is doing
- When you need the last N minutes of screen activity as structured data
- To detect active meetings, focused work sessions, or context switches
- To build the episodic activity buffer for downstream Synthesizer processing

## When NOT to Use

- For historical analysis spanning hours/days → use `search_time_range` with explicit timestamps instead
- For video export → use `export_video` only when explicitly requested by the user
- For UI element inspection → use `search_elements` only for accessibility tasks

## Screenpipe MCP Tools

### Core Capture Tools

```
recent_context(minutes=5)
  → Returns last N minutes of activity as structured summary
  → Fields: apps, windows, text_content, active_duration_per_app

activity_summary(start_time, end_time)
  → Returns structured summary for a time range (ISO8601 timestamps)
  → Fields: app_usage [{app, duration_seconds, windows}], meetings, productivity_score

search_content(query, contentType="all", limit=10)
  → Full-text search across all captured content
  → contentType: "ocr" | "ui" | "audio" | "all"

search_time_range(start, end, query="")
  → Returns events in a time range, optionally filtered by text
  → Useful for finding when specific work happened
```

### Meeting Tools

```
list_meetings()
  → Returns all detected meetings with title, participants, start/end time

search_elements(query)
  → Searches UI element tree for accessibility metadata
```

## Standard Capture Sequence

Use this sequence every Watcher cycle:

```
1. Call recent_context(minutes=5) to get a quick summary
2. Call activity_summary(start_time=<5min ago>, end_time=<now>) for structured breakdown
3. Optionally call list_meetings() if audio was detected
4. Synthesize into an activity event JSON object
```

## Output Event Schema

Always return a JSON object with this shape — no extra prose:

```json
{
  "timestamp": "<ISO8601>",
  "active_app": "<primary app name>",
  "active_window": "<window title>",
  "inferred_task": "<one sentence description of what user is doing>",
  "confidence": 0.85,
  "meetings": [{"title": "...", "duration_minutes": 30}],
  "app_breakdown": {"VSCode": 180, "Chrome": 60},
  "raw_summary": "<brief natural language summary>"
}
```

## Confidence Scoring

| Signals | Confidence |
|---|---|
| Focused single app for >3min, clear window title | 0.85–1.0 |
| Multiple apps, coherent workflow (e.g., IDE + browser) | 0.65–0.85 |
| Frequent context switches, idle periods | 0.40–0.65 |
| Screensaver/locked/no activity | 0.0–0.40 |

## Pitfalls

- **Timestamp format**: Always use ISO8601 with timezone (`2025-01-15T10:30:00+00:00`)
- **Empty results**: If `recent_context` returns no data, the user may be idle or Screenpipe may not be running — set confidence to 0.0 rather than guessing
- **Meeting detection**: `list_meetings()` only works if microphone access is granted in Screenpipe
- **OCR noise**: Window titles extracted via OCR may have artifacts — normalize them (strip trailing special chars)

## Checklist

- [ ] Called `recent_context` and `activity_summary` for the same time window
- [ ] Populated all required fields in the output JSON
- [ ] Set confidence based on signal quality (not artificially high)
- [ ] Timestamp is UTC ISO8601
- [ ] Returned JSON only (no markdown wrapping)
