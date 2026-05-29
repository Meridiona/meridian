# `services/tests/evals/` — classifier eval harness

The experimentation harness for the MLX session-classifier. Scores predictions against a hand-authored golden dataset, emits OTel spans to OpenObserve so each run is inspectable as a trace tree, and serves as the gate that every model swap / prompt edit / parameter tweak must pass.

For the full failure-mode taxonomy and reasoning-about-evals primer, see [`TESTING.md` §9](../../../TESTING.md#9-classifier-eval-pipeline-deepeval--golden-dataset).

---

## File inventory

### Data

| Path | Role |
|---|---|
| `data/seeds/sessions_<persona>.json` | Hand-authored seed sessions. One file per persona: `sessions_a_meridian.json` (Meridian dev, 35 sessions), `sessions_b_generic.json` (generic SaaS dev, 35 sessions). Each session has structured fields + `ground_truth` + `design_notes` flagging the failure mode it targets. |
| `data/seeds/tickets_<persona>.json` | Open ticket list the classifier picks from. `tickets_meridian.json` = 5 real KAN-* + 2 synthetic decoys. `tickets_generic.json` = 5 real PROJ-* + 2 synthetic decoys. |
| `data/generated/` | **Gitignored.** All rendered outputs go here — regenerate on demand. |
| `data/generated/goldens_<persona>.json` | Goldens rendered from hand-authored seeds (output of `render_seeds.py`). |
| `data/generated/goldens_real.json` | Goldens exported from real labelled sessions in `meridian.db` (output of `build_dataset.py`). |

### Scripts

| File | Role |
|---|---|
| `render_seeds.py` | Reads `data/seeds/` → renders `data/generated/goldens_<persona>.json` in the deepeval Golden shape. Run after any seed edit. |
| `build_dataset.py` | Real-data exporter: queries `meridian.db` for already-classified sessions → writes `data/generated/goldens_real.json`. Two modes (SESSION_IDS / bulk). |

### Eval runners (standalone)

| File | Role |
|---|---|
| `eval_classifier.py` | Interactive eval runner — scores Goldens against the MLX server, emits OTel spans to OpenObserve, prints per-tier accuracy table. Use this for experimentation. |
| `eval_agent.py` | hermes-agent eval — runs real DB sessions through the hermes AIAgent, emits deepeval `@observe` traces to Confident AI. Standalone, not in CI. |

### Pytest suites

| File | Role |
|---|---|
| `test_classifier.py` | CI pytest suite — harness smoke tests (no model) + `test_mlx_e2e` + `test_mlx_per_golden` → Confident AI. Formal assertions; runs in CI. |
| `test_model_sweep.py` | Scaffold for Ollama model / prompt variant comparison. `CANDIDATE_MODELS` and `PROMPT_VARIANTS` are unpopulated — activate when running KAN-113/KAN-114. |

### Support

| File | Role |
|---|---|
| `metrics.py` | `TaskKeyMatchMetric`, `SessionTypeMatchMetric` (exact-match, no LLM), plus `AGENT_E2E_METRICS` (Ollama-judged `TaskCompletionMetric` for hermes-agent eval). |
| `strategies.py` | `EvalStrategy` base class + `DirectHttpStrategy` + `REGISTRY` + `from_env()`. Selected via `EVAL_STRATEGY` env var. |
| `conftest.py` | pytest path setup only. |

---

## Architecture convention — where strategies live

**Eval strategies live in `services/tests/evals/strategies.py`, NOT in `services/agents/`.**

- `services/agents/` = **production code** — tagger daemon, MLX server, in-process classifier. Ships in the production path.
- `services/tests/evals/strategies.py` = **eval-only strategies** — pluggable inference approaches for comparing models, sampling params, agentic decompositions, retrieval variants.
- A strategy that proves out in eval is **promoted** into `services/agents/` as a deliberate, separate step — never silently shared.

**Common mistakes to avoid:**
- ❌ Don't add a new strategy to `services/agents/` before it's proven in eval.
- ❌ Don't import from `services/agents/strategies*` — no such module exists.

---

## Run cookbook

### Eval run on the synthetic dataset (primary workflow)

```bash
# 1. Render seeds → Goldens (once per seed edit)
services/.venv/bin/python services/tests/evals/render_seeds.py a_meridian
# writes: services/tests/evals/data/generated/goldens_a_meridian.json

# 2. Run eval (server auto-discovered on port 7823)
EVAL_DATASET_PATH=services/tests/evals/data/generated/goldens_a_meridian.json \
services/.venv/bin/python services/tests/evals/eval_classifier.py

# Optional: validate a specific model is loaded
services/.venv/bin/python services/tests/evals/eval_classifier.py --model phi-4
```

Prints a per-tier accuracy table to stdout; pushes a full trace tree to OpenObserve under `service.name = meridian-eval`.

### Pytest mode (CI / formal gate)

```bash
# Smoke tests only — no model load, ~1 s
services/.venv/bin/pytest services/tests/evals/test_classifier.py \
  -m "integration and not slow"

# End-to-end eval → Confident AI
EVAL_DATASET_PATH=services/tests/evals/data/generated/goldens_a_meridian.json \
services/.venv/bin/pytest services/tests/evals/test_classifier.py \
  -k test_mlx_e2e -v

# Per-Golden parametrize
EVAL_DATASET_PATH=services/tests/evals/data/generated/goldens_a_meridian.json \
services/.venv/bin/pytest services/tests/evals/test_classifier.py \
  -k test_mlx_per_golden -v
```

### Confident AI dashboard (regression view, run history)

```bash
# One-time login
services/.venv/bin/deepeval login

CONFIDENT_API_KEY=$(grep '^CONFIDENT_API_KEY=' .env | cut -d= -f2-) \
EVAL_DATASET_PATH=services/tests/evals/data/generated/goldens_a_meridian.json \
services/.venv/bin/deepeval test run services/tests/evals/test_classifier.py \
  -k test_mlx_e2e \
  --override-ini "addopts=" \
  --identifier "qwen35-9b-optiq-4bit-dev_a-baseline" \
  --ignore-errors
```

**Identifier conventions:** `<model>-<dataset>-<purpose>` e.g. `qwen35-9b-optiq-4bit-dev_a-baseline`. Repeat to track the same config over time; vary when you change model, prompt, temperature, or dataset.

### Choosing between OpenObserve and Confident AI

| Question | Use |
|---|---|
| "Did all spans land? Was the root captured?" | OpenObserve — full trace tree with custom attributes |
| "What did the classifier reason for seed_id=26?" | OpenObserve — `actual_reasoning` event has full text |
| "Which Goldens flipped between yesterday and today?" | Confident AI — per-Golden regression diff |
| "How has hard-decoy accuracy trended this week?" | Confident AI — per-tier history charts |
| "Compare model-A vs model-B across all tiers" | Confident AI — hyperparameter A/B view |
| "I want everything local, no third-party cloud" | OpenObserve only |

Both can coexist as complementary views, but **not in the same process** — `eval_classifier.py` routes OTel spans to OpenObserve; `deepeval test run` hijacks the global TracerProvider and routes to Confident AI. Run each separately.

---

## Golden file schema

After `render_seeds.py`, `data/generated/goldens_<persona>.json` contains an array of:

```json
{
  "input": "<rendered prompt from build_user_message — RECENT WORK CONTEXT + SESSION + CANDIDATE TICKETS>",
  "expected_output": "{\"task_key\": \"KAN-139\" | \"none\", \"session_type\": \"task\"|\"overhead\"|\"untracked\", \"reasoning\": \"<ground truth reasoning>\"}",
  "additional_metadata": {
    "seed_id":    7,
    "app_name":   "DBeaver",
    "difficulty": "easy",
    "persona":    "a_meridian"
  }
}
```

The recent-context block is built from the last 5 *scoreable* prior seed sessions. Non-scoreable sessions (`scoreable=false`) exist for timeline density but never enter Goldens or the recent-context block.

---

## OpenObserve trace schema

`eval_classifier.py` emits two span types under `service.name = meridian-eval`:

| Span | Parent | Key attributes |
|---|---|---|
| `eval.run` | root | `run.id`, `persona`, `dataset_path`, `server_url`, `model_id`, `dataset_size`, `accuracy.task_key`, `accuracy.session_type`, `accuracy.both` |
| `eval.classify` | `eval.run` | `seed_id`, `difficulty`, `app_name`, `persona`, `expected.task_key`, `expected.session_type`, `actual.task_key`, `actual.session_type`, `classifier.confidence`, `key_ok`, `type_ok`, `both_ok`, `elapsed_s`, event `actual_reasoning` |

Per-Golden `force_flush` ensures spans land in OpenObserve as they complete — killing the run mid-flight keeps everything classified so far.

---

## When to re-run

- After any edit to `services/skills/activity/task-classifier/SKILL.md` (prompt change)
- After bumping `MLX_MODEL_ID` or restarting the MLX server with a different model
- After adding or editing Goldens in `data/seeds/sessions_<persona>.json`
- After any change to `services/agents/run_task_linker_mlx.py` (FSM schema, sampling, system prompt)
- Before merging any PR that touches the classifier or its prompt

The dataset is intentionally tilted toward failure modes that matter in production:

| Tier | Fails if… |
|---|---|
| `easy` | classifier fundamentally broken — regression-detector tripwire |
| `medium` | recent-context block isn't earning its weight |
| `hard` | model can't discriminate close ticket pairs |
| `hard-decoy` | model picks decoys when it shouldn't |
| `overhead` | classifier hallucinates tickets for non-work content |
| `untracked` | classifier hallucinates tickets when none fits |

A flat accuracy number hides regressions — always look at the per-tier breakdown.

---

## TODO / planned work

- **Full-pipeline eval mode** — currently benchmarks the classifier in isolation (pre-stored `session_text`). A second mode (`--mode=full-pipeline`) would pull raw frames from screenpipe DB, run `extract_block_context()` / ETL fresh, then score. Key prerequisite: the real-session extraction script (task #3) needs to snapshot raw screenpipe frame rows at label time.

---

## Limitations / known issues

- **`metrics.py` imports `OllamaModel` at module load** — `ollama` package must be installed even for exact-match-only runs.
- **Span loss without per-Golden flush** — `eval_classifier.py` calls `force_flush` after every Golden. New runners that skip this will lose spans on long runs.
- **deepeval `@observe` ≠ OTel `@observe`** — `test_classifier.py` targets Confident AI; `eval_classifier.py` targets OpenObserve. Mutually exclusive within one process.
- **OSS run-diff pending** — Confident AI has per-Golden run-over-run diff built in. OSS alternative (`compare_runs.py`) is not yet built.
- **No prompt versioning** — `SKILL.md` is loaded once at module import. A/B prompts requires restarting the MLX server.
- **Confident AI sends prompt content to a third party** — if seed `session_text` contains sensitive screen captures, redact before pushing, or stay OpenObserve-only.
