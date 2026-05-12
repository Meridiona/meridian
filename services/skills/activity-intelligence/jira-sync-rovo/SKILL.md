---
name: jira-sync-rovo
description: "Use when you need to update the Jira board based on detected user activity — log worktime, transition ticket status, and add progress comments using Rovo MCP tools only (never raw Jira REST API)."
version: 1.0.0
author: Hermes Agent
license: MIT
metadata:
  hermes:
    tags: [Jira, Rovo, MCP, Productivity, Automation]
    related_skills: [activity-context-inference, memory-consolidation]
---

# Jira Sync via Rovo MCP

Updates the Jira board using Rovo MCP tools. All Jira operations go through the `mcp__claude_ai_Atlassian_Rovo__*` toolset — never direct REST API calls.

## When to Use

- `current_context.json` has `trigger_jira_sync: true`
- Confidence is at or above 0.65
- User has been working on a known Jira ticket or project

## When NOT to Use

- Confidence < 0.65 — always skip, never guess
- When `active_project` is null and `jira_key` is null (nothing to sync to)
- When the user is in a meeting (meeting data in context) — meetings should NOT trigger ticket transitions
- When the same state was already synced (check `last_synced` in jira_state.json)

## Available Rovo MCP Tools

```
searchJiraIssuesUsingJql(jql, fields)
  → Find tickets matching JQL. Use to discover relevant open tickets.
  → Example: project = MYPROJ AND status != Done AND assignee = currentUser()

getJiraIssue(issueKey)
  → Get full ticket detail including status, summary, description, assignee

getTransitionsForJiraIssue(issueKey)
  → Get valid status transitions for a ticket (use before transitioning)

transitionJiraIssue(issueKey, transitionId)
  → Move ticket to a new status. ALWAYS call getTransitions first to get valid IDs.

addWorklogToJiraIssue(issueKey, timeSpent, comment)
  → Log worked time. timeSpent format: "1h 30m" or "45m"

addCommentToJiraIssue(issueKey, body)
  → Add a comment. Keep it to 1-2 sentences.

createJiraIssue(projectKey, summary, description, issueType)
  → Create a new ticket. Only do this when explicitly clear from context.

getVisibleJiraProjects()
  → List all accessible projects. Use during bootstrap.
```

## Decision Logic

```
1. Read current_context.json → extract jira_key, active_project, inferred_task, evidence

2. If jira_key is set:
   a. Call getJiraIssue(jira_key) to get current status
   b. Determine if status transition is warranted (see Transition Rules)
   c. If focused_minutes >= 10: addWorklogToJiraIssue
   d. If significant progress detected: addCommentToJiraIssue

3. If jira_key is null but active_project is set:
   a. searchJiraIssuesUsingJql to find open tickets for the project
   b. Match by task keywords (inferred_task vs ticket summaries)
   c. If high-confidence match found, set jira_key and proceed as above
   d. If no match, skip — do NOT create tickets speculatively

4. Write updated jira_state.json
5. Write trigger_jira_sync: false to current_context.json
```

## Transition Rules

Only transition a ticket when the inferred_task strongly implies a new status:

| Inferred Task Keywords | Suggested Transition |
|---|---|
| "testing", "running tests", "writing tests" | In Review |
| "deploying", "deployment", "shipping" | Done |
| "reviewing PR", "code review" | In Review |
| "debugging", "fixing", "investigating" | In Progress |
| "planning", "designing", "scoping" | In Progress |

- Always call `getTransitionsForJiraIssue` to get the valid transition ID for that project
- Never hard-code transition IDs — they vary by project workflow

## Worklog Format

```
focused_minutes from evidence.focused_minutes:
  >= 60 min → log "1h"
  30-59 min → log "30m"
  10-29 min → log "<N>m"
  < 10 min → skip worklog entirely

Comment on worklog: "<inferred_task>" (keep ≤ 80 chars)
```

## Comment Style

Keep comments terse and factual. Examples:

Good: "Implemented JWT refresh token rotation in auth.py"
Bad: "The user was working on this for a while and made some changes to the authentication system which seems to relate to..."

Max 2 sentences. No markdown. No hedging language.

## jira_state.json Schema

```json
{
  "tickets": {
    "PROJ-123": {
      "status": "In Progress",
      "summary": "Implement auth refresh",
      "last_synced": "<ISO8601>",
      "total_logged_minutes": 45
    }
  },
  "last_sync": "<ISO8601>",
  "projects": [
    {"key": "PROJ", "name": "My Project", "last_seen": "<ISO8601>"}
  ]
}
```

Write this with `write_file` after all Jira operations complete.

## Pitfalls

- **Always get transitions before transitioning** — hard-coded IDs will break across projects
- **No speculative ticket creation** — only create tickets when the context makes it completely unambiguous
- **Idempotency**: Check `last_synced` in jira_state — if the ticket was synced within the last 15 minutes for the same task, skip
- **Time zones**: Jira worklog timestamps must be ISO8601 with offset; use UTC
- **Comment rate limiting**: Do not add more than 1 comment per Jira sync cycle per ticket

## Checklist

- [ ] Confirmed confidence >= 0.65 before starting
- [ ] Called getTransitionsForJiraIssue before any transition
- [ ] Worklog logged only if focused_minutes >= 10
- [ ] Comment kept to ≤ 2 sentences, no hedging
- [ ] jira_state.json updated with write_file
- [ ] trigger_jira_sync set to false in current_context.json
- [ ] No tickets created speculatively
