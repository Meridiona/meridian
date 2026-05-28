# `services/tests/evals/` — classifier eval harness

The experimentation harness for the MLX session-classifier. Scores predictions against a hand-authored golden dataset, emits OTel spans to OpenObserve so each run is inspectable as a trace tree, and serves as the gate that every model swap / prompt edit / parameter tweak must pass.

For the full failure-mode taxonomy and reasoning-about-evals primer, see [`TESTING.md` §9](../../../TESTING.md#9-classifier-eval-pipeline-deepeval--golden-dataset).

---

## File inventory

| File | Role |
|---|---|
| `golden_seed/dev_<persona>_sessions.json` | Hand-authored seed sessions. Persona today: `a_meridian` (35 sessions). Future: `b_generic`. Each session has structured fields + `ground_truth` + `design_notes` flagging which failure mode it targets. |
| `golden_seed/candidates_<project>.json` | Open ticket list the classifier picks from. `candidates_meridian.json` = 5 real KAN-* + 2 synthetic decoys (`KAN-142`, `KAN-145`). |
| `render_seeds.py` | Bridge — reads seed sessions, projects through `agents._prompts.build_user_message`, writes `.synthetic-dataset-<persona>.json` in the deepeval Golden shape. |
| `.dataset.json` | Goldens exported from real labelled sessions in `meridian.db` (legacy path via `build_dataset.py`). |
| `.synthetic-dataset-<persona>.json` | Goldens rendered from hand-authored seeds (output of `render_seeds.py`). Not committed — regenerate on demand. |
| `build_dataset.py` | Real-data exporter: queries `meridian.db` for already-classified sessions and writes `.dataset.json`. Two modes (SESSION_IDS / bulk). |
| `metrics.py` | `TaskKeyMatchMetric`, `SessionTypeMatchMetric` (exact-match, no LLM), plus `AGENT_E2E_METRICS` (uses an Ollama-judged `TaskCompletionMetric` — only relevant for hermes-agent eval, not the MLX classifier). |
| `conftest.py` | pytest path setup. |
| `test_mlx_classifier.py` | deepeval-driven pytest suite — runs `_run_mlx` over every Golden, asserts both metrics pass. Smoke tests + e2e + per-Golden parametrize. Honours `EVAL_DATASET_PATH`. |
| `test_stage3_classifier.py` | Half-built scaffold for hermes-agent eval. Placeholder for model/prompt sweep (`CANDIDATE_MODELS`, `PROMPT_VARIANTS` unfilled). |
| `eval_agent.py` | hermes-agent end-to-end eval (uses `AGENT_E2E_METRICS`). Standalone, not in CI. |
| `smoke_run.py` | One-shot runner that emits OTel spans to OpenObserve (one `eval.run` root + one `eval.classify` per Golden). Use this for experimentation; CI uses `test_mlx_classifier.py`. |

---

## Run cookbook

### Smoke run on the synthetic dataset

```bash
# 1. Render the latest seeds
services/.venv/bin/python services/tests/evals/render_seeds.py            # default a_meridian

# 2. Run against the live MLX server
EVAL_DATASET_PATH=services/tests/evals/.synthetic-dataset-a_meridian.json \
MLX_SERVER_URL=http://localhost:7823 \
services/.venv/bin/python services/tests/evals/smoke_run.py
```

Prints a per-tier accuracy table on stdout; pushes a full trace tree to OpenObserve under `service.name = meridian-eval`.

### Pytest mode (gate / CI)

```bash
# Smoke tests only — no model load, ~1 s
services/.venv/bin/pytest services/tests/evals/test_mlx_classifier.py \
  -m "integration and not slow"

# End-to-end (uses MLX server if MLX_SERVER_URL set, else in-process load)
MLX_SERVER_URL=http://localhost:7823 \
services/.venv/bin/pytest services/tests/evals/test_mlx_classifier.py \
  -k test_mlx_e2e -v

# Per-Golden parametrize — one pytest test_case per Golden
MLX_SERVER_URL=http://localhost:7823 \
services/.venv/bin/pytest services/tests/evals/test_mlx_classifier.py \
  -k test_mlx_per_golden -v
```

`EVAL_DATASET_PATH` defaults to `.dataset.json`. Set it explicitly to score the synthetic dataset.

### Confident AI dashboard (regression view, run history)

The deepeval-native cloud dashboard at [confident-ai.com](https://confident-ai.com) — best place to see per-Golden regression diffs between runs, per-tier accuracy trends, and hyperparameter A/B comparisons. Free tier covers small teams.

```bash
# One-time login (interactive — opens browser)
services/.venv/bin/deepeval login

# IMPORTANT: deepeval login writes the API key to .env.local by default.
# Move it to .env so the rest of the toolchain picks it up via python-dotenv.
# Final state should have CONFIDENT_API_KEY=<key> in .env (gitignored).

# Each run: tag with --identifier so you can find it in the dashboard.
# --override-ini "addopts=" is REQUIRED to bypass the project-wide default
#   `-m 'not integration and not slow'` filter in services/pyproject.toml,
#   which otherwise deselects every test in test_mlx_classifier.py.
CONFIDENT_API_KEY=$(grep '^CONFIDENT_API_KEY=' .env | cut -d= -f2-) \
EVAL_DATASET_PATH=services/tests/evals/.synthetic-dataset-a_meridian.json \
MLX_SERVER_URL=http://localhost:7823 \
services/.venv/bin/deepeval test run services/tests/evals/test_mlx_classifier.py \
  -k test_mlx_e2e \
  --override-ini "addopts=" \
  --identifier "phi4-4bit-dev_a-baseline" \
  --ignore-errors
```

Open the dashboard at `https://app.confident-ai.com` after the run completes. Each `--identifier` becomes a tagged run row.

**Identifier conventions** (matters for run-over-run diffing):
- `<model>-<dataset>-<purpose>` — e.g. `qwen3-7b-dev_a-baseline`, `phi4-4bit-dev_a-skill_v2_decoy_aware`
- Repeat the same identifier across runs to track the same configuration over time; vary identifier when you swap any of: model, prompt variant, temperature, dataset.
- Add date suffix (`-20260528`) when you want immutable snapshots vs. moving baselines.

### Choosing between OpenObserve and Confident AI

| Question you want answered | Use |
|---|---|
| "Did the run land all 26 spans? Was the root captured?" | OpenObserve — full trace tree with custom attributes |
| "What did the classifier reason for seed_id=26?" | OpenObserve — `actual_reasoning` event has full text |
| "Which Goldens flipped between yesterday's run and today's?" | Confident AI — per-Golden regression diff (the killer feature) |
| "How has hard-decoy accuracy trended this week?" | Confident AI — per-tier history charts |
| "Compare qwen3-7b vs phi-4-4bit across all tiers" | Confident AI — hyperparameter A/B view |
| "I want everything local, no third-party cloud" | OpenObserve only (build `compare_runs.py` for diffs) |

Both can coexist as complementary views, but **not in the same process** — `smoke_run.py` calls `observability.setup()` which routes OTel spans to OpenObserve; `deepeval test run` hijacks the global TracerProvider and routes to Confident AI. Run each separately when you want both views of the same eval state.

---

## Golden file schema

After `render_seeds.py`, `.synthetic-dataset-<persona>.json` contains an array of:

```json
{
  "input": "<rendered prompt string from build_user_message — RECENT WORK CONTEXT + SESSION + CANDIDATE TICKETS>",
  "expected_output": "{\"task_key\": \"KAN-139\" | \"none\", \"session_type\": \"task\"|\"overhead\"|\"untracked\", \"reasoning\": \"<ground truth reasoning>\"}",
  "additional_metadata": {
    "seed_id":    7,
    "app_name":   "DBeaver",
    "difficulty": "easy",
    "persona":    "a_meridian"
  }
}
```

The recent-context block is built from the last 5 *scoreable* prior seed sessions (`scoreable=true` in `ground_truth`). Sub-scoreable sessions (`scoreable=false`) exist in the seed file for timeline density but never enter the Goldens or the recent-context block.

---

## OpenObserve trace schema

`smoke_run.py` emits two span types under `service.name = meridian-eval`:

| Span | Parent | Key attributes |
|---|---|---|
| `eval.run` | root | `run.id`, `persona`, `dataset_path`, `server_url`, `dataset_size`, `accuracy.task_key`, `accuracy.session_type`, `accuracy.both` |
| `eval.classify` | `eval.run` | `seed_id`, `difficulty`, `app_name`, `persona`, `expected.task_key`, `expected.session_type`, `actual.task_key`, `actual.session_type`, `classifier.confidence`, `key_ok`, `type_ok`, `both_ok`, `elapsed_s`, event `actual_reasoning` |

Per-Golden `force_flush` in the runner ensures spans land in OpenObserve as they complete (so killing the run mid-flight keeps everything classified so far; you can also watch progress live in the OpenObserve Traces UI).

---

## When to re-run

- After any edit to `services/skills/activity/task-classifier/SKILL.md` (prompt change)
- After bumping `MLX_MODEL_ID` or restarting the MLX server with a different model
- After adding or editing Goldens in `golden_seed/dev_<persona>_sessions.json`
- After any change to `services/agents/run_task_linker_mlx.py` (FSM schema, sampling defaults, system_prompt composition)
- Before merging any PR that touches the classifier or its prompt — eyeball the per-tier table for regressions

The dataset is intentionally tilted toward failure modes that matter in production:

| Tier | Fails if… |
|---|---|
| `easy` | classifier fundamentally broken — regression-detector tripwire |
| `medium` | recent-context block isn't earning its weight |
| `hard` | model can't discriminate close ticket pairs |
| `hard-decoy` | model picks decoys when it shouldn't |
| `overhead` | classifier hallucinates tickets for non-work content (highest-volume prod failure) |
| `untracked` | classifier hallucinates tickets when none fits (the "confidently wrong" mode) |

A flat accuracy number lies about regressions — look at the per-tier breakdown, not just the headline.

---

## Limitations / known issues

- **`metrics.py` imports `OllamaModel` at module load**, so the `ollama` package must be installed even if you're only using the exact-match metrics. Acceptable for now; a lazy-init refactor would remove the dependency for classifier-only eval runs.
- **Span loss without per-Golden flush** — `smoke_run.py` calls `force_flush` after every Golden. If you write a new runner that doesn't, expect to lose spans (esp. the root `eval.run`) when the BatchSpanProcessor's 5-second shutdown drain can't keep up with a 10-minute run.
- **deepeval `@observe` ≠ OTel `@observe`** — the deepeval `@observe` in `test_mlx_classifier.py` targets Confident AI cloud; OpenObserve spans need OTel SDK setup via `observability.setup()` (see `smoke_run.py`). The two trace destinations are mutually exclusive within one process.
- **OSS run-diff still pending** — Confident AI cloud has per-Golden run-over-run diff built in. OSS alternative is the still-pending `compare_runs.py` that would diff two `.synthetic-dataset-*.json` snapshots by `seed_id` (useful for an air-gapped workflow or to back up the Confident AI view).
- **No prompt versioning** — `SKILL.md` is loaded once at module import. To A/B prompts, restart the MLX server with a different system prompt or wait for the Layer 1 refactor that makes `system_prompt` a per-call override.
- **Confident AI sends prompt content to a third party** — if your seed `session_text` contains sensitive screen captures, redact before pushing, or stay OpenObserve-only.
