---
name: eval-feedback
description: "After a classifier eval run, pull failed cases from OpenObserve, append them to FEEDBACK.json as structured observations, cluster them into failure_classes, and summarize what to fix in the next SKILL.md revision. Invoke when the user says 'analyze the latest eval', 'what failed in this run', 'update the feedback file', or similar."
allowed-tools: Bash, Read, Edit, Write
---

# eval-feedback Skill

You are updating `services/skills/activity/task-classifier/FEEDBACK.json` based on the outcome of a classifier eval run. This file is the structured backlog the team uses when revising the classifier prompt (`services/skills/activity/task-classifier/SKILL.md`) — every failure you log here becomes evidence for or against a specific prompt rule.

## When to run

The user invokes this skill on demand after running an eval — typically via `services/tests/evals/smoke_run.py` against the MLX server on port 7823. They will ask in plain language ("walk me through the latest run", "what failed", "update feedback for this trace", etc.). You do not run automatically.

## Inputs

Ask the user only if these aren't obvious from context:

| Input | Default if unspecified | Where to find |
|---|---|---|
| `trace_id` | The most recent `eval.run` trace in OpenObserve | Query OpenObserve as in step 1 below |
| `run_id` (human label) | Derive from `trace_id` + date: `<model>-<persona>-<purpose>-<YYYYMMDD>` | Ask user if model/purpose is unclear |
| `model`, `prompt_version`, `dataset` | Read from `services/agents/run_task_linker_mlx.py` (model id), `SKILL.md` frontmatter (version), and the env var/path used by the eval | Default to current values unless user specifies otherwise |

If the user says "latest run", use the OpenObserve query in step 1 to find the most recent `eval.run` span. If they give a `trace_id` directly, skip to step 2.

## Step-by-step procedure

### 1. Find the run (only if user said "latest")

```bash
AUTH=$(grep MERIDIAN_OO_AUTH .env | cut -d= -f2-)
NOW_US=$(date -u +%s)000000
WEEK_AGO_US=$(( NOW_US - 7*86400*1000000 ))

curl -s -X POST "http://localhost:5080/api/default/_search?type=traces" \
  -H "Authorization: Basic $AUTH" \
  -H "Content-Type: application/json" \
  -d "{
    \"query\": {
      \"sql\": \"SELECT trace_id, _timestamp, persona, dataset_size, accuracy_both FROM \\\"default\\\" WHERE service_name='meridian-eval' AND operation_name='eval.run' ORDER BY _timestamp DESC\",
      \"start_time\": $WEEK_AGO_US,
      \"end_time\": $NOW_US,
      \"from\": 0,
      \"size\": 5
    }
  }"
```

Pick the newest `trace_id`. If the user wants to inspect an older run, show them the list and ask.

### 2. Pull every failing classify span for that trace

```bash
AUTH=$(grep MERIDIAN_OO_AUTH .env | cut -d= -f2-)
NOW_US=$(date -u +%s)000000
WEEK_AGO_US=$(( NOW_US - 7*86400*1000000 ))
TRACE_ID="<paste trace_id here>"

curl -s -X POST "http://localhost:5080/api/default/_search?type=traces" \
  -H "Authorization: Basic $AUTH" \
  -H "Content-Type: application/json" \
  -d "{
    \"query\": {
      \"sql\": \"SELECT seed_id, difficulty, app_name, expected_task_key, expected_session_type, actual_task_key, actual_session_type, classifier_confidence, key_ok, type_ok, events FROM \\\"default\\\" WHERE service_name='meridian-eval' AND operation_name='eval.classify' AND trace_id='$TRACE_ID' AND both_ok='false' ORDER BY seed_id\",
      \"start_time\": $WEEK_AGO_US,
      \"end_time\": $NOW_US,
      \"from\": 0,
      \"size\": 100
    }
  }"
```

Also pull the parent `eval.run` span (same query, `operation_name='eval.run' AND trace_id=$TRACE_ID`) to get `dataset_size`, `accuracy.task_key`, `accuracy.session_type`, `accuracy.both`.

The `events` column on each failing span is a JSON string with the classifier's full `actual_reasoning` — that text is your primary signal for which failure_class the observation belongs to.

### 3. Read current `FEEDBACK.json`

Read `services/skills/activity/task-classifier/FEEDBACK.json`. Understand:
- Which `run_id`s already exist (don't double-append; if the trace is already logged, tell the user and stop).
- Which `failure_classes` already exist (you'll want to reuse their IDs when failures match).

### 4. Cluster each failure into a `failure_class`

For each failing span, read its `classifier_reasoning` (from the events array) and assign it to an existing failure_class **only when the failure pattern genuinely matches** the class description, not just keywords. The existing classes as of baseline:

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

Use Edit (or Read + Write) to update the file. Append:

**(a) One entry to `runs`:**
```json
{
  "run_id": "<derived>",
  "trace_id": "<from OpenObserve>",
  "confident_ai_run": "<if available, otherwise null>",
  "date": "<YYYY-MM-DD>",
  "model": "<model id>",
  "prompt_version": "<SKILL.md version>",
  "prompt_path": "services/skills/activity/task-classifier/SKILL.md",
  "dataset": "<dataset path used>",
  "dataset_rendered": "<.synthetic-dataset-* path>",
  "candidates": "<candidates file used>",
  "persona": "<persona>",
  "server_url": "<MLX server URL>",
  "metrics": {
    "total_goldens": N,
    "passed_both": N,
    "task_key_accuracy": 0.xxx,
    "session_type_accuracy": 0.xxx,
    "both_accuracy": 0.xxx
  },
  "notes": "<1-2 sentences on what's different from prior runs — model swap? prompt change? new dataset? Or 'first run after <change>'.>"
}
```

**(b) One observation per failing seed:**
```json
{
  "id": "obs-<YYYYMMDD>-<NNN>",          // NNN = next sequence; check max in file first
  "run_id": "<run_id>",
  "seed_id": N,
  "tier": "<difficulty>",
  "app_name": "<from span>",
  "expected": { "task_key": "...", "session_type": "..." },
  "actual": { "task_key": "...", "session_type": "...", "confidence": 0.xx },
  "key_ok": false|true,
  "type_ok": false|true,
  "classifier_reasoning": "<full text from events.actual_reasoning>",
  "failure_class_id": "<one ID from step 4>"
}
```

**(c) Update each affected `failure_class`:**
- Add `<run_id>` to `observed_in_runs` (avoid duplicates)
- Add `seed_id` to `observed_in_seeds`
- Increment `occurrence_count` by the number of new observations in this run
- If a class's status is `resolved_in:<version>` and you're seeing it AGAIN in a newer prompt version, flip status back to `open` and add a note: `"reopened_in": "<run_id>"` — this is a regression signal worth surfacing prominently to the user.

**(d) If you created a new failure_class**, append it with the full structure:
```json
{
  "id": "<kebab-case>",
  "title": "<one-line plain-English title>",
  "description": "<2-3 sentences describing what the classifier does wrong AND the triggering signal>",
  "observed_in_runs": ["<run_id>"],
  "observed_in_seeds": [N],
  "occurrence_count": 1,
  "proposed_prompt_rule": "<a concrete sentence-or-paragraph the user could literally copy-paste into SKILL.md>",
  "status": "open",
  "resolved_in": null
}
```

### 6. Validate

After writing, run:
```bash
python3 -c "import json; json.load(open('services/skills/activity/task-classifier/FEEDBACK.json')); print('valid')"
```

If invalid, fix and re-validate before continuing.

### 7. Summarize for the user

Report back in this shape — concise, focused on what's actionable:

```
Run logged: <run_id> (trace <trace_id>)
Headline: <X>/<N> passed · task_key <X.X%> · session_type <X.X%>

Failures this run (<N>):
  seed=<id> tier=<x> → assigned to failure_class:<id>  [+new|+existing]
  seed=<id> tier=<x> → assigned to failure_class:<id>
  ...

Open failure_classes ranked by total occurrences across all runs:
  <id> — <count> occurrences across <M> runs — <one-line proposed_prompt_rule>
  ...

[If anything regressed]
Regressions:
  failure_class:<id> was resolved_in:v2.1 but reappeared in this run (seeds <N, N>)

Suggested next move:
  <one sentence — usually "edit SKILL.md to add rule X" or "the top class is unchanged, add more Goldens to tier Y", or "regression in class Z, investigate prompt v2.1 changes">
```

## Guardrails

- **Don't edit `SKILL.md`** from this skill. The skill only updates FEEDBACK.json. Prompt edits are a separate, deliberate revision step the user drives.
- **Don't invent observations.** Only log spans that actually appeared in the OpenObserve query result. If a span is missing fields, note it in the summary rather than fabricating.
- **Don't merge across runs.** Each run gets its own `runs` entry and its own observations. Resist the temptation to "consolidate" old runs.
- **If the trace returns zero failures**, still append the `runs` entry with the 100% pass-rate metrics, no observations needed, and tell the user — a clean run is data too.
- **Confidence in clustering should be honest.** If a failure is genuinely ambiguous between two failure_classes, note it in the observation as `"failure_class_candidates": ["a", "b"]` instead of `failure_class_id`, and surface the ambiguity in the summary so the user can resolve it.

## Reference

- Schema and existing entries: `services/skills/activity/task-classifier/FEEDBACK.json`
- The classifier prompt being evaluated: `services/skills/activity/task-classifier/SKILL.md`
- The runner that emits the OTel spans you query: `services/tests/evals/smoke_run.py`
- OpenObserve env vars (auth, endpoint): `.env` keys `MERIDIAN_OO_AUTH`, `MERIDIAN_OTLP_ENDPOINT`
