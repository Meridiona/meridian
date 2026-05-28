# task-classifier — Prompt Feedback (for next SKILL.md version)

This file collects empirical failure-mode evidence from eval runs against the
hand-authored golden dataset (see `services/tests/evals/`). Do **not** treat it
as a changelog — treat it as a backlog of prompt-edit candidates to fold into
the next SKILL.md revision.

When you cut a new version, work through the items below, ship the prompt
change in one commit, re-run `smoke_run.py` against the same dataset, and
record the before/after pass count under "## Resolved" so future maintainers
can see which prompt rule fixed which failure mode.

---

## Source run

- **Trace ID:** `945c2f502830e4dc73a9e4a9ec417496` (OpenObserve, `service.name=meridian-eval`)
- **Run identifier (Confident AI):** `phi4-4bit-dev_a-baseline`
- **Model:** `mlx-community/phi-4-4bit` via MLX server on port 7823
- **Prompt:** SKILL.md v2.0.0
- **Dataset:** `dev_a_sessions.json` → `.synthetic-dataset-a_meridian.json` (26 scoreable Goldens)
- **Date:** 2026-05-28
- **Headline:** 21/26 fully passed · TaskKey 80.8% · SessionType 84.6%

---

## Pending — prompt edits to make in the next SKILL.md revision

### 1. Talking about a ticket ≠ working on a ticket

**Affected Goldens:** seed_id=1, seed_id=10 (both `overhead` tier, both Slack)

**Failure pattern:** Slack DMs that *mention* a ticket key in conversation
("when you pick up KAN-138 lets resolve…") cause the classifier to claim the
user is actively working on that ticket. Both cases triggered with confidence
0.85.

**Proposed rule:**
> Mentions of a ticket key in chat, Slack, email, or comments are *conversation*,
> not work. To classify a session as `task` for a ticket, the user must be
> actively editing files, running commands, debugging issues, or otherwise
> *executing* on the ticket — not just discussing it.

**Discriminator question for the prompt:** *"Am I seeing the user **do** the
work, or **discuss** the work?"*

---

### 2. Reading about a topic ≠ working on that topic

**Affected Goldens:** seed_id=26 (`hard-decoy` tier, Chrome)

**Failure pattern:** User browsing an article titled "Designing golden
datasets for LLM classifiers" while KAN-139 is *about* building a golden
dataset. Classifier mapped topic → ticket and claimed `KAN-139 / task` at
0.85 confidence. Correct answer: `none / untracked`.

**Proposed rule:**
> Reading articles, browsing documentation, or watching videos about a topic
> is research/learning, **not** active work on a matching ticket — even if the
> topic and the ticket are an exact match. Classify these as `untracked` /
> `overhead` unless the user is also actively editing or executing.

---

### 3. Editing meta-files (CLAUDE.md, configs, docs) ≠ working on the ticket those files describe

**Affected Goldens:** seed_id=33 (`untracked` tier, VS Code)

**Failure pattern:** User fixed a typo in CLAUDE.md (refrence → reference).
The classifier reasoned: "CLAUDE.md describes the classifier, KAN-139 is
about the classifier, therefore this IS KAN-139 work." Confidence: 0.90 —
the most confidently wrong case in the run.

**Proposed rule:**
> Editing documentation, configuration, CLAUDE.md, or eval files is
> *overhead* — not active work on whatever ticket those files describe.
> The exception is when the active ticket explicitly says "update docs"
> or "edit config" in its description.

---

### 4. Branch name should outweigh file-path inference when they disagree

**Affected Goldens:** seed_id=20 (`easy` tier, VS Code) — the one `easy` failure

**Failure pattern:** User on branch `merge-add-obs-with-mlx-persistent-server`
(clearly KAN-138 work — the branch name says so). Classifier ignored the
branch name, looked at modified files (`services/agents/observability.py`,
`services/agents/server.py`), and picked KAN-136 (observability ticket)
instead. Both metrics partial-failed (`type_ok=true, key_ok=false`).

**Proposed rule:**
> When the git branch name encodes a ticket key, or strongly suggests one
> ticket while modified file paths suggest another, the branch name takes
> priority. The user committed to that branch deliberately; file overlap
> across tickets is incidental.

**Note:** This one is more subtle than rules 1–3. The risk of over-weighting
branch name is that long-lived branches accumulate work for multiple tickets.
Consider phrasing as "prefer branch name when present and recent" rather than
a hard override.

---

### 5. Optimism bias

**Cross-cutting observation:** All 5 failures in the source run over-classified
(real label = `none/overhead/untracked`, classifier label = `task` with a
ticket attached). Zero false-negatives. The classifier wants to find a ticket
and given any thread to pull on, will pull. High confidence on every wrong
answer (0.85–0.90).

**Proposed prompt addition:**
> Default to `untracked` or `overhead` when the evidence is *adjacent* rather
> than *direct*. Adjacent evidence (mentions, related reading, related infra)
> is the most common failure mode for this classifier in production — bias
> toward "I don't know" rather than confidently picking a ticket that the
> user is merely *near*.

This may be more impactful than rules 1–4 individually, since it addresses
the underlying bias all four exploit.

---

## How to validate a new SKILL.md version

1. Edit `SKILL.md`, bump the `version:` field.
2. Restart the MLX server so the new prompt is loaded:
   `launchctl kickstart -k gui/$(id -u)/com.meridiona.mlx-server`
3. Re-render the dataset (in case seed files changed too):
   `services/.venv/bin/python services/tests/evals/render_seeds.py`
4. Run the eval:
   ```
   EVAL_DATASET_PATH=services/tests/evals/.synthetic-dataset-a_meridian.json \
   MLX_SERVER_URL=http://localhost:7823 \
   services/.venv/bin/python services/tests/evals/smoke_run.py
   ```
5. In OpenObserve, compare the new trace's per-tier accuracy against
   trace_id `945c2f502830e4dc73a9e4a9ec417496` (the baseline).
6. Move resolved items below; leave anything still failing in the Pending list.

---

## Resolved

*(empty — populate as prompt edits land)*
