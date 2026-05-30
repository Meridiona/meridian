---
name: eval-feedback
description: "After a classifier eval run, read the local results JSON, append failures to FEEDBACK.json as structured observations, cluster them into failure_classes, and summarize what to fix in the next SKILL.md / strategy / config revision. Invoke when the user says 'analyze the latest eval', 'what failed in this run', 'update the feedback file', or similar."
allowed-tools:
  - Bash
  - Read
  - Edit
  - Write
---

# eval-feedback Skill

You are updating `services/skills/activity/task-classifier/FEEDBACK.json` based on the outcome of a classifier eval run. This file is the structured backlog the team uses when revising the classifier prompt (`services/skills/activity/task-classifier/SKILL.md`), strategies (`services/tests/evals/strategies.py`), or experiment configs (`services/tests/evals/configs/`) — every failure you log here becomes evidence for or against a specific change.

## Data sources — local-first

Every `eval_classifier.py` invocation writes **two** copies of the run data:

1. **`services/tests/evals/results/run_<run_id>.json`** — **local canonical results.** Same data as the OTel trace, immediately readable, no auth, no round-trip. **This is your primary input.**
2. **OpenObserve trace** under `service=meridian-eval` — full waterfall view for human exploration. Fallback if the local file is missing for some reason.

**Always read the local file first.** Only fall back to OpenObserve when the file is absent (rare — e.g. user ran an old build, or rm'd the results dir).

## When to run

The user invokes this skill on demand after running an eval — typically via `services/tests/evals/eval_classifier.py` against the MLX server on port 7823. They will ask in plain language ("walk me through the latest run", "what failed", "update feedback for this trace"). You do not run automatically.

## Inputs

Ask the user only if these aren't obvious from context:

| Input | Default if unspecified | Where to find |
|---|---|---|
| `run_id` (or results file path) | The newest file in `services/tests/evals/results/` | `ls -t services/tests/evals/results/run_*.json \| head -1` |

The results file contains everything: `run_id`, `trace_id`, `experiment.name`, all metrics, all per-seed results with full reasoning. **You don't need to ask the user for model, prompt_version, dataset, etc. — those are in the file.**

## Step-by-step procedure

### 1. Find the run

If the user says "latest run":

```bash
ls -t services/tests/evals/results/run_*.json | head -1
```

If the user names a config or persona, narrow with a glob:

```bash
ls -t services/tests/evals/results/run_b_generic_*.json | head -1     # latest Dev B
ls -t services/tests/evals/results/run_*_direct_http_*.json | head -1  # latest direct_http strategy
```

If the user gives a `run_id` directly, the path is `services/tests/evals/results/run_<run_id>.json`.

If the local file genuinely doesn't exist, **only then** fall back to OpenObserve (see § OpenObserve fallback at end).

### 2. Pull failures + run metadata from the results JSON

`jq` slices give you everything without opening the whole file in your context:

```bash
RESULTS="<path from step 1>"

# Run-level metadata + headline
jq '{run_id, experiment, config: (.config | {strategy, model_id, dataset_name, persona, hyperparameters, recorded_from_config}), metrics: (.metrics | {total_goldens, passed_both, task_key_accuracy, session_type_accuracy, both_accuracy, per_tier})}' "$RESULTS"

# All failing seeds (this is the working set for the rest of the skill)
jq '[.per_seed_results[] | select(.both_ok == false)]' "$RESULTS"

# Trace ID (for cross-referencing OpenObserve if a human wants to dig further)
jq -r '.trace_id' "$RESULTS"
```

Each failing entry has `seed_id`, `difficulty`, `app_name`, `expected.{task_key,session_type}`, `actual.{task_key,session_type,confidence,reasoning}`, `key_ok`, `type_ok`, `elapsed_s`, `error`. **The full `actual.reasoning` text is the primary signal for which failure_class each observation belongs to.**

### 3. Read current `FEEDBACK.json` — lean path

FEEDBACK.json grows ~20 KB per run (observations are 80%+ of the bulk). To avoid blowing up the context window after ~25 runs, **do not Read the whole file** for the dedup-and-cluster steps. The skill only needs three slices to make append decisions:

```bash
FB=services/skills/activity/task-classifier/FEEDBACK.json

# Existing run_ids (dedup check — refuse if our run is already logged)
jq -r '.runs[].run_id' "$FB"

# Existing failure_class IDs + titles (for the clustering decision in step 4)
jq -r '.failure_classes[] | "\(.id) — \(.title) [status=\(.status), occurrences=\(.occurrence_count)]"' "$FB"

# Next observation sequence number (for the new obs IDs in step 5b)
jq -r '.observations[].id' "$FB" | grep "^obs-$(date +%Y%m%d)-" | sort | tail -1
# (if none for today, start at 001; otherwise increment the trailing NNN)

# Schema declaration (the contract for which fields go on a failure_class)
jq '._meta.failure_class_schema' "$FB"
```

This pulls a few KB regardless of file size. Use full `Read` ONLY when:
- The user explicitly asks to inspect an old observation (`"show me obs-20260528-007"`)
- You need to re-cluster a previously-recorded observation (rare, only when refactoring the taxonomy)
- The file is genuinely small (< 50 KB) and reading it whole is simpler

**Refuse to double-append:** if the run's `run_id` is already in `jq -r '.runs[].run_id'`, stop and tell the user the run is already logged.

### 4. Cluster each failure into a `failure_class`

For each failing seed, read its `actual.reasoning` (already in the slice from step 2) and assign it to an existing failure_class **only when the failure pattern genuinely matches** the class description, not just keywords. The existing classes as of baseline:

| failure_class_id | When to assign |
|---|---|
| `chat-mention-as-work` | Failure is in a chat/comms app (Slack, Mail, iMessage, etc.) and reasoning cites a ticket key that appeared in conversation text |
| `reading-as-doing` | Failure is in a browser/PDF reader and reasoning maps article topic to a ticket's domain |
| `meta-file-edit-as-ticket-work` | Failure is editing classifier configs, CLAUDE.md, SKILL.md, eval files, or similar meta-artifacts |
| `filepath-outweighs-branch` | Failure shows the classifier picked a different real ticket than the branch name suggested |
| `optimism-bias` | Cross-cutting — assign as a *secondary* class to ANY failure where the wrong answer was `task` with a real ticket at confidence ≥0.8 (most failures will qualify) |

**If no existing class fits**, propose a new one. New class IDs should be kebab-case and describe the failure mechanism, not the symptom: `chat-mention-as-work` (good), `slack-failures` (bad — describes where, not why).

Be conservative: better to create one new class for a genuinely new pattern than to force-fit into an existing class. The whole point of the structured log is that we can see which patterns persist across runs.

### 5. Append to `FEEDBACK.json`

**Prefer a Python script** that does load → modify → write, rather than `Edit` or `Read + Write` on the whole file. Python keeps the JSON out of the model context entirely — it scales to any file size, and it's safer because it always rewrites valid JSON. Skeleton:

```python
import json
from pathlib import Path

fb_path = Path("services/skills/activity/task-classifier/FEEDBACK.json")
results_path = Path("services/tests/evals/results/run_<run_id>.json")  # from step 1

fb = json.loads(fb_path.read_text())
results = json.loads(results_path.read_text())

# Guard against double-append
if any(r["run_id"] == results["run_id"] for r in fb["runs"]):
    raise SystemExit(f"run_id {results['run_id']} already logged")

fb["runs"].append({...})              # see (a) below
fb["observations"].extend([...])      # see (b) below
# Mutate fb["failure_classes"] in place; see (c) and (d) below

fb_path.write_text(json.dumps(fb, indent=2, ensure_ascii=False) + "\n")
```

Use `Edit` directly on FEEDBACK.json only when the file is small (< 50 KB) and the change is tiny (e.g., marking one class `resolved_in:<version>`). For appending a full run worth of data, always go through Python.

Append:

**(a) One entry to `runs`:** every field below maps directly to a field in the results JSON — copy-through, don't re-derive.

```json
{
  "run_id":            "<results.run_id>",
  "trace_id":          "<results.trace_id>",
  "results_file":      "services/tests/evals/results/run_<run_id>.json",
  "date":              "<results.timestamp YYYY-MM-DD>",
  "experiment_name":   "<results.experiment.name, or null if env-var run>",
  "experiment_config": "<results.experiment.config_file, or null>",
  "strategy":          "<results.config.strategy>",
  "model":             "<results.config.model_id — actual server model>",
  "model_declared":    "<results.config.hyperparameters.model — what the experiment intended>",
  "prompt_version":    "<results.config.recorded_from_config.prompt_version, or 'unknown'>",
  "prompt_path":       "services/skills/activity/task-classifier/SKILL.md",
  "dataset":           "<results.config.dataset_path>",
  "dataset_name":      "<results.config.dataset_name>",
  "persona":           "<results.config.persona>",
  "server_url":        "<results.config.server_url>",
  "session_text_cap":  <results.config.session_text_cap or results.config.recorded_from_config.session_text_cap>,
  "temperature":       <results.config.recorded_from_config.temperature or null>,
  "max_tokens":        <results.config.recorded_from_config.max_tokens or null>,
  "metrics": {
    "total_goldens":         <results.metrics.total_goldens>,
    "passed_both":           <results.metrics.passed_both>,
    "task_key_accuracy":     <results.metrics.task_key_accuracy>,
    "session_type_accuracy": <results.metrics.session_type_accuracy>,
    "both_accuracy":         <results.metrics.both_accuracy>,
    "per_tier":              <results.metrics.per_tier>,
    "latency":               <results.metrics.latency>
  },
  "notes": "<1-2 sentences on what's different from prior runs. If experiment.description is present, use or paraphrase it; otherwise reason from the diff vs the prior same-persona run.>"
}
```

**(b) One observation per failing seed:**

```json
{
  "id":               "obs-<YYYYMMDD>-<NNN>",   // NNN = next sequence; check max in file first
  "run_id":           "<run_id>",
  "seed_id":          <seed_id>,
  "tier":             "<difficulty>",
  "app_name":         "<from per_seed_results[].app_name>",
  "expected":         { "task_key": "...", "session_type": "..." },
  "actual":           { "task_key": "...", "session_type": "...", "confidence": 0.xx },
  "key_ok":           false|true,
  "type_ok":          false|true,
  "classifier_reasoning": "<full text from per_seed_results[].actual.reasoning>",
  "failure_class_id": "<one ID from step 4>"
}
```

**(c) Update each affected `failure_class`:**
- Add `<run_id>` to `observed_in_runs` (avoid duplicates)
- Add `seed_id` to `observed_in_seeds`
- Increment `occurrence_count` by the number of new observations in this run
- If a class's status is `resolved_in:<version>` and you're seeing it AGAIN in a newer prompt version, flip status back to `open` and add a note: `"reopened_in": "<run_id>"` — this is a regression signal worth surfacing prominently to the user.

**(d) If you created a new failure_class**, append it with the required structure and any applicable optional fields. See `_meta.failure_class_schema` in FEEDBACK.json for the authoritative field list.

Required (always present):
```json
{
  "id":                   "<kebab-case>",
  "title":                "<one-line plain-English title>",
  "description":          "<2-3 sentences describing what the classifier does wrong AND the triggering signal>",
  "observed_in_runs":     ["<run_id>"],
  "observed_in_seeds":    [N],
  "occurrence_count":     1,
  "proposed_prompt_rule": "<a concrete sentence-or-paragraph the user could literally copy-paste into SKILL.md>",
  "status":               "open",
  "resolved_in":          null
}
```

Optional (add only when applicable — do not stub with null):
- `highest_confidence` (number) — max confidence on any failure in this class
- `priority` ("highest"|"high"|"medium"|"lower"|"lowest") — triage rank, default "medium" when absent
- `rationale` (string) — one-sentence justification of priority or cross-class relationships
- `direction` ("false-positive"|"false-negative") — default "false-positive" when absent. Set explicitly on false-negative classes since they invert the prompt-rule logic.
- `note` (string) — caveat or cross-class-tension comment for the SKILL.md maintainer

When updating an EXISTING failure_class that already has some optional fields (e.g., `highest_confidence`), recompute the value across the new observations and update in place. Don't strip optional fields from existing classes.

### 6. Validate

After writing:
```bash
python3 -c "import json; json.load(open('services/skills/activity/task-classifier/FEEDBACK.json')); print('valid')"
```

If invalid, fix and re-validate before continuing.

### 7. Summarize for the user

Report back in this shape — concise, focused on what's actionable. When `experiment.name` is present, lead with it (signals "this was a deliberate experiment, not an ad-hoc run").

```
Run logged: <run_id>
  Experiment: <experiment.name>  (or "ad-hoc env-var run" if name is null)
  Strategy:   <strategy>  ·  Model: <model_id>  ·  Dataset: <dataset_name>
  Headline:   <X>/<N> passed · task_key <X.X%> · session_type <X.X%> · both <X.X%>

Failures this run (<N>):
  seed=<id> tier=<x> app=<app> → failure_class:<id>  [+new|+existing]
  seed=<id> tier=<x> app=<app> → failure_class:<id>
  ...

Open failure_classes ranked by total occurrences across all runs:
  <id> — <count> occurrences across <M> runs — <one-line proposed_prompt_rule>
  ...

[If anything regressed]
Regressions:
  failure_class:<id> was resolved_in:<version> but reappeared in this run (seeds <N, N>)

Suggested next move:
  <one sentence — choose ONE>:
  - "edit SKILL.md to add rule X" (when a failure_class has hit ≥3 occurrences and proposed_prompt_rule is concrete)
  - "the top class is unchanged across N runs — architectural fix needed (e.g. ExtractThenClassifyStrategy)"
  - "regression in class Z, investigate prompt v2.1 changes"
  - "<persona> dataset under-covers tier <x>; add more Goldens there"
```

## Guardrails

- **Don't edit `SKILL.md`** from this skill. The skill only updates FEEDBACK.json. Prompt edits are a separate, deliberate revision step the user drives.
- **Don't edit strategies or configs.** Same reason. Strategy code changes are deliberate work; experiment configs are user-curated artifacts.
- **Don't invent observations.** Only log seeds that actually appeared in the results JSON. If a field is missing, note it in the summary rather than fabricating.
- **Don't merge across runs.** Each run gets its own `runs` entry and its own observations. Resist the temptation to "consolidate" old runs.
- **If the run returned zero failures**, still append the `runs` entry with the 100% pass-rate metrics, no observations needed, and tell the user — a clean run is data too.
- **Confidence in clustering should be honest.** If a failure is genuinely ambiguous between two failure_classes, note it in the observation as `"failure_class_candidates": ["a", "b"]` instead of `failure_class_id`, and surface the ambiguity in the summary so the user can resolve it.
- **`model` vs `model_declared` mismatch is a finding, not a bug to silently absorb.** If the results JSON shows `config.model_id` differs from `config.hyperparameters.model` by more than a namespace prefix (e.g. `mlx-community/Qwen3.5-9B-OptiQ-4bit` vs `Qwen3.5-9B-OptiQ-4bit` is fine, but `phi-4` vs `Qwen` is not), flag it in the summary — the experiment's declared model didn't match what actually served.

## OpenObserve fallback (rare)

If `services/tests/evals/results/run_*.json` doesn't exist for the run the user wants (e.g. an older run logged before the local-results writer existed, or someone rm'd the directory):

```bash
AUTH=$(grep MERIDIAN_OO_AUTH .env | cut -d= -f2-)
NOW_US=$(date -u +%s)000000
WEEK_AGO_US=$(( NOW_US - 7*86400*1000000 ))

# Find recent eval.run traces
curl -s -X POST "http://localhost:5080/api/default/_search?type=traces" \
  -H "Authorization: Basic $AUTH" -H "Content-Type: application/json" \
  -d "{\"query\":{\"sql\":\"SELECT trace_id, run_id, persona, strategy, accuracy_both FROM \\\"default\\\" WHERE service_name='meridian-eval' AND operation_name='eval.run' ORDER BY _timestamp DESC\",\"start_time\":$WEEK_AGO_US,\"end_time\":$NOW_US,\"from\":0,\"size\":5}}"

# For a given trace_id, pull failing classify spans
TRACE_ID="<paste>"
curl -s -X POST "http://localhost:5080/api/default/_search?type=traces" \
  -H "Authorization: Basic $AUTH" -H "Content-Type: application/json" \
  -d "{\"query\":{\"sql\":\"SELECT seed_id, difficulty, app_name, expected_task_key, expected_session_type, actual_task_key, actual_session_type, classifier_confidence, key_ok, type_ok, events FROM \\\"default\\\" WHERE service_name='meridian-eval' AND operation_name='eval.classify' AND trace_id='$TRACE_ID' AND both_ok='false' ORDER BY seed_id\",\"start_time\":$WEEK_AGO_US,\"end_time\":$NOW_US,\"from\":0,\"size\":100}}"
```

The `events` column is a JSON array — each event has the per-Golden reasoning under `classifier_response` and `actual_reasoning`. OpenObserve carries everything the local results file does (it was the original source-of-truth before the local writer landed), just less ergonomically.

## Reference

- Schema and existing entries: `services/skills/activity/task-classifier/FEEDBACK.json`
- The classifier prompt being evaluated: `services/skills/activity/task-classifier/SKILL.md`
- The runner that writes the results JSON: `services/tests/evals/eval_classifier.py`
- Results schema documentation: `services/tests/evals/README.md` § Results schema
- Strategy abstraction: `services/tests/evals/strategies.py`
- Experiment configs: `services/tests/evals/configs/`
- OpenObserve env vars (fallback only): `.env` keys `MERIDIAN_OO_AUTH`, `MERIDIAN_OTLP_ENDPOINT`
