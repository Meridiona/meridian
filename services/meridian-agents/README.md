# meridian-agents

LLM-driven session → ticket correlator and dispatcher for meridian. Slots in
after the Rust daemon's intelligence pipeline:

```
Rust ETL → app_sessions     ← read by the synthesizer
        + active_session    ← read by the synthesizer
        + pm_tasks          ← candidate tickets the LLM may match against
                              ↓
                    Synthesizer (every ~5 min, hybrid trigger)
                              ↓
        Tags every closed session via ticket_links (method='llm',
        confidence, routing). Writes session_summaries, upserts the
        context_graph, sets activity_context. Queues Jira write-backs
        on auto-routed sessions.
                              ↓
                    Jira-keeper (event-driven, on activity_context.trigger_jira_sync)
                              ↓
        Drains dispatch_queue. Cross-checks pm_tasks. Calls Rovo Atlassian
        MCP to add worklogs / comments / transitions. Writes back via
        dispatch_queue + activity_context.last_synced.
```

## Status

Migration `005_agents.sql` lands the agent-side schema. `db.py` exposes the
read/write surface. The synthesizer SKILL.md is in place. `llm.py`,
`agents/synthesizer.py`, `agents/jira_keeper.py`, and `orchestrator.py`
are still TODO.

## Layout

```
services/meridian-agents/
  pyproject.toml                  uv-managed, Python 3.11+
  uv.lock
  README.md
  src/meridian_agents/
    __init__.py
    config.py                     env-var loader
    db.py                         async SQLite layer (aiosqlite)
    skills.py                     load_skill(name) — points at ./skills/
    agents/                       TODO: synthesizer.py, jira_keeper.py
    sinks/                        TODO: protocol + jira.py
  skills/
    synthesizer/SKILL.md          system prompt for the synthesizer
  tests/
    conftest.py                   migrated_db_path fixture, seed helpers
    test_config.py                23 tests
    test_db.py                    59 tests
  vendor/
    hermes/                       hermes-agent v2026.5.7 vendored in-repo
                                  (run_agent.py, agent/, tools/, ...).
                                  Installed by uv via [tool.uv.sources].
  reference/                      source material from hermes-activity-agent
                                  used while porting — off the import path.
```

## Setup (once)

Requires [uv](https://docs.astral.sh/uv/):

```bash
cd services/meridian-agents
uv sync --extra dev
```

That builds wheels from the local sources for both `meridian-agents` and the
vendored `hermes-agent` at `vendor/hermes/`. No git fetch, no PyPI dep —
hermes lives inside the repo and is rebuilt on each sync.

The Rust daemon must have run at least once on the same `MERIDIAN_DB` so
migrations `001_initial.sql` … `005_agents.sql` apply. `db.py.schema_check`
fails loudly with an actionable message otherwise.

## Run

```bash
uv run meridian-agents          # daemon mode  (TODO — orchestrator.py)
uv run meridian-agents --once   # single tick (TODO)
```

## Tests + lint

```bash
uv run pytest tests/ -q   # 82 passing
uv run ruff check         # clean
```

Tests apply the actual `src/migrations/00*.sql` files to a temp SQLite file
so they catch schema drift between Rust and Python the moment it happens.

## Configuration

Reads from environment, plus `~/.meridian/.env` if present (mirrors the Rust
daemon's dotenv convention):

| Var | Default | Purpose |
|---|---|---|
| `MERIDIAN_DB` | `~/.meridian/meridian.db` | Shared SQLite file with the Rust daemon |
| `MERIDIAN_AGENTS_POLL_INTERVAL_SECS` | `300` | Long-pole synthesizer cadence (used by the orchestrator) |
| `MERIDIAN_AGENTS_AUTO_THRESHOLD` | `0.85` | Match confidence ≥ this → `routing='auto'`, queue Jira write-back |
| `MERIDIAN_AGENTS_QUEUE_THRESHOLD` | `0.60` | Match confidence ≥ this and < auto → `routing='queue'` |
| `OLLAMA_BASE_URL` | `https://ollama.com` | Cloud Ollama OpenAI-compatible endpoint |
| `OLLAMA_API_KEY` | _(required)_ | Cloud Ollama API key |
| `OLLAMA_MODEL` | _(required)_ | e.g. `gpt-oss:120b` |
| `JIRA_BASE_URL` / `JIRA_EMAIL` / `JIRA_API_TOKEN` | reused from Rust daemon | When all three set, the Jira sink is enabled; otherwise falls back to log-only. |
