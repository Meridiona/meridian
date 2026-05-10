# Active Intelligence

A small system of three cooperating agents that watch your screen activity, infer what you're working on, and (optionally) sync that context back into Jira.

The agents themselves live in [`agents/`](agents/). Everything else in this repo is the
[Hermes](https://github.com/NousResearch/hermes-agent) framework code that the agents depend on for
LLM orchestration, MCP plumbing, and tool dispatch.

---

## How it works

Three agents run in a coordinated asyncio loop driven by [`agents/orchestrator.py`](agents/orchestrator.py):

```
┌──────────┐   buffer.jsonl   ┌──────────────┐  current_context.json   ┌──────────────┐
│ Watcher  │ ───────────────> │ Synthesizer  │ ──────────────────────> │ Jira Keeper  │
│ 3 min    │                  │  20 min      │                         │ on demand    │
└──────────┘                  └──────────────┘                         └──────────────┘
     │                              │                                        │
     ▼                              ▼                                        ▼
 Screenpipe MCP              Python tool calls                       mcp-atlassian MCP
 (npx)                       (write_current_context,                 (uvx)
                              upsert_context_map_node)
```

| Agent | File | Cadence | What it does |
|---|---|---|---|
| **Watcher** | [`agents/watcher.py`](agents/watcher.py) | every 3 min | Asks the LLM to call Screenpipe MCP tools and emit a structured JSON activity event. Appends to `~/.hermes/activity/buffer.jsonl`. |
| **Synthesizer** | [`agents/synthesizer.py`](agents/synthesizer.py) | every 20 min | Reads the recent buffer + the persistent context map, asks the LLM to infer the current project / Jira ticket / task, and updates `~/.hermes/activity/current_context.json`. |
| **Jira Keeper** | [`agents/jira_keeper.py`](agents/jira_keeper.py) | when `trigger_jira_sync == true` and confidence ≥ 0.65 | Logs time, posts a progress comment, and transitions status on the inferred Jira ticket via `mcp-atlassian`. |
| **Orchestrator** | [`agents/orchestrator.py`](agents/orchestrator.py) | — | Single asyncio process running the three loops. `--once` runs one full pipeline pass for debugging. |
| **Bootstrap** | [`agents/bootstrap.py`](agents/bootstrap.py) | one-time | Creates `~/.hermes/{activity,jira,memories,logs}/` and seeds JSON state files; can wire Screenpipe into `~/.hermes/config.yaml`. |

Each agent uses an `AIAgent` (from [`run_agent.py`](run_agent.py)) and patches `handle_function_call` so MCP and Python-side tools route through small per-agent bridges.

State lives outside the repo, under `~/.hermes/`:

```
~/.hermes/
├── activity/
│   ├── buffer.jsonl           ← Watcher events
│   ├── context_map.json       ← persistent knowledge graph
│   └── current_context.json   ← latest inferred context
├── jira/
│   ├── jira_state.json        ← per-ticket sync history
│   └── ticket_mappings.json
└── logs/activity-agent.log
```

The system prompts for each agent live in [`skills/activity/{watcher,synthesizer,jira-keeper}/SKILL.md`](skills/activity/) and are loaded by [`agents/config.py`](agents/config.py).

---

## Setup

### Prerequisites

- Python 3.10+
- `npx` (for `screenpipe-mcp`) — install Node.js if missing
- `uvx` (for `mcp-atlassian`) — `pip install uv` or `brew install uv`
- The [Screenpipe](https://screenpi.pe) desktop app, running, with screen capture permissions granted
- An LLM endpoint (Ollama Cloud, Anthropic, OpenAI, LM Studio…)
- (Optional) An Atlassian Cloud account with a Jira API token, if you want the Jira Keeper to actually sync

### Install

```bash
git clone <this-repo> hermes-activity-agent
cd hermes-activity-agent

python3 -m venv .venv
source .venv/bin/activate
pip install -e .

cp .env.example .env
# Edit .env — at minimum set your LLM provider key + (optionally) JIRA_*

python -m agents.bootstrap --add-mcp
```

`bootstrap.py` creates the state directories under `~/.hermes/` and (with `--add-mcp`) wires `screenpipe-mcp` into `~/.hermes/config.yaml`.

### Run

One pass through the full pipeline (good for debugging):

```bash
python -m agents.orchestrator --once
```

Long-running loop:

```bash
python -m agents.orchestrator
# or, with the wrapper script:
./start-activity-agent.sh
```

Each agent module is also runnable standalone for targeted testing:

```bash
python -m agents.watcher
python -m agents.synthesizer
python -m agents.jira_keeper
```

---

## Configuration

The defaults are in [`agents/config.py`](agents/config.py). Override at runtime via env vars:

| Variable | Default | Purpose |
|---|---|---|
| `HERMES_MODEL` | `nemotron-3-super` | LLM model name |
| `HERMES_BASE_URL` | `https://ollama.com/v1` | OpenAI-compatible endpoint |
| `OLLAMA_API_KEY` | — | API key for the default endpoint |
| `JIRA_URL` | — | e.g. `https://yourorg.atlassian.net` |
| `JIRA_EMAIL` | — | Account email |
| `JIRA_API_TOKEN` | — | Atlassian API token |

Loop cadences and thresholds (edit [`agents/config.py`](agents/config.py)):

- `WATCHER_INTERVAL_SECONDS = 180`
- `SYNTHESIZER_INTERVAL_SECONDS = 1200`
- `CONFIDENCE_THRESHOLD = 0.65` (below this, Jira sync is skipped)
- `BUFFER_WINDOW_MINUTES = 30`

---

## Repository layout

```
agents/                     ← The 5 agent modules — start reading here
  bootstrap.py              ← one-time state setup
  config.py                 ← env vars, paths, skill loader
  watcher.py                ← Screenpipe → buffer.jsonl
  synthesizer.py            ← buffer → current_context.json
  jira_keeper.py            ← current_context.json → Jira via mcp-atlassian
  orchestrator.py           ← asyncio loop coordinator
skills/activity/            ← System prompts for each agent
  watcher/SKILL.md
  synthesizer/SKILL.md
  jira-keeper/SKILL.md
skills/activity-intelligence/
                            ← Earlier-iteration source skills (kept for reference)

run_agent.py                ← The Hermes AIAgent class the agents wrap
agent/, tools/, hermes_cli/ ← Hermes framework support modules
toolsets.py, model_tools.py, utils.py, hermes_*.py
                            ← More framework support
plugins/, providers/, gateway/, cli.py …
                            ← Other Hermes framework modules; pulled in
                              transitively by run_agent.py and friends
start-activity-agent.sh     ← Convenience launcher
pyproject.toml              ← Dependency manifest
```

---

## Credits

- Agent design and orchestration: this repo
- LLM/MCP plumbing, `AIAgent`, framework code: [Nous Research Hermes](https://github.com/NousResearch/hermes-agent) (Apache-2.0)

See `LICENSE` for the upstream Hermes license.
