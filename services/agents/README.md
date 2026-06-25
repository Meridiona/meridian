# `services/agents/` — pipeline reference

Technical reference for the task classifier and Jira agents. Read this before changing the LLM provider, modifying classification logic, or working on the Jira update flow.

For a higher-level overview (installation, daemon ops, configuration), see [`services/README.md`](../README.md).

---

## Pipeline

The Rust daemon owns the pipeline (ETL, coding-agent ingest, classification
trigger, pm-worklog driver) and the database. This service is the **model
layer**: one persistent MLX server (`server.py`) that the daemon calls over HTTP
for the three jobs that need a local LLM.

```
screenpipe.db (read-only)
        │  raw OCR frames, audio, AX events, window titles
        ▼
Rust ETL daemon  (src/etl/)
        │  merges frames → app_sessions by app-switch + gap detection
        ▼
meridian.db  →  app_sessions
        │
        ├── coding-agent ingest  (src/coding_agent_session_ingest/, Rust)
        │    Indexes ~/.claude/projects/ + ~/.codex/sessions/ JSONLs into
        │    segmented rows, seals them, and summarises each sealed segment.
        │    Claude/Codex subprocesses summarise their own sessions; the
        │    MLX server's /summarise is the fallback. Rows walk task_method:
        │    coding_agent_live → pending_summariser → pending_classifier.
        │
        ├── classification  (src/intelligence/task_linker/, Rust → MLX)
        │    Rust reads unclassified rows and the pending_classifier queue,
        │    then sends:
        │      POST http://127.0.0.1:7823/classify_sessions
        │           {session_ids: [...], meridian_db: ...}
        │    MLX server (Qwen3.5-2B-OptiQ-4bit, FSM-constrained outlines):
        │      fetch session + pm_tasks + recent context
        │      → SessionClassification {task_key, session_type, confidence,
        │                               reasoning, method, dimensions}
        │    Rust writes ticket_links + session_dimensions, advances cursor.
        │
        └── pm-worklog (Stage 4)  (src/pm_worklog/, Rust → MLX)
             The Rust hour-driven driver collects each (task, hour) bundle and
             sends it to the agno worklog synthesiser hosted on the MLX server:
               POST http://127.0.0.1:7823/synthesise_worklog  {bundle: ...}
               → JiraUpdate {summary, evidence-bearing bullets, confidence}
             Rust grounds the result and DRAFTS the worklog; it posts to Jira
             (REST) only after a human approves it in the dashboard.
```

---

## Classifier detail (`task_classifier_agent.py`)

**Reads:** `session` dict (app_name, duration_s, window_titles, session_text, audio_snippets) and a list of open `pm_tasks` (task_key, title, description_text).

**Writes:** nothing directly — returns a `ClassifierDecision` dataclass; the Rust caller writes `ticket_links`.

**Skips:** sessions shorter than `MIN_LLM_DURATION_S` (default 30 s) return `routing=skip` without an LLM call.

The system prompt is loaded from `services/skills/activity/task-classifier/SKILL.md` (or `~/.meridian/skills/activity/task-classifier/SKILL.md` for user-level overrides). hermes `AIAgent` config: `enabled_toolsets=[]`, `max_iterations=1`, `quiet_mode=True`, `skip_context_files=True`, `skip_memory=True`, `load_soul_identity=False`.

Routing thresholds (env-overridable):

- `AGENT_AUTO_FLOOR` (default `0.65`) — confidence ≥ this → `routing=auto`
- `AGENT_QUEUE_FLOOR` (default `0.40`) — confidence ≥ this → `routing=queue`
- below both floors → `routing=skip`

The response parser (`_parser.py`) tolerates fenced code blocks, leading prose, and truncated JSON — thinking-style models sometimes exceed `AGENT_MAX_TOKENS` mid-string.

---

## DB schema overview

All tables below live in `meridian.db`. The Rust daemon owns DDL (`src/migrations/`); Python only does SELECT/INSERT/UPDATE.

| Table | Owner | Role |
|---|---|---|
| `app_sessions` | Rust ETL | Completed sessions — append-only, read-only from Python (`db.fetch_session`, `db.fetch_unprocessed_sessions`). |
| `pm_tasks` | Rust intelligence | Cached Jira/GitHub/Linear tasks, refreshed every ~30 min. Stage 2 reads, falls back to `agents/jira_mcp.py` when empty. |
| `ticket_links` | Python agents | One row per session: `task_key`, `session_type` (`task`/`overhead`/`unknown`), `routing` (`auto`/`queue`/`skip`), `method` (`llm_standalone` / `agent_unavailable` / `agent_invalid_response`). UNIQUE on `session_id`. |
| `session_dimensions` | Python agents | Multi-label tags written by legacy pipeline. Composite PK `(session_id, dimension, value)`. |
| `dispatch_queue` | Python agents | Pending external write-backs (Jira worklog/comment, GitHub, Linear). Drainer not yet wired (see Limitations). |
| `session_embeddings` | Legacy | Multi-vec session encoding from the old embedding pipeline. Not written by the current classifier. |
| `pm_task_embeddings` | Legacy | Task vectors from the old embedding pipeline. Not written by the current classifier. |
| `agent_runs` | Python agents | Audit log per classification batch. `'running'` rows are swept to `'aborted'` on next run. |
| `agent_cursor` | Python agents | Single-row high-water mark `last_session_id`. Advanced by Rust after each batch; never decreases. |
| `activity_context` | Legacy | Single-row "current focus" snapshot — written by the old synthesizer, read by the (deferred) jira_keeper drainer. |
| `context_graph_nodes` | Legacy | Persistent knowledge graph from the old hermes-style synthesizer. Currently unused by the 3-stage pipeline. |
| `session_summaries` | Legacy | One LLM-derived narrative summary per session. Stage 1+2+3 don't write this — left in place for a future summariser. |

Migrations: `003_intelligence.sql` (pm_tasks, ticket_links), `005_agents.sql` (agent_runs, agent_cursor, dispatch_queue, etc.), `007_session_dimensions.sql`, `008_session_embeddings.sql` (single-vec, superseded), `009_multi_sample_embeddings.sql` (drops + recreates `session_embeddings` as multi-vec).

---

## Configuration

All env vars are read in `agents/config.py`. `.env` files are loaded in priority order (earliest wins): `services/.env` → repo-root `.env`.

### Classifier

| Var | Default | Purpose |
|---|---|---|
| `MERIDIAN_DB` | `~/.meridian/meridian.db` | Path to the SQLite file. Must already exist (Rust daemon creates it). |
| `MERIDIAN_HOME` | `~/.meridian` | Parent dir; logs go under this. |
| `MIN_LLM_DURATION_S` | `30` | Sessions shorter than this (seconds) skip the LLM call entirely (`routing=skip`). |
| `CONFIDENCE_THRESHOLD` | `0.65` | Minimum confidence for a result to be used downstream (e.g. by the Jira keeper). |
| `LOG_LEVEL` | `INFO` | Standard Python log level. |

### LLM (task classifier and Jira updater)

| Var | Default | Purpose |
|---|---|---|
| `LLM_PREFER_LOCAL` | `1` | On Apple Silicon, try a running local LLM server first; fall back to cloud when none is found. Set to `0` to always use the cloud config below. |
| `LLM_BUDGET_PCT` | `0.5` | Fraction of free Metal GPU memory to allocate when starting an mlx_lm server. `0.8` recommended on 64 GB+ machines (unlocks 35B+ models). |
| `OLLAMA_MODEL` | — | Cloud fallback model ID for any OpenAI-compatible endpoint (e.g. `gpt-4o`, `claude-sonnet-4-6`). Used when no local server is detected or `LLM_PREFER_LOCAL=0`. Also the primary LLM config for the Jira updater, which has no local selection. |
| `OLLAMA_HOST` | — | Cloud fallback base URL (e.g. `https://api.openai.com/v1`, `https://openrouter.ai/api/v1`). |
| `OLLAMA_API_KEY` | — | API key for the cloud fallback endpoint. |
| `AGENT_AUTO_FLOOR` | `0.65` | Confidence ≥ this → routing `auto`. |
| `AGENT_QUEUE_FLOOR` | `0.40` | Confidence ≥ this → routing `queue`. |
| `AGENT_MAX_TOKENS` | `4000` | Per-response token cap. The JSON parser is truncation-tolerant if you raise this. |
| `AGENT_SKILL_NAME` | `task-classifier` | Subdir under `skills/activity/` to load the system prompt from. |

### Classifier thresholds

| Var | Default | Purpose |
|---|---|---|
| `AGENT_AUTO_FLOOR` | `0.65` | Confidence ≥ this → `routing=auto` (high-confidence match). |
| `AGENT_QUEUE_FLOOR` | `0.40` | Confidence ≥ this → `routing=queue` (low-confidence, human review). |
| `AGENT_MAX_TOKENS` | `4000` | Per-response token cap for the hermes AIAgent call. |
| `AGENT_SKILL_NAME` | `task-classifier` | Subdir under `skills/activity/` to load the system prompt from. |

---

## Module layout

| File | Role |
|---|---|
| `config.py` | Env loading, paths (`MERIDIAN_DB`, `LOG_DIR`), LLM config (`LLM_PREFER_LOCAL`, `LLM_BUDGET_PCT`, cloud fallback `OLLAMA_*`), Jira updater tunables. |
| `observability.py` | OpenTelemetry + JSON structured logging bootstrap. Single `setup(agent_name)` call per process. |
| `llm_selector.py` | Dynamic local LLM selection — server discovery (Ollama, LM Studio, llama.cpp, mlx_lm), MLX model catalog, managed `mlx_lm.server` lifecycle, `select_model_for_hermes()`. |
| `task_classifier_agent.py` | hermes AIAgent wrapper; calls `select_model_for_hermes()` for every invocation, falls back to cloud config when no local endpoint is available. Returns `ClassifierDecision`. |
| `run_task_linker.py` | Subprocess entry point spawned by the Rust daemon. Reads JSON from stdin (sessions + pm_tasks), calls `classify_session()` for each, writes JSON to stdout. |
| `_parser.py` | `parse_response()` — extracts `{task_key, confidence, reasoning}` from raw LLM output; truncation-tolerant. |
| `_prompts.py` | `build_user_message()` — formats a session + candidate tasks into the prompt for the task classifier. |
| `_hermes_setup.py` | Ensures the `run_agent` module is importable; switches between installed package and `services/.hermes/` dev checkout. |
| `db/` | SQLite read/write layer. Six submodules: `sessions`, `agent_runs`, `dispatch`, `jira_updates`, `context`, `connections`. Schema owned by the Rust daemon. |
| `jira_updater.py` | `run_update()` — fetches in-progress Jira tickets, queries Meridian MCP for session data, generates hermes summary, posts comment. |
| `jira_updater_daemon.py` | Long-running daemon wrapping `run_update()` on office-hour slots. CLI: `--trigger-now`, `--task`, `--dry-run`, `--interval`. |
| `run_jira_updater.py` | One-shot CLI entry point spawned by the Rust daemon for Jira updates. |
| `jira_mcp.py` | Fallback Jira task fetcher (boots `uvx mcp-atlassian` over stdio) used when `pm_tasks` is empty. |
| `jira_keeper.py` | Legacy hermes-era synthesizer. Not exercised by the current pipeline; kept for eventual dispatch_queue drainer port. |
| `bootstrap.py` | Legacy DB sanity-check script. Verifies required tables exist. |

---

## How to change the LLM

### Local server already running (LM Studio / Ollama)

No config needed. The selector auto-detects any running server and reuses its loaded model. Load whichever model you want in LM Studio or Ollama — the next classification tick picks it up automatically.

```
LM Studio  → open app, load model from the Models panel
Ollama     → ollama run <model-name>
```

### Change which MLX model loads automatically

When no external server is running, the selector starts its own `mlx_lm.server` on port 8765 and picks the largest model that fits within `metal_headroom × LLM_BUDGET_PCT`. Raise `LLM_BUDGET_PCT` to unlock larger models:

```bash
# In services/.env
LLM_BUDGET_PCT=0.8   # 80% of free Metal headroom (recommended on 64 GB+ machines)
```

No restart needed for `run_task_linker.py` (fresh subprocess per tick). If the selected model changes, the managed server is killed and restarted automatically.

### Force a specific cloud provider

Set `LLM_PREFER_LOCAL=0` and configure the cloud endpoint:

```bash
# OpenAI
OLLAMA_MODEL=gpt-4o
OLLAMA_HOST=https://api.openai.com/v1
OLLAMA_API_KEY=sk-...

# OpenRouter
OLLAMA_MODEL=anthropic/claude-sonnet-4-6
OLLAMA_HOST=https://openrouter.ai/api/v1
OLLAMA_API_KEY=sk-or-...

# Anthropic direct
OLLAMA_MODEL=claude-sonnet-4-6
OLLAMA_HOST=https://api.anthropic.com/v1
OLLAMA_API_KEY=sk-ant-...
```

The `OLLAMA_*` names are legacy — they accept any OpenAI-compatible endpoint. These vars also serve as the primary (and only) LLM config for the Jira updater, which has no local selection.

If the new model emits chain-of-thought before its JSON answer, the parser in `_parser.py` already tolerates fenced blocks and leading prose. If it consistently truncates, raise `AGENT_MAX_TOKENS` — the truncation-repair path is a fallback, not a target.

---

## Dynamic local LLM selection

`llm_selector.py` implements `select_model_for_hermes()`, called by `task_classifier_agent.py` at the start of every Stage 3 invocation. It returns a `LocalModelEndpoint` (model, base_url, api_key, runtime) or `None` to fall back to cloud.

### Decision flow

```
LLM_PREFER_LOCAL=0  ──────────────────────────────────────────► cloud fallback
LLM_PREFER_LOCAL=1 (default)
  │
  ├─ non-Apple Silicon ──────────────────────────────────────► cloud fallback
  │
  ├─ Ollama running? (:11434, /api/ps has loaded models)
  │    └─ YES ─────────────────────────────────────────────► reuse Ollama model
  │
  ├─ LM Studio running? (:1234, /api/v0/models state=loaded)
  │    └─ YES ─────────────────────────────────────────────► reuse LM Studio model
  │
  ├─ llama.cpp / mlx_lm running? (:8080)
  │    └─ YES ─────────────────────────────────────────────► reuse that model
  │
  ├─ headroom < model min_ram_gb × budget_pct ──────────────► cloud fallback
  ├─ thermal pressure high ─────────────────────────────────► cloud fallback
  │
  └─ start mlx_lm.server on :8765 with best-fitting model ─► managed MLX model
       └─ server fails to start ────────────────────────────► cloud fallback
```

### MLX model catalog

Models are ordered largest→smallest by `min_ram_gb`. Selection picks the first that fits `metal_headroom_gb × budget_pct`:

| Model | HuggingFace ID | Min RAM |
|---|---|---|
| llama3.3-70b | `mlx-community/Llama-3.3-70B-Instruct-4bit` | 40 GB |
| r1-70b | `mlx-community/DeepSeek-R1-Distill-Llama-70B-4bit` | 40 GB |
| qwen3.6-35b-moe | `mlx-community/Qwen3.6-35B-A3B-4bit` | 21 GB |
| r1-32b | `mlx-community/DeepSeek-R1-Distill-Qwen-32B-4bit` | 19 GB |
| phi-4 | `mlx-community/phi-4-4bit` | 8.5 GB |
| r1-14b | `mlx-community/DeepSeek-R1-Distill-Qwen-14B-4bit` | 8.5 GB |
| gemma3-12b | `mlx-community/gemma-3-12b-it-qat-4bit` | 7 GB |
| qwen3.5-4b | `mlx-community/Qwen3.5-4B-MLX-4bit` | 2.5 GB |
| llama3.2-3b | `mlx-community/Llama-3.2-3B-Instruct-4bit` | 1.8 GB |

Example: 28 GB headroom, `LLM_BUDGET_PCT=0.5` → budget = 14 GB → **phi-4** (first fit at 8.5 GB). Same machine with `LLM_BUDGET_PCT=0.8` → budget = 22.4 GB → **qwen3.6-35b-moe** (first fit at 21 GB).

When the screen is locked the selector uses `min(0.8, budget_pct × 1.5)` as the effective budget, allowing a larger model to load while the machine is idle.

### In-process MLX model selection (`select_mlx_model_id`)

`select_mlx_model_id` is called by the MLX server (`server.py`) at startup to pick which model to load directly into the process (via mlx_lm + outlines). It uses the same catalog but applies a three-stage priority:

1. **Preferred fits** — return the caller-supplied `preferred_hf_id` if `preferred_min_ram_gb ≤ budget`. This keeps the eval-tuned classifier model on capable machines.
2. **Largest cached model fits** — if the preferred is too large, return the largest catalog model whose files are already in the HF cache and whose `min_ram_gb ≤ budget`. Avoids surprising multi-GB downloads on constrained machines.
3. **Largest catalog model that fits (may download)** — if nothing cached fits, return the largest catalog entry where `min_ram_gb ≤ budget`, regardless of cache. This triggers a one-time download of the best available model rather than loading an oversized one that exceeds available memory. Falls back to `preferred_hf_id` only when **no catalog model fits** at all (budget so low even the 1.8 GB model won't load).

**Why stage 3 matters on low-RAM machines:** an M1 Air (8 GB) has Metal headroom ≈ 5.4 GB. At `LLM_BUDGET_PCT=0.5` the budget is ~2.7 GB. The default preferred model is 6.5 GB (`Qwen3.5-2B-OptiQ-4bit`). Without the fix, stage 3 returned the preferred unconditionally — the server then attempted to load a 6.5 GB model into a 2.7 GB budget, causing memory pressure or an outright load failure. With the fix, stage 3 selects `Qwen3.5-4B-MLX-4bit` (2.5 GB) or `Llama-3.2-3B-Instruct-4bit` (1.8 GB) — whichever is largest and fits — and downloads it on first use.

**Check what would be selected:**

```bash
cd services
.venv/bin/python -c "
from agents.llm_selector import select_mlx_model_id, probe_compute
snap = probe_compute()
print(f'Headroom: {snap.metal_headroom_gb:.1f} GB  thermal: {snap.thermal_level}')
model = select_mlx_model_id('mlx-community/Qwen3.5-2B-OptiQ-4bit', 6.5, 0.5)
print(f'Would load: {model}')
"
```

### Persistent MLX server

`_ensure_mlx_server()` manages a subprocess tracked in `~/.meridian/mlx_lm_server.pid` (JSON: pid, model, port). The model loads once and persists between `run_task_linker.py` invocations (which are fresh subprocesses each tick). If the budget changes and a different model is selected, the old server is killed and the new model loads automatically.

### Check which model is currently selected

```bash
cd services
.venv/bin/python -c "
from agents.llm_selector import discover_running_servers, select_model_for_hermes, probe_compute
stats = probe_compute()
print(f'Headroom: {stats.metal_headroom_gb:.1f} GB  chip: {stats.chip_name}')
for s in discover_running_servers():
    print(f'  running: {s.runtime} @ {s.base_url}  loaded={s.models}')
ep = select_model_for_hermes()
print(f'Selected: {ep.model}  runtime={ep.runtime}' if ep else 'No local model — cloud fallback')
"
```

### Observability

The entire classification pipeline — from the Rust ETL span that spawns the subprocess through to the LLM response parse — is a single distributed trace in OpenObserve. The Rust daemon injects a W3C `traceparent` into the JSON payload; Python picks it up and attaches all its spans as children.

**Trace hierarchy (one classification cycle):**

```
meridian-rust  run_task_linking            ← Rust entry, fields: sessions, pm_tasks, cursor
└─ meridian-task-classifier  run_task_linker.batch     fields: sessions.count, pm_tasks.count
   └─ run_task_linker.classify_one         fields: session.id, session.app_name, result.routing, result.confidence
      ├─ task_classifier.select_model      fields: llm.model, llm.runtime, llm.is_local
      │  ├─ llm_selector.discover_servers  fields: servers.found, servers.names
      │  └─ llm_selector.probe_compute     fields: compute.headroom_pct, compute.thermal_ok, compute.chip
      ├─ task_classifier.agent_call        fields: llm.model, llm.base_url, pm_tasks.count, agent.elapsed_ms
      └─ task_classifier.parse_response    fields: parsed.task_key, parsed.confidence, parsed.routing
```

**Span attributes quick reference:**

| Span | Key attributes |
|---|---|
| `run_task_linking` | `sessions`, `pm_tasks`, `cursor` |
| `run_task_linker.classify_one` | `session.id`, `session.app_name`, `session.duration_s`, `result.routing`, `result.task_key`, `result.confidence`, `result.method` |
| `task_classifier.select_model` | `llm.model`, `llm.runtime`, `llm.is_local`, `llm.budget_pct` |
| `llm_selector.probe_compute` | `compute.headroom_pct`, `compute.thermal_ok`, `compute.chip`, `compute.screen_locked` |
| `task_classifier.agent_call` | `llm.model`, `llm.base_url`, `pm_tasks.count`, `response.length`, `agent.elapsed_ms` |
| `task_classifier.parse_response` | `parsed.task_key`, `parsed.confidence`, `parsed.routing` |

Logs are correlated with traces via `trace_id` / `span_id` fields in every JSON log record. Filter on `service_name = 'meridian-task-classifier'` in OpenObserve Logs to see the full text alongside spans.

**Required env vars** (see `.env.example`):
- `MERIDIAN_OO_AUTH` — `base64(email:password)` for your OpenObserve instance; when unset, OTLP export is silently skipped
- `MERIDIAN_OTLP_TRACES_ENDPOINT` — defaults to `http://localhost:5080/api/default/v1/traces`
- `LOG_LEVEL` — Python log verbosity; `DEBUG` also dumps raw prompts and rule hits

### Installation for local inference

```bash
# Install the optional mlx-lm extra (Apple Silicon only)
cd services
pip install -e ".[local-llm]"
```

`psutil` is in core dependencies and always installed. `mlx-lm` is optional — without it, the managed MLX server path is skipped and the selector falls through to cloud.

---

## How to debug a misclassification

1. **Check what the Rust daemon stored:**
   ```sql
   -- in ~/.meridian/meridian.db
   SELECT session_id, task_key, confidence, routing, method
   FROM ticket_links WHERE session_id = <ID>;
   ```

2. **Re-run the classifier against a live session** — call the MLX server directly with the session id:
   ```bash
   curl -s -X POST http://127.0.0.1:7823/classify_sessions \
     -H "Content-Type: application/json" \
     -d "{\"session_ids\": [<ID>]}" | jq .
   ```

   Or use the standalone MLX script (reads from stdin, prints JSON to stdout):
   ```bash
   cd services
   echo '{"session_ids": [<ID>], "meridian_db": "'"$HOME"'/.meridian/meridian.db"}' \
     | .venv/bin/python -m agents.run_task_linker_mlx
   ```

3. **Check the MLX server inference log** for per-session results:
   ```bash
   ls -lt services/logs/mlx/
   tail -f services/logs/mlx/server_*.jsonl | jq '{session_id:.session_id, task_key:.result.task_key, elapsed_s:.result.elapsed_s}'
   ```

4. **Verify the MLX server is ready:**
   ```bash
   curl -s http://127.0.0.1:7823/health | jq .
   tail -f ~/.meridian/logs/mlx-server.log
   ```

5. **Tune the skill prompt** — edit `services/skills/activity/task-classifier/SKILL.md` and restart the MLX server for changes to take effect (the prompt is loaded at startup).

---

## Backfill tools

Two Rust binaries for re-running classification on past sessions. Neither
touches `agent_cursor` — safe to re-run multiple times (writes are idempotent).

### Session category backfill
Re-runs Foundation Models categorisation on a session range:
```bash
cargo run --bin backfill_session_categories -- --today
cargo run --bin backfill_session_categories -- --yesterday
cargo run --bin backfill_session_categories -- --from-date 2025-05-01 --to-date 2025-05-14
cargo run --bin backfill_session_categories -- --from-id 100 --to-id 500
cargo run --bin backfill_session_categories -- --dry-run --today
```

### Task classification backfill
Re-runs MLX session→task classification on a session range (requires the MLX server to be running):
```bash
cargo run --bin backfill_task_classification -- --today
cargo run --bin backfill_task_classification -- --yesterday
cargo run --bin backfill_task_classification -- --from-date 2025-05-01 --to-date 2025-05-14
cargo run --bin backfill_task_classification -- --from-id 100 --to-id 500
cargo run --bin backfill_task_classification -- --dry-run --today
```

---

## Limitations / known gaps

- **Jira write-backs are handled by the pm-worklog (Stage 4) driver, not from classification.** Classification only maps a session to a ticket. Jira worklogs are produced by the Rust hour-driven driver (`src/pm_worklog/`), which synthesises each worklog via the MLX server's `/synthesise_worklog` and DRAFTS it; it posts to Jira over REST only after a human approves the worklog in the dashboard (Worklogs view). The `dispatch_queue` table is reserved for future GitHub/Linear write-backs and is not yet drained.
- **`pm_tasks` populated by Rust.** The classifier receives `pm_tasks` as part of the JSON payload from the Rust daemon. If the Rust intelligence module hasn't synced tasks yet (clean install, first run), `pm_tasks` will be empty and every session gets `routing=skip`. Run the Rust daemon for at least one full cycle first.
- **No re-classification UI.** There is no CLI to re-run classification on a single session — construct the JSON payload manually (see debugging guide above) or trigger the Rust daemon.
- **Coding-agent sessions are classified on their summary, not their transcript.** The Rust coding-agent ingest (`src/coding_agent_session_ingest/`) seals and summarises each segment first; rows walk `coding_agent_live → pending_summariser → pending_classifier`, and only then does the classification trigger send them to the MLX server. A coding session therefore appears as a worklog candidate only after its summary exists.
- **The legacy modules (`jira_keeper.py`, `bootstrap.py`) and DB tables (`activity_context`, `context_graph_nodes`, `session_summaries`)** are kept in place for the eventual dispatcher port. They are not exercised by the current pipeline.
