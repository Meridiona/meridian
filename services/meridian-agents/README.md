# meridian-agents

LLM-driven session → ticket correlator and dispatcher for meridian.

This service slots in after the Rust daemon's intelligence pipeline:

```
Rust ETL → app_sessions → Rust categorizer (ActivityKind)
                                 ↓
                        meridian-agents  ←  pm_tasks (Rust refreshes)
                                 ↓
                        ticket_links (method='llm', confidence, routing)
                                 ↓
                        dispatch_queue → Jira / GitHub / Linear write-back
```

The Rust daemon owns the schema and writes deterministic categorisation; this
service uses an LLM to summarise sessions, match them to cached PM tasks, and
push worklogs / comments back to the source system.

## Status

**Scaffolding only.** Vendored runtime is in place; the agent and sink
modules are stubs awaiting implementation. See
`/Users/akarshhegde/.claude/plans/crystalline-dancing-thacker.md` for the
full plan.

## Layout

```
services/meridian-agents/
  pyproject.toml                       # uv-managed; Python 3.11+
  src/meridian_agents/
    __init__.py
    __main__.py                        # TODO — entry point
    orchestrator.py                    # TODO — asyncio main loop
    config.py                          # TODO — env-var loader
    db.py                              # TODO — meridian.db reader/writer
    llm.py                             # TODO — wraps hermes_runtime.AIAgent
    agents/
      session_summarizer.py            # TODO
      project_tagger.py                # TODO
    sinks/
      __init__.py                      # TODO — Sink protocol + LogSink
      jira.py                          # TODO — Jira write-back
    hermes_runtime/                    # vendored from hermes-activity-agent
      ai_agent.py                      # AIAgent class + tool-call loop
      atomic_io.py                     # atomic_json_write etc.
      logging.py                       # rotating handler + redacting formatter
      skill_loader.py                  # load SKILL.md prompts
  skills/                              # SKILL.md prompts (TODO)
  tests/
  reference/                           # source material from hermes — not on import path
    LICENSE.hermes
    hermes_agents/{watcher,synthesizer,jira_keeper,orchestrator,bootstrap,config}.py
    hermes_skills/{watcher,synthesizer,jira-keeper}/SKILL.md
```

## Setup (once)

Requires [uv](https://docs.astral.sh/uv/):

```bash
cd services/meridian-agents
uv sync
```

The Rust daemon must have run at least once on the same `MERIDIAN_DB` so
that `app_sessions`, `pm_tasks`, `ticket_links`, and the upcoming
`agent_runs` / `agent_cursor` / `dispatch_queue` tables exist.

## Run

```bash
uv run meridian-agents          # daemon mode (TODO)
uv run meridian-agents --once   # single tick — analyse newest session and exit (TODO)
```

## Configuration

Reads from environment (and `~/.meridian/.env` if present, same convention
as the Rust daemon):

| Var | Default | Purpose |
|---|---|---|
| `MERIDIAN_DB` | `~/.meridian/meridian.db` | Shared with the Rust daemon |
| `MERIDIAN_AGENTS_POLL_INTERVAL_SECS` | `300` | Tick cadence |
| `MERIDIAN_AGENTS_AUTO_THRESHOLD` | `0.85` | Confidence ≥ this → auto-dispatch |
| `MERIDIAN_AGENTS_QUEUE_THRESHOLD` | `0.60` | Confidence ≥ this and < auto → queue for review |
| `OLLAMA_BASE_URL` | `https://ollama.com` | Cloud Ollama OpenAI-compatible endpoint |
| `OLLAMA_API_KEY` | _(required)_ | Cloud Ollama API key |
| `OLLAMA_MODEL` | _(required)_ | e.g. `gpt-oss:120b` |
| `JIRA_BASE_URL` / `JIRA_EMAIL` / `JIRA_API_TOKEN` | _(reused from Rust daemon)_ | When set, the Jira sink is enabled; otherwise falls back to `LogSink`. |
