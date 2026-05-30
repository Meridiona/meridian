# Jira Comment Style Reference

Tone and structure conventions for Meridian-generated PM updates.
Loaded on-demand by the agent via `get_skill_reference`.

## Voice

- **Engineer-to-engineer**, not status-report-to-manager
- Past tense for shipped, present continuous for in-progress
- Lower-case bullets unless the first word is a proper noun
- One sentence per bullet, max ~25 words

## Bullet shape

```
<verb in past/present> <specific object> <evidence ref in parens>
```

Examples that match the house style:

- `wired the OS process-tree detection for ax-sidecar host app (s10212)`
- `debugging FK cleanup in migration 022 — sqlite refuses DROP+RECREATE in same tx`
- `picked process-tree over osascript for host detection: works when window not focused`

Anti-patterns:

- `I worked on…` (avoid first-person)
- `Successfully implemented…` (no marketing verbs)
- `Made some changes` (no specificity)
- `Need to do more work` (no signal)

## Section ordering

The schema enforces this order. Don't try to reorder by importance —
consistency is more useful than dramatic structure for a daily log.

1. summary (1 line)
2. what_shipped
3. in_progress
4. blockers (omit section if empty)
5. decisions (omit section if empty)
6. next_steps (≤ 5 items, omit if empty)

## Evidence ref format

Each bullet's `evidence_refs` is a list of `session_id` integers from
the bundle. The renderer turns them into a small parenthetical:
`(s10212)` for one ref, `(s10212, s10231)` for many, `(s10212+3)` when
more than three.

## When to mention tools / files

Yes — when the bundle's `top_titles` clearly identifies the file. Eg.
`edited ax_sidecar.py to add process-tree detection (s10212)`.

No — when only the app name is known. Don't write `worked in VS Code`;
that's noise.

## When to mention time

The footer auto-includes `~N min`. Don't repeat time spent inside the
bullets unless it's narratively important (`spent 40 min on one
flaky test before finding the race condition`).
