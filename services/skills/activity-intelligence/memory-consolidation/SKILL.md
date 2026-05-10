---
name: memory-consolidation
description: "Use when you need to consolidate the episodic activity buffer into semantic long-term memory — summarizing recurring patterns, pruning old buffer entries, and updating MEMORY.md with durable insights."
version: 1.0.0
author: Hermes Agent
license: MIT
metadata:
  hermes:
    tags: [Memory, Consolidation, Learning, Patterns, Productivity]
    related_skills: [knowledge-map-update, activity-context-inference]
---

# Memory Consolidation

Transforms the raw activity buffer into durable semantic memory. Runs infrequently (daily or when buffer exceeds 500 lines) to extract recurring patterns, update the knowledge graph with confidence decay, and write human-readable summaries to `~/.hermes/memories/`.

## When to Use

- Buffer (`buffer.jsonl`) exceeds 500 lines
- End of day (daily cron or manual trigger)
- Before a long idle period (user logging off)
- When context_map has not been updated in > 24 hours

## When NOT to Use

- During active Watcher/Synthesizer cycles — consolidation is expensive and blocks capture
- When the buffer has fewer than 20 events (insufficient data)

## Consolidation Steps

### Step 1: Pattern Extraction

Read all buffer events and group by:
- `active_app` + window title keywords → work session clusters
- Time of day distribution → identify peak productivity hours
- Recurring `inferred_task` values → habitual work patterns

### Step 2: Frequency Analysis

For each pattern cluster:
```
if pattern.count >= 3 across different days:
    label as "recurring"
if pattern.count >= 10:
    label as "habitual"
```

### Step 3: Update context_map.json

Apply staleness decay to all nodes:
```
for each node:
    days_since_seen = (now - node.last_seen).days
    if days_since_seen > 7:
        node.confidence_avg *= 0.85 ** (days_since_seen / 7)
    if node.confidence_avg < 0.1:
        node.stale = True
```

Add new recurring-pattern nodes with type="pattern".

### Step 4: Write MEMORY.md Summary

Append a dated entry to `~/.hermes/memories/MEMORY.md`:

```markdown
## 2025-01-15

### Work Patterns
- Primary focus: backend-api (85% of coding time)
- Recurring task: "debugging auth service" (4 sessions this week)
- Peak productivity: 09:00–12:00

### Projects Active
- backend-api (PROJ-123, PROJ-124) — 6.5h total
- devops (3 deployment sessions)

### New Patterns Learned
- User switches to Postman when debugging API endpoints
- Code review sessions always precede deployment

### Jira Activity
- Logged 6.5h across 2 tickets
- Transitioned PROJ-123 from In Progress → In Review
```

### Step 5: Trim Buffer

After consolidation, keep only the last 100 lines of buffer.jsonl:
```
keep = last 100 lines
overwrite buffer.jsonl with keep
```

## Output Files

| File | Action |
|---|---|
| `~/.hermes/memories/MEMORY.md` | Append dated summary |
| `~/.hermes/activity/context_map.json` | Apply decay + add pattern nodes |
| `~/.hermes/activity/buffer.jsonl` | Trim to last 100 lines |

All writes use `write_file`.

## Pattern Node Format

New nodes added during consolidation:

```json
{
  "id": "pattern_<slug>",
  "type": "pattern",
  "label": "<human description>",
  "last_seen": "<ISO8601>",
  "frequency": 12,
  "confidence_avg": 0.82,
  "metadata": {
    "triggers": ["VSCode open", "Terminal active"],
    "typical_duration_minutes": 45,
    "jira_projects": ["PROJ"]
  }
}
```

## Pitfalls

- **Don't over-consolidate**: Patterns need at least 3 occurrences before being treated as reliable
- **Preserve ambiguity**: If a pattern has mixed contexts, don't collapse them — create separate nodes
- **MEMORY.md is append-only**: Never overwrite the whole file — always append the new dated section
- **Buffer trim after write**: Trim buffer.jsonl ONLY after all other files are successfully written
- **Date boundaries**: Group events by calendar day in the user's local timezone, not UTC

## Checklist

- [ ] Read full buffer.jsonl before making changes
- [ ] Extracted pattern clusters (min 3 occurrences)
- [ ] Applied staleness decay to context_map nodes
- [ ] Appended dated summary to MEMORY.md
- [ ] context_map.json written with updated nodes
- [ ] buffer.jsonl trimmed to last 100 lines AFTER all other writes
- [ ] No existing MEMORY.md content overwritten
