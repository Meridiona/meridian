"""coding_agent_indexer — registers ended Claude Code / Codex sessions.

A coding agent session lives as an append-only JSONL file on disk. This
package registers each session as one or more `app_sessions` rows in
`meridian.db` (one row per local calendar day), so the downstream
summariser and PM-update workflow can find them.

Two entry points trigger registration:

  * **`hook.py`** — invoked by Claude Code's SessionEnd hook the moment a
    session ends. Real-time, zero polling overhead.
  * **`daemon.py`** — a low-frequency sweeper (default 10 min) that catches
    sessions the hook missed: crashes, force-quits, macOS sleep, and all
    Codex sessions (Codex has no equivalent SessionEnd hook).

Both call the same `register.register_ended_session()` which is idempotent
against the `(claude_session_uuid, day_utc)` unique index.

Supported agents:
  * **Claude Code** — ~/.claude/projects/<project>/<uuid>.jsonl
  * **Codex** — ~/.codex/sessions/<YYYY>/<MM>/<DD>/rollout-*.jsonl
"""
