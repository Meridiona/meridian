"""coding_agent_summariser — fills `session_summary` for sealed coding rows.

Phase 2 of the coding-agent pipeline. The `coding_agent_indexer` writes sealed,
immutable `app_sessions` rows for each Claude Code work-burst (see
[[project_indexer_segmentation]]) with `task_method='pending_summariser'`. This
package turns each into a factual prose `session_summary` that the PM-update
workflow consumes.

How it summarises (decided empirically — see [[project_coding_session_summariser]]):
  * Pipe the already-rendered `session_text` transcript to `claude -p` running
    the native `session-summary` skill with `--json-schema`, using the user's
    Claude subscription (no API-key billing). A prior burst's summary is passed
    as context so continued sessions read coherently.
  * On rate/usage limit (or no `claude` CLI), fall back to the local MLX server's
    OpenAI-compatible `/v1/chat/completions`.

Guarantees:
  * **Idempotent** — only sealed rows with `session_summary IS NULL` are picked;
    the write is `WHERE session_summary IS NULL` so concurrent runs / restarts
    never double-write or clobber.
  * **Crash-safe** — the summary is written only on success; an interrupted row
    stays NULL and is retried next tick. No cursor, no lost work.
  * **Bounded** — one transcript in memory at a time, sequential calls, a cap
    per tick, and a subprocess timeout; the daemon is idle (event-wait) between
    ticks. Cheap on CPU and memory.

Scope: Claude Code sessions only. Does NOT link sessions to Jira tickets — that
is the next phase.
"""
