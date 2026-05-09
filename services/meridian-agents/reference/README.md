# Reference: hermes-activity-agent source material

Source files used as a porting reference. **Off the Python import path.**

The active hermes runtime is the full upstream `hermes-agent` v2026.5.7,
vendored at `../vendor/hermes/` and installed via uv from local path. This
`reference/` directory holds smaller artefacts we lifted from the
hermes-activity-agent demo (in `~/Documents/Learning/hermes-activity-agent/`)
when we were initially mapping out the synthesizer's behaviour. They serve
as templates for the upcoming meridian agents but are not imported anywhere.

## Contents

- `LICENSE.hermes` — MIT license from upstream. Preserve attribution when
  copying any further files.
- `hermes_agents/` — the original `agents/` directory:
  - `watcher.py` — polls Screenpipe MCP every 3 min; meridian replaced this
    with direct reads from `app_sessions` / `active_session` (no separate
    watcher agent).
  - `synthesizer.py` — every 20 min, infers task / project from buffered
    activity. Source material for `meridian_agents/agents/synthesizer.py`,
    which now also handles per-session task tagging.
  - `jira_keeper.py` — syncs to Jira via mcp-atlassian. Source material for
    `meridian_agents/agents/jira_keeper.py`, which uses Rovo Atlassian MCP
    + cross-checks `pm_tasks`.
  - `orchestrator.py` — asyncio coordinator. Replaced by the upcoming
    meridian-side orchestrator with a hybrid time + session trigger.
  - `bootstrap.py` — state-dir init under `~/.hermes/`. Not needed —
    meridian persists everything in `meridian.db`.
  - `config.py` — env-var + skill loader. Replaced by
    `src/meridian_agents/skills.py` for skill loading.
- `hermes_skills/` — original SKILL.md prompts. The current synthesizer
  prompt at `services/meridian-agents/skills/synthesizer/SKILL.md` was
  written from scratch but draws on these.

## What we did NOT bring over here

The full hermes platform (`cli.py`, `gateway/`, `tools/`, `plugins/`,
`hermes_cli/`, `acp_*`, etc.) is not in this `reference/` dir. Those live
in the active vendor at `../vendor/hermes/` and are installed by uv as
`hermes-agent`.
