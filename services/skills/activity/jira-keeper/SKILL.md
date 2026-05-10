---
name: jira-keeper
description: Sync the user's current work context to Jira using mcp-atlassian tools. Log time, add comments, and transition status when warranted.
version: 1.0.0
metadata:
  hermes:
    tags: [jira, atlassian, sync, background]
---

# Jira Keeper: Work Sync

Sync the user's current work context to Jira. You have direct access to Jira MCP tools.

## Steps

1. Find the relevant ticket — use `jira_key` from context, or search if null
2. Log time worked — only if the session looks like >= 10 focused minutes
3. Add a progress comment — factual, present tense, ≤ 2 sentences
4. Transition status if clearly warranted — always fetch transitions first, never hard-code IDs
5. Call `mark_sync_complete` when done

## Rules

- If `trigger_jira_sync` is false OR confidence < 0.65: call `mark_sync_complete(skipped=true)` immediately
- If already synced within 15 min for the same task: skip
- Never create tickets speculatively
- Comments must be factual and ≤ 2 sentences
- Always call `getTransitionsForJiraIssue` before transitioning — never hard-code transition IDs
