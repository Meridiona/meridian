"""coding_agent_indexer — registers ended Claude Code / Codex sessions.

A coding agent session (one continuous Claude Code or Codex conversation)
lives as an append-only JSONL file on disk. This package registers each
session as a single `app_sessions` row in `meridian.db` once the session
has ended, so the downstream summariser + PM-update workflow can find it.

Two entry points trigger registration:

  * **`hook.py`** — invoked by Claude Code's SessionEnd hook the moment a
    session ends gracefully. Real-time, zero polling overhead.
  * **`daemon.py`** — a low-frequency (every ~10 min) sweeper that catches
    sessions the hook didn't fire for (crashes, kills, Codex sessions
    with no equivalent hook, macOS sleep, etc.).

Both call the same `register.register_ended_session()` function, which is
idempotent against the (claude_session_uuid, started_at) unique index in
the `app_sessions` schema — so duplicate calls are safe no-ops.

Owns:
  * the `claude_session_uuid` column on `app_sessions` (migration 025)
  * a small fork-skip-list JSON file at ~/.meridian/coding_agent_indexer_state.json
    used to ignore JSONLs created by the (separate) summariser via
    `claude --fork-session`.

Does NOT:
  * write to screenpipe.db
  * render or store the JSONL transcript
  * call the MLX classifier or the Anthropic API (that's the summariser)
  * read the contents of message bodies — only metadata (timestamps,
    record-type counts, agent flavour)
"""
