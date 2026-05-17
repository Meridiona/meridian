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

### LLM (Stage 3 only)

| Var | Default | Purpose |
|---|---|---|
| `HERMES_MODEL` | `nemotron-3-super` | Model name passed to `AIAgent`. |
| `HERMES_BASE_URL` | `https://ollama.com/v1` | OpenAI-compatible endpoint. |
| `OLLAMA_API_KEY` | — | API key (also doubles as the OpenAI-compat key). |
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
| `config.py` | Env loading, paths (`MERIDIAN_DB`, `LOG_DIR`), stage flags, override-file helpers, LLM creds. |
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

## How to swap the LLM provider for Stage 3

The hermes `AIAgent` is OpenAI-compatible-API generic. Just set:

```bash
export HERMES_BASE_URL=https://api.anthropic.com/v1     # or LM Studio, Ollama, etc.
export HERMES_MODEL=claude-3-5-sonnet-20241022
export OLLAMA_API_KEY=<provider key>                    # name is legacy
```

No code change. Restart the daemon (`launchctl kickstart -k gui/$(id -u)/com.meridiona.tagger-daemon`). The model + endpoint are logged at the top of every Stage-3 invocation (`stage3: model=… base_url=…`).

If the new model is noisier (e.g. emits chain-of-thought before the JSON), the parser at `stage3.py:154` already tolerates fenced blocks and leading prose. If it consistently truncates, lower `STAGE3_MAX_TOKENS` is the wrong fix — raise it. The truncation-repair path is a fallback, not a target.

---

## Dynamic local LLM selection

When `LLM_PREFER_LOCAL=1` (the default), Stage 3 attempts to route hermes through a local LLM endpoint before falling back to the static cloud config. The feature is implemented in `llm_selector.py` and wired into `task_classifier_agent.py`.

**Selection priority:** The function `select_model_for_hermes()` checks for available endpoints in this order:

1. Any already-running LLM server (Ollama on port 11434, LM Studio on 1234, llama.cpp/mlx_lm on 8080) with a model loaded in memory — zero load cost.
2. Start a persistent `mlx_lm.server` process on port 8765, selecting the best-fitting MLX model by available Metal GPU headroom × `LLM_BUDGET_PCT`.
3. Return `None` — caller uses static `MODEL`/`BASE_URL` from env or config.

**Persistent MLX server:** When option 2 runs, `_ensure_mlx_server()` manages a subprocess tracked in `~/.meridian/mlx_lm_server.pid` (JSON: pid, model, port). If the same model is already running, returns immediately. If a different model is needed, kills and restarts. The model loads once and persists between `run_task_linker.py` invocations, avoiding repeated cold-start costs.

Configuration:

| Var | Default | Purpose |
|---|---|---|
| `LLM_PREFER_LOCAL` | `1` | Try local model before cloud when Stage 3 runs. |
| `LLM_BUDGET_PCT` | `0.5` | Fraction of free GPU memory to use when loading MLX models (0.0–1.0). |

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
