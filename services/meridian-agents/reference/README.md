# Reference: hermes-activity-agent

Source material vendored from
[`hermes-activity-agent`](https://github.com/) — a fork/derivative of Hermes
(Nous Research). Files in this directory are **not on the Python import
path**; they exist as templates the meridian-agents implementation will
mine from.

The active vendored runtime lives at
`../src/meridian_agents/hermes_runtime/`. Files there have been copied in
unmodified; pull future updates from upstream and replay any local
adjustments.

## Contents

- `LICENSE.hermes` — MIT license from upstream. Preserve attribution when
  copying any further files.
- `hermes_agents/` — the original `agents/` directory:
  - `watcher.py` — polls Screenpipe MCP every 3 min; ignored (we read
    `meridian.db` directly).
  - `synthesizer.py` — every 20 min, infers task / project from buffered
    activity. Source material for `meridian_agents/agents/session_summarizer.py`.
  - `jira_keeper.py` — syncs to Jira via mcp-atlassian. Source material for
    `meridian_agents/agents/project_tagger.py` and `meridian_agents/sinks/jira.py`
    (which uses Jira REST directly via `httpx`, not MCP).
  - `orchestrator.py` — asyncio coordinator. We write a fresh orchestrator
    tuned to "new rows in app_sessions" rather than fixed-cadence polling.
  - `bootstrap.py` — state-dir init under `~/.hermes/`. Replaced by
    `meridian_agents/config.py` (no separate state dir; we read/write
    `meridian.db`).
  - `config.py` — env-var + skill loader. Already vendored as
    `hermes_runtime/skill_loader.py`.
- `hermes_skills/` — original SKILL.md prompts. Adapt into
  `services/meridian-agents/skills/{session_summarizer,project_tagger}/SKILL.md`.

## What we did NOT bring over

The full hermes platform (`cli.py`, `gateway/`, `tools/`, `plugins/`,
`hermes_cli/`, `acp_adapter/`, etc.) is not vendored — we only need the
runtime pieces an autonomous agent loop relies on. If a future feature
needs more from upstream, vendor it here first and document why.
