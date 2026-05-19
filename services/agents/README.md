# `services/agents/` — pipeline reference

Deep technical reference for the 3-stage tagger. Read this before modifying any stage, adding a rule, swapping the embedding model, or changing the LLM provider.

For a higher-level overview (what this service is, how to install the daemon, common ops), see [`services/README.md`](../README.md).

---

## Pipeline

```
app_sessions (Rust ETL writes)
        │
        │  daemon polls every TAGGER_TICK_SECS
        │  picks rows with id > agent_cursor.last_session_id
        ▼
┌──────────────────────────────────────────────────────────────────────┐
│ tagger.run_once  (agents/tagger.py:335)                              │
│                                                                      │
│   for each session in id-ascending order:                            │
│     ┌──────────────────────────────────────┐                         │
│     │ trivial-overhead pre-filter          │  duration < MIN_LLM_DUR │
│     │   write ticket_links/overhead/skip   │  OR no titles/ocr/audio│
│     │   advance cursor; next session       │                         │
│     └──────────────────────────────────────┘                         │
│                       │ otherwise                                    │
│                       ▼                                              │
│   ┌──────── Stage 1  rules + ticket regex ────────┐                  │
│   │ run_rules() → resolve_hits() → upsert dims    │                  │
│   │ extract_tickets() ∩ pm_tasks                  │                  │
│   │   match  → ticket_links task/auto  (final)    │                  │
│   │   shaped → ticket_links task/skip  (final)    │                  │
│   │   none   → defer to Stage 2                   │                  │
│   └──────────────────────────────────────────────┘                  │
│                       │  (only when Stage 1 deferred)                │
│                       ▼                                              │
│   ┌──────── Stage 2  embedding match ─────────────┐                  │
│   │ stage2_match():                                │                  │
│   │   embed session as multi-vec (titles+audio+OCR)│                  │
│   │   cosine_max @ pm_task_embeddings              │                  │
│   │   blend: 0.55*cos + 0.30*dim + 0.15*past_vote  │                  │
│   │   route: auto | queue | skip                   │                  │
│   └────────────────────────────────────────────────┘                 │
│                       │  (only when Stage 2 routing=queue)           │
│                       ▼                                              │
│   ┌──────── Stage 3  LLM tiebreaker ───────────────┐                 │
│   │ hermes AIAgent (skill: stage3-tiebreaker)      │                 │
│   │   one shot, no tools, JSON in / JSON out       │                 │
│   │   wins iff routing != skip, else fall back to S2│                │
│   └────────────────────────────────────────────────┘                 │
│                                                                      │
│     advance_cursor(session.id)                                       │
└──────────────────────────────────────────────────────────────────────┘
```

The cursor is advanced after **every** session, regardless of which stages ran. A SIGTERM mid-batch loses at most the in-flight session; everything written before the advance is durable.

---

## Per-stage detail

### Stage 1 — rules + regex (`agents/tagger.py`, `agents/rules/`)

**Reads:** `app_sessions` row (window titles, OCR samples, audio snippets, app name, duration).
**Writes:** `session_dimensions` (multi-label tags, idempotent UPSERT on `(session_id, dimension, value)`); `ticket_links` only when a regex hit matches a known `pm_tasks.task_key`, OR when a ticket-shaped string is seen but doesn't match (recorded as `task/skip`).
**Skips:** trivial-overhead sessions (`duration_s < MIN_LLM_DURATION_S` or no signal) — written as `overhead/skip` with method `stage1_prefilter`, never see Stages 2 or 3.

The rule registry lives in `agents/rules/__init__.py:50`. Rules self-register via the `@rule(name=…, dim=…)` decorator. Each rule receives a session dict and returns `RuleHit | list[RuleHit] | None`.

Built-in rules: `activity.py`, `intent.py`, `engagement.py`, `collaboration.py`, `tool.py`, `topic.py`, `practice.py`, `ticket.py`.

The taxonomy (allowed dimensions, closed-vocab values, single- vs multi-value flag) is in `agents/taxonomy.py`. Hits with unknown dimensions or unknown values on closed dimensions are dropped with a warning — never persisted (`agents/rules/__init__.py:182`).

### Stage 2 — embedding match (`agents/stage2.py`)

**Reads:** the session row, all `pm_tasks` (open, sorted by updated_at), `session_dimensions` (for `dim_overlap`), `session_embeddings` of past sessions whose `ticket_links.method` is **not** `stage2*` (anti-self-reinforcement filter).
**Writes:** `session_embeddings` (multi-row, one per sample), `pm_task_embeddings` (single-row per task, refreshed when `text_hash` changes), and the final `ticket_links` row (method `stage2_embed`).
**Skips:** when `pm_tasks` is empty, when the session has no extractable text, or when no candidates score above the queue threshold.

Score formula:

```
score = 0.55 * cosine_unit + 0.30 * dim_overlap + 0.15 * past_vote
```

Re-normalised when one component has no signal:

- no past history → `0.65 * cos + 0.35 * dim`
- empty session dims → `0.75 * cos + 0.25 * past`
- nothing else available → `1.0 * cos`

Routing:

- `auto` — `top1 ≥ 0.62` AND `top1 - top2 ≥ 0.08`
- `queue` — `top1 ≥ 0.40` (sent to Stage 3 if enabled)
- `skip` — anything below

Multi-vec MaxSim: each session is encoded as `(M, 384)` where M = 1 (titles) + 1 (audio) + N (per-OCR-sample, capped at 20 by the ETL). Per-task cosine is `max over session samples`. See migration `009_multi_sample_embeddings.sql` for the rationale.

### Stage 3 — LLM tiebreaker (`agents/stage3.py`)

**Reads:** the session, Stage-1 `session_dimensions`, Stage-2 top candidates, and the `pm_tasks` rows for those candidates (for title + description).
**Writes:** the final `ticket_links` row (method `stage3_llm`) and a `dispatch_queue` row when `routing in ('auto', 'queue')`.
**Skips:** the LLM returns `null`, returns invalid JSON, returns a `task_key` not in the candidate set, or hermes `AIAgent` import fails. In any of these the Stage-2 verdict stands.

System prompt loaded from `services/skills/activity/stage3-tiebreaker/SKILL.md` (or `~/.meridian/skills/activity/stage3-tiebreaker/SKILL.md` for user-level overrides). hermes `AIAgent` config: `enabled_toolsets=[]`, `max_iterations=1`, `quiet_mode=True`, `skip_context_files=True`, `skip_memory=True`, `load_soul_identity=False`.

Routing thresholds (env-overridable):

- `STAGE3_AUTO_FLOOR` (default `0.65`)
- `STAGE3_QUEUE_FLOOR` (default `0.40`)

The parser (`_extract_json` in `stage3.py:154`) tolerates fenced blocks, leading prose, and **truncated** JSON (thinking-style models sometimes blow through `STAGE3_MAX_TOKENS` mid-string).

---

## DB schema overview

All tables below live in `meridian.db`. The Rust daemon owns DDL (`src/migrations/`); Python only does SELECT/INSERT/UPDATE.

| Table | Owner | Role |
|---|---|---|
| `app_sessions` | Rust ETL | Completed sessions — append-only, read-only from Python (`db.fetch_session`, `db.fetch_unprocessed_sessions`). |
| `pm_tasks` | Rust intelligence | Cached Jira/GitHub/Linear tasks, refreshed every ~30 min. Stage 2 reads, falls back to `agents/jira_mcp.py` when empty. |
| `ticket_links` | Python tagger | One row per session: `task_key`, `session_type` (`task`/`overhead`/`unknown`), `routing` (`auto`/`queue`/`skip`), `method` (`stage1_regex` / `stage1_prefilter` / `stage2_embed` / `stage3_llm`). UNIQUE on `session_id`. |
| `session_dimensions` | Python tagger | Multi-label tags. Composite PK `(session_id, dimension, value)`. Conflict policy: keep MAX confidence, replace source. |
| `dispatch_queue` | Python tagger | Pending external write-backs (Jira worklog/comment, GitHub, Linear, log no-op). Drainer not yet wired (see Limitations). |
| `session_embeddings` | Python tagger | Multi-vec session encoding. PK `(session_id, model, sample_idx)`. Replaced wholesale on each upsert (sample count varies). |
| `pm_task_embeddings` | Python tagger | One vector per task, plus JSON `expected_dims` for `dim_overlap`. PK `(task_key, model)`. Re-embedded only when `text_hash` changes. |
| `agent_runs` | Python tagger | Audit log per `tagger.run_once` cycle. `'running'` rows are swept to `'aborted'` on daemon startup. |
| `agent_cursor` | Python tagger | Single-row high-water mark `last_session_id`. Never decreases (the SQL has `WHERE ? > last_session_id`). |
| `activity_context` | Legacy | Single-row "current focus" snapshot — written by the old synthesizer, read by the (deferred) jira_keeper drainer. |
| `context_graph_nodes` | Legacy | Persistent knowledge graph from the old hermes-style synthesizer. Currently unused by the 3-stage pipeline. |
| `session_summaries` | Legacy | One LLM-derived narrative summary per session. Stage 1+2+3 don't write this — left in place for a future summariser. |

Migrations: `003_intelligence.sql` (pm_tasks, ticket_links), `005_agents.sql` (agent_runs, agent_cursor, dispatch_queue, etc.), `007_session_dimensions.sql`, `008_session_embeddings.sql` (single-vec, superseded), `009_multi_sample_embeddings.sql` (drops + recreates `session_embeddings` as multi-vec).

---

## Configuration

All env vars are read in `agents/config.py`. `.env` files are loaded in priority order (earliest wins): `services/.env` → repo-root `.env`.

### Pipeline scope

| Var | Default | Purpose |
|---|---|---|
| `MERIDIAN_DB` | `~/.meridian/meridian.db` | Path to the SQLite file. Must already exist (Rust daemon creates it). |
| `MERIDIAN_HOME` | `~/.meridian` | Parent dir; logs and `tagger.config.json` go under this. |
| `SESSION_BATCH_LIMIT` | `50` | Max sessions per `tagger.run_once` cycle. |
| `ONLY_TODAY` | `1` | When truthy, skip sessions whose `started_at` is before today's local-midnight (UTC-converted). Set `0` to backfill history. |
| `MIN_LLM_DURATION_S` | `30` | Stage-1 trivial-overhead floor — sessions shorter than this are auto-tagged `overhead/skip`. |

### Stage on/off

| Var | Default | Purpose |
|---|---|---|
| `STAGE1_ENABLED` | `1` | Turn off Stage 1 (rules + regex). Almost never useful. |
| `STAGE2_ENABLED` | `1` | Turn off Stage 2 (embeddings). Falls back to Stage 1 only. |
| `STAGE3_ENABLED` | `1` | Turn off Stage 3 (LLM). Stage 2 verdict is final. |

Booleans accept `1` / `true` / `yes` / `on` (truthy) and `0` / `false` / `no` / `off` / `""` (falsy).

### Daemon loop

| Var | Default | Purpose |
|---|---|---|
| `TAGGER_TICK_SECS` | `7` | Poll cadence in seconds. |
| `TAGGER_HEARTBEAT_SECS` | `300` | Heartbeat log when idle (no new sessions). |
| `TAGGER_STAGES` | `auto` | Legacy override. `auto` (the default) honours `STAGE{1,2,3}_ENABLED` + the override file; an explicit list (`1,2`) freezes the stage set. |
| `TAGGER_CONFIG_FILE` | `~/.meridian/tagger.config.json` | Hot-toggle override file (see below). |
| `LOG_LEVEL` | `INFO` | Standard Python log level. `DEBUG` dumps full session bundles + raw rule output. |

### LLM (Stage 3 and task classifier)

| Var | Default | Purpose |
|---|---|---|
| `LLM_PREFER_LOCAL` | `1` | On Apple Silicon, try a running local LLM server first; fall back to cloud when none is found. Set to `0` to always use the cloud config below. |
| `LLM_BUDGET_PCT` | `0.5` | Fraction of free Metal GPU memory to allocate when starting an mlx_lm server. `0.8` recommended on 64 GB+ machines (unlocks 35B+ models). |
| `OLLAMA_MODEL` | — | Cloud fallback model ID for any OpenAI-compatible endpoint (e.g. `gpt-4o`, `claude-sonnet-4-6`). Used when no local server is detected or `LLM_PREFER_LOCAL=0`. Also the primary LLM config for the Jira updater, which has no local selection. |
| `OLLAMA_HOST` | — | Cloud fallback base URL (e.g. `https://api.openai.com/v1`, `https://openrouter.ai/api/v1`). |
| `OLLAMA_API_KEY` | — | API key for the cloud fallback endpoint. |
| `STAGE3_AUTO_FLOOR` | `0.65` | LLM confidence ≥ this → routing `auto`. |
| `STAGE3_QUEUE_FLOOR` | `0.40` | LLM confidence ≥ this → routing `queue`. |
| `STAGE3_MAX_TOKENS` | `4000` | Per-response cap. The JSON parser is truncation-tolerant if you tighten this. |
| `STAGE3_SKILL_NAME` | `stage3-tiebreaker` | Subdir under `skills/activity/` to load the system prompt from. |

### Bounded retry

| Var | Default | Purpose |
|---|---|---|
| `LLM_RETRY_ATTEMPTS` | `3` | Retries on HTTP 429 / transient failure. |
| `LLM_RETRY_BACKOFF_S` | `5` | Backoff between attempts. |

---

## Hot-toggle: `~/.meridian/tagger.config.json`

The daemon re-reads this file every tick when launched with the default `--stage auto`. File schema:

```json
{ "stage1": true, "stage2": false, "stage3": true }
```

Resolution order: file present → file wins; file absent → env defaults (`STAGE{1,2,3}_ENABLED`).

CLI helpers (you almost never want to hand-edit JSON):

```bash
python -m agents.tagger --stages-status         # show env / file / resolved
python -m agents.tagger --enable-stage 3
python -m agents.tagger --disable-stage 3
python -m agents.tagger --clear-stages-override # delete the file → fall back to env
```

When you launch the daemon with an **explicit** `--stage 1,2`, live mode is off and the stage set is frozen for the lifetime of that process. Passing `--stage auto` (or no `--stage`) keeps live mode on.

---

## Module layout

| File | Role |
|---|---|
| `config.py` | Env loading, paths (`MERIDIAN_DB`, `LOG_DIR`), stage flags, override-file helpers, LLM config (`LLM_PREFER_LOCAL`, `LLM_BUDGET_PCT`, cloud fallback `OLLAMA_*`). |
| `db.py` | SQLite connection + every read/write the tagger does. Schema is read-only here. |
| `tagger.py` | Stage-1 driver, single-session inspector (`--session`), `run_once` entry point. |
| `tagger_daemon.py` | Long-running launchd-supervised loop wrapping `tagger.run_once`. Zombie sweep, hot-toggle, backoff. |
| `rules/__init__.py` | Rule decorator + registry, hit resolver, shared text helpers (`session_text`, `extract_tickets`, `extract_urls`). |
| `rules/{activity,intent,engagement,collaboration,tool,topic,practice,ticket}.py` | The actual rule library. |
| `taxonomy.py` | Allowed dimensions + closed-vocab values + `SINGLE_VALUE_DIMENSIONS`. The runner drops unknown values silently with a warning. |
| `embeddings.py` | Lazy-loads `BAAI/bge-small-en-v1.5`, BLOB ↔ ndarray, multi-vec session encoder, brute-force cosine. |
| `text_for_embedding.py` | Deterministic recipe for "what text represents this session / task" + `text_hash` for change detection. |
| `stage2.py` | Embedding match, dim_overlap blend, past_vote MaxSim, routing decision. |
| `stage3.py` | hermes `AIAgent` wrapper, prompt builder, JSON parser (truncation-tolerant), routing decision. |
| `llm_selector.py` | Dynamic local LLM selection — server discovery (Ollama, LM Studio, llama.cpp, mlx_lm), MLX model catalog, managed `mlx_lm.server` lifecycle, `select_model_for_hermes()`. |
| `task_classifier_agent.py` | Calls `select_model_for_hermes()` before each AIAgent invocation; falls back to cloud config when no local endpoint is available. |
| `jira_keeper.py` | **Deprecated** — old hermes-style synthesizer that wrote to Jira via `mcp-atlassian`. Not yet replaced; the dispatch_queue drainer that should subsume it is unimplemented. |
| `jira_mcp.py` | Fallback Jira fetcher when `pm_tasks` is empty (boots `uvx mcp-atlassian` over stdio). The Rust daemon owns the long-term cache. |
| `bootstrap.py` | One-time setup helpers (legacy — most paths now created on demand). |

---

## How to add a new rule

1. Pick the dimension. Closed-vocab dimensions (`activity`, `intent`, `engagement`, `collaboration`) require values from `agents/taxonomy.py`; open ones (`tool`, `topic`, `practice`) accept any string.
2. Add a function in the appropriate `agents/rules/<dimension>.py`:

   ```python
   from agents.rules import rule, RuleHit, session_text

   @rule(name="cargo_test_visible", dim="practice")
   def cargo_test_rule(session: dict):
       text = session_text(session).lower()
       if "cargo test" in text or "running tests" in text:
           return RuleHit(dimension="practice", value="tests_written", confidence=0.85)
       return None
   ```

3. The rule auto-registers when the module is imported. `discover_rules()` in `agents/rules/__init__.py:150` iterates every submodule on tagger startup, so no manual registration needed.
4. Verify with `python -m agents.tagger --session <ID> --dry-run` — the inspector prints every fire under `STAGE 1 — RULES`.
5. If the dimension or value is new, add it to `agents/taxonomy.py` first. The runner drops unknown dim/value pairs with a warning.

---

## How to swap the embedding model

1. Pick a new SentenceTransformer-compatible model. Same dim → easier (no schema change). Different dim → still safe, the `dim` column on each row scopes the index.
2. Edit `agents/embeddings.py`:
   - `EMBED_MODEL_NAME = "..."`
   - `EMBED_MODEL_SHORT = "..."` (this is what gets written to the `model` column)
   - `EMBED_DIM = ...`
3. **Add a new migration** that recreates `session_embeddings` and `pm_task_embeddings` if the dim changed (existing BLOBs are wrong-sized). Otherwise the next tick will re-embed every session/task with the new model name and the old rows linger harmlessly.
4. Drop old rows or wait — `text_hash` mismatches force re-embed on the next access.

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

If the new model emits chain-of-thought before its JSON answer, the parser at `stage3.py:154` already tolerates fenced blocks and leading prose. If it consistently truncates, raise `STAGE3_MAX_TOKENS` — the truncation-repair path is a fallback, not a target.

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

Every `task_classifier_agent.decide` span carries three attributes you can filter on in OpenObserve Traces:

| Span attribute | Example values |
|---|---|
| `llm.model` | `mlx-community/Qwen3.6-35B-A3B-4bit`, `gpt-4o` |
| `llm.runtime` | `lmstudio`, `ollama`, `mlx_managed`, `cloud` |
| `llm.is_local` | `true` / `false` |

Every `run_task_linker` result log line also emits `llm_model`, `llm_runtime`, `llm_is_local` as top-level JSON fields, queryable by field filter in OpenObserve Logs.

### Installation for local inference

```bash
# Install the optional mlx-lm extra (Apple Silicon only)
cd services
pip install -e ".[local-llm]"
```

`psutil` is in core dependencies and always installed. `mlx-lm` is optional — without it, the managed MLX server path is skipped and the selector falls through to cloud.

---

## How to debug a misclassification

Order from cheapest to most invasive:

1. `python -m agents.tagger --show <ID>` — read-only dump of what's stored.
2. `python -m agents.tagger --session <ID> --dry-run` — re-run all stages with full logging, no DB writes.
3. `python -m agents.tagger --session <ID>` — same, but persists. Resets dims + ticket_link first.
4. `python -m agents.tagger --list-recent 50 --all-history` — overview of recent tagging quality.
5. `python -m agents.tagger --stages-status` — confirm which stages are actually running.
6. Tail logs:
   - `~/.meridian/logs/tagger.log` — CLI runs
   - `~/.meridian/logs/tagger-daemon.log` — long-running daemon
   - `~/.meridian/logs/tagger-daemon.err` — launchd stderr
7. SQL spot checks:
   ```sql
   SELECT * FROM ticket_links WHERE session_id = <ID>;
   SELECT * FROM session_dimensions WHERE session_id = <ID> ORDER BY dimension, confidence DESC;
   SELECT method, COUNT(*) FROM ticket_links GROUP BY method;
   ```
8. `LOG_LEVEL=DEBUG python -m agents.tagger --session <ID>` — full session bundle + raw rule hits.

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
Re-runs hermes session→task classification on a session range:
```bash
cargo run --bin backfill_task_classification -- --today
cargo run --bin backfill_task_classification -- --yesterday
cargo run --bin backfill_task_classification -- --from-date 2025-05-01 --to-date 2025-05-14
cargo run --bin backfill_task_classification -- --from-id 100 --to-id 500
cargo run --bin backfill_task_classification -- --dry-run --today
```

---

## Limitations / known gaps

- **`dispatch_queue` drainer is not implemented.** Stage 1/2/3 enqueue rows with `state='pending'`, but nothing currently moves them to `'sent'`. The old `agents/jira_keeper.py` was the hermes-era equivalent and hasn't been ported to read from the queue. Until the drainer ships, no automatic Jira write-backs happen.
- **`past_vote` requires history.** The first ~50 tagged sessions of a clean install run with `has_past=False` and the `0.65 * cos + 0.35 * dim` cold-start blend.
- **Self-reinforcement avoidance.** `_past_vote` filters out neighbours whose `ticket_links.method LIKE 'stage2%'`, so Stage 2 only votes from Stage-1 regex matches and (eventually) human-confirmed tags. This is correct but means past_vote only kicks in for sessions whose rule-based ticket extraction worked — which is exactly the cases Stage 2 doesn't run for. The signal is weaker than it looks.
- **OCR domination.** A single noisy OCR sample (VS Code extension banner, Claude.ai sidebar) used to dominate the single-vec embedding. Migration `009` switched to multi-vec MaxSim to fix this; if you see it recur, check `text_for_embedding._VSCODE_BANNER_RE` and similar guards.
- **`_TICKET_FALSE_POSITIVES`** in `rules/__init__.py:69` is hand-curated. New regex false positives need to be added there (e.g. `GPT-5`, `HTTP-429`).
- **The legacy modules (`jira_keeper.py`, `bootstrap.py`, `activity_context`, `context_graph_nodes`, `session_summaries`)** are kept in place for the eventual dispatcher port. They are NOT exercised by the current pipeline.
