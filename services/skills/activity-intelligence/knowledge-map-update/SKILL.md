---
name: knowledge-map-update
description: "Use when you need to update context_map.json — the persistent append-only knowledge graph of user work patterns, projects, tools, and relationships — after inferring new context from activity data."
version: 1.0.0
author: Hermes Agent
license: MIT
metadata:
  hermes:
    tags: [Memory, KnowledgeGraph, Context, Persistence, Productivity]
    related_skills: [activity-context-inference, memory-consolidation]
---

# Knowledge Map Update

Maintains `~/.hermes/activity/context_map.json`, the persistent graph that accumulates the agent's understanding of the user's work patterns over time. Nodes never get deleted — stale nodes decay in weight but remain as history.

## When to Use

- After each Synthesizer inference cycle, to record new patterns
- When a new project, tool, Jira ticket, or task type is discovered
- When an existing node's `frequency` or `last_seen` needs updating
- After a successful Jira sync, to reinforce the project→ticket edge

## When NOT to Use

- Do NOT delete nodes — mark them as `stale: true` and lower `frequency` instead
- Do NOT update the map without a corresponding `current_context.json` update
- For memory consolidation across days → use `memory-consolidation` skill

## Graph Schema

```json
{
  "nodes": [
    {
      "id": "<uuid or slug>",
      "type": "project | task | tool | pattern | ticket",
      "label": "<human-readable name>",
      "last_seen": "<ISO8601>",
      "frequency": 14,
      "confidence_avg": 0.78,
      "metadata": {}
    }
  ],
  "edges": [
    {
      "from": "<node_id>",
      "to": "<node_id>",
      "relation": "uses | part_of | transitions_to | linked_to | opens_with",
      "weight": 0.85,
      "last_seen": "<ISO8601>"
    }
  ],
  "last_updated": "<ISO8601>"
}
```

## Node Types

| Type | Description | Example label |
|---|---|---|
| `project` | A codebase or product area | "backend-api", "mobile-app" |
| `task` | A recurring work pattern | "debugging", "code review", "deployment" |
| `tool` | An app or tool used | "VSCode", "Postman", "Terminal" |
| `ticket` | A Jira ticket | "PROJ-123" |
| `pattern` | A combination of tools/tasks | "backend-debugging-session" |

## Update Algorithm

### Adding a New Node

```
1. Generate slug id: slugify(type + "_" + label)
2. Check if node with same id exists
3. If exists: increment frequency, update last_seen, update confidence_avg
4. If new: add node with frequency=1
```

### Adding/Updating an Edge

```
1. Find edge with matching (from, to, relation)
2. If exists: increase weight by 0.05 (cap at 1.0), update last_seen
3. If new: add edge with weight=0.5
```

### Staleness Decay (apply during consolidation, not every cycle)

```
for each node where (now - last_seen) > 7 days:
    node.confidence_avg *= 0.85
    if node.confidence_avg < 0.1:
        node.stale = True
```

## Common Update Patterns

After a Synthesizer cycle produces a new context:

```
1. Upsert project node (if active_project is not null)
2. Upsert task node (from inferred_task keywords)
3. Upsert tool node (from dominant_app in evidence)
4. Add edge: project → task (relation: "involves")
5. Add edge: task → tool (relation: "uses")
6. If jira_key: upsert ticket node, add edge project → ticket (relation: "tracked_by")
```

## Writing the Update

Always use `write_file` to persist:

```
write_file(path="~/.hermes/activity/context_map.json", content=<json string>)
```

Always pretty-print JSON (indent=2) for human readability.

## Pitfalls

- **ID collisions**: Use deterministic slugs (`project_backend-api`) not random UUIDs — this allows upsert semantics
- **Edge explosion**: Only create edges that are semantically meaningful — don't create an edge for every co-occurrence
- **Frequency inflation**: Only increment frequency once per synthesis cycle, not once per event
- **Write atomically**: Write the entire file — never append fragments

## Checklist

- [ ] All new nodes upserted with correct type
- [ ] Edges reflect actual relationships (not spurious co-occurrences)
- [ ] `frequency` and `last_seen` updated for existing nodes
- [ ] `last_updated` set to current UTC timestamp
- [ ] File written with write_file (pretty JSON)
- [ ] No nodes deleted
