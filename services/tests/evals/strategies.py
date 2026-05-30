"""Eval strategy abstraction — pluggable classification approaches for the eval harness.

A Strategy generates `actual_output` for a pre-rendered Golden prompt. Deepeval
then scores that output; the strategy doesn't touch the scoring layer.

These strategies are EVAL-ONLY. Production code (services/agents/) has its own
classifier path via run_task_linker_mlx.py. Strategies here exist so the eval
harness can swap inference approaches — models, sampling params, agentic
decomposition, retrieval-augmented variants, etc. — without touching production
code. When a strategy proves out in eval and is worth shipping, the logic is
promoted into services/agents/ as a deliberate productionization step.

Integration points:
  eval_classifier.py      (OpenObserve path)  — strategy.classify_prompt() drives
                                                the eval loop; strategy name/config
                                                emitted as span attributes on eval.run.
  test_classifier.py      (Confident AI path) — strategy.classify_prompt() replaces
                                                the inline HTTP block in _run_mlx;
                                                strategy.as_hyperparameters() is
                                                passed to evaluate() so Confident AI
                                                can compare runs across strategies.

Adding a new strategy:
  1. Subclass EvalStrategy below.
  2. Implement classify_prompt(rendered_prompt) -> StrategyResult.
  3. Add the class to REGISTRY.
  4. Select via EVAL_STRATEGY env var or pass the class directly to the runner.
"""
from __future__ import annotations

import json
import logging
import os
import time
import urllib.request
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from typing import Any

log = logging.getLogger("tests.evals.strategies")


# ---------------------------------------------------------------------------
# Base layer
# ---------------------------------------------------------------------------

@dataclass
class StrategyResult:
    """Standardised output from any classification strategy."""

    task_key: str | None
    confidence: float
    session_type: str
    reasoning: str
    elapsed_s: float
    method: str = "unknown"
    strategy_name: str = "unknown"
    extra: dict[str, Any] = field(default_factory=dict)

    def __post_init__(self) -> None:
        self.confidence = max(0.0, min(1.0, self.confidence))

    def as_actual_output(self) -> str:
        """Return the JSON string deepeval LLMTestCase.actual_output expects."""
        return json.dumps(
            {
                "task_key": self.task_key or "none",
                "session_type": self.session_type,
                "reasoning": self.reasoning,
            },
            ensure_ascii=False,
        )


class EvalStrategy(ABC):
    """Base class for eval-pipeline classification strategies.

    Each subclass encapsulates one approach to generating actual_output from a
    pre-rendered Golden prompt. The prompt is a fully-rendered string (the
    output of build_user_message) — strategies vary what they do with it:

      DirectHttpStrategy      POST to the MLX /classify HTTP server (baseline)
      ExtractThenClassify*    Two-stage: extract structured signals, then classify
      LargerModelHttp*        Same HTTP interface, different model or server
      RetrievalAugmented*     Fetch prior-classification priors before classifying

    * = future tasks
    """

    def __init__(self, name: str, config: dict[str, Any] | None = None) -> None:
        self.name = name
        self.config: dict[str, Any] = config or {}

    @abstractmethod
    def classify_prompt(self, rendered_prompt: str) -> StrategyResult:
        """Classify a session from its pre-rendered prompt string.

        Args:
            rendered_prompt: Full prompt string (Golden.input from eval dataset)

        Returns:
            StrategyResult — task_key, confidence, session_type, reasoning, elapsed_s
        """

    def as_hyperparameters(self) -> dict[str, str | int | float]:
        """Return a dict for deepeval's hyperparameters= argument.

        Deepeval requires dict[str, str | int | float]. Subclasses may override
        to add strategy-specific config keys (model, temperature, etc.).
        """
        params: dict[str, str | int | float] = {"strategy": self.name}
        for k, v in self.config.items():
            if isinstance(v, (str, int, float)):
                params[k] = v
        return params

    def _error_result(self, reason: str, elapsed: float = 0.0) -> StrategyResult:
        return StrategyResult(
            task_key=None,
            confidence=0.0,
            session_type="overhead",
            reasoning=reason,
            elapsed_s=elapsed,
            method=f"{self.name}_error",
            strategy_name=self.name,
            extra={"error": reason},
        )


# ---------------------------------------------------------------------------
# Concrete strategies
# ---------------------------------------------------------------------------

class DirectHttpStrategy(EvalStrategy):
    """POST the rendered prompt to the running MLX /classify HTTP endpoint.

    This is the baseline strategy — identical behaviour to the _classify_http
    helper that smoke_run.py used before the strategy abstraction was added.
    Config keys (all optional, override via EVAL_* env vars or explicit dict):
        endpoint   — full URL of /classify (default: http://127.0.0.1:7823/classify)
        timeout    — request timeout in seconds (default: 120)
        model      — model label for telemetry (default: from MLX_MODEL_ID env var)
    """

    def __init__(self, config: dict[str, Any] | None = None) -> None:
        cfg = {
            "endpoint": os.environ.get(
                "MLX_SERVER_URL", "http://127.0.0.1:7823"
            ).rstrip("/") + "/classify",
            "timeout": 120,
            "model": os.environ.get("MLX_MODEL_ID", "Qwen3.5-9B-OptiQ-4bit"),
        }
        if config:
            cfg.update(config)
        super().__init__("direct_http", cfg)

    def classify_prompt(self, rendered_prompt: str) -> StrategyResult:
        endpoint = self.config["endpoint"]
        timeout = int(self.config["timeout"])
        t0 = time.time()
        try:
            req = urllib.request.Request(
                endpoint,
                data=json.dumps({"input": rendered_prompt}).encode(),
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                data = json.loads(resp.read())
        except Exception as exc:
            return self._error_result(str(exc)[:200], time.time() - t0)

        elapsed = time.time() - t0
        return StrategyResult(
            task_key=data.get("task_key") or None,
            confidence=float(data.get("confidence", 0.0)),
            session_type=data.get("session_type", "overhead"),
            reasoning=data.get("reasoning", ""),
            elapsed_s=elapsed,
            method="http",
            strategy_name=self.name,
            extra={"dimensions": data.get("dimensions", {})},
        )

    def as_hyperparameters(self) -> dict[str, str | int | float]:
        return {
            "strategy": self.name,
            "model": str(self.config.get("model", "")),
            "endpoint": str(self.config.get("endpoint", "")),
        }


# ---------------------------------------------------------------------------
# ExtractThenClassifyStrategy — two-stage pipeline (Task #13)
# ---------------------------------------------------------------------------

_EXTRACT_SYSTEM_PROMPT = """\
You are an evidence extractor for a session classifier. You will read a "session" \
showing what a developer was doing on screen (OCR text, window titles, audio \
snippets), optionally preceded by "RECENT WORK CONTEXT" of prior sessions.

Your job is to describe what the user was ACTUALLY DOING, based on observable \
evidence. You do NOT classify the session, pick a ticket, or assign a task — \
that is a downstream step that depends on your output. Be precise; do not \
speculate beyond what the evidence shows.

Output ONE valid JSON object matching the schema below. No markdown, no \
prose, no code fences — just the JSON object.

SCHEMA:
{
  "primary_app": "<app the user was focused on, e.g. 'Slack', 'Code', 'Google Chrome'>",
  "user_action": "<ONE of the enum values below>",
  "ticket_mentions": [
    {
      "key": "<TICKET-NNN>",
      "evidence_type": "<ONE of: mentioned_in_chat | mentioned_in_url | topic_match | branch_name | file_path_match | actively_editing>",
      "context": "<≤60 chars: where you saw this mention>"
    }
  ],
  "active_work_signals": [
    "<short phrase per signal, e.g. 'editing AuthContext.tsx', 'running pytest', 'git commit pushed', 'PR opened'. EMPTY LIST if no active implementation activity.>"
  ],
  "evidence_strength_for_implementation": "<ONE of: none | weak | moderate | strong>"
}

USER ACTION ENUM (pick exactly ONE — the dominant action of the session):
  actively_implementing   — editing source code, running tests, running build
                            commands, pushing commits, opening PRs. The user is
                            PRODUCING artifacts.
  reviewing_pr            — viewing GitHub/GitLab PR pages, reading diffs,
                            leaving review comments. The user is REVIEWING work.
  discussing_in_chat      — reading or writing Slack/Teams/Mail/iMessage messages.
  researching_in_browser  — reading Stack Overflow, documentation, articles,
                            blog posts. The user is LEARNING / GATHERING info.
  passively_observing     — Zoom calls, screen-shares, video conferences. The
                            user is a VIEWER, not the driver.
  managing_tickets        — navigating Linear/Jira UI, updating ticket status,
                            viewing boards/lists. Ticket-administration UI work.
  writing_planning_notes  — composing design docs, architecture notes, planning
                            docs, README updates ABOUT future work.
  system_utility          — Activity Monitor, terminal for system maintenance,
                            checking RAM/CPU/processes. NOT productive work on
                            a specific feature.

EVIDENCE STRENGTH RUBRIC for `evidence_strength_for_implementation` — the
strongest implementation evidence linking the user to ANY specific ticket:
  strong:   user is actively editing code / running commands tied to a specific
            ticket's branch or file paths (e.g. branch='feat/proj-210-auth' AND
            editor open on src/auth/login.ts).
  moderate: editor open on the ticket's branch/files but no active editing
            (e.g. screen-share of code during a meeting; idle editor window).
  weak:     ticket key appears in chat / URL / article / planning notes, but no
            editor activity. The user is TALKING ABOUT or LOOKING AT the ticket,
            not implementing it.
  none:     no implementation activity at all (system utility, idle, etc.).

CRITICAL: ticket-key visibility ALONE is not "strong" evidence. A mention in
chat is `weak`. Editor-on-branch-without-editing is `moderate`. Active editing
is `strong`.
"""

_CLASSIFY_SYSTEM_PROMPT = """\
You are a session classifier. You receive (1) a structured evidence extraction \
from a stage-1 model and (2) a list of candidate tickets. You do NOT see the \
raw session text — only the structured extraction.

Decide:
  task_key:     ONE of the candidate ticket keys, or null
  session_type: ONE of: task | overhead | untracked

DECISION RULES (apply in order; the first matching rule wins):

RULE 1: If `evidence_strength_for_implementation` is "none" OR "weak":
  → task_key = null
  → session_type = overhead   if user_action is one of:
                                  discussing_in_chat
                                  passively_observing
                                  managing_tickets
                                  system_utility
  → session_type = untracked  if user_action is one of:
                                  researching_in_browser
                                  writing_planning_notes
                                  reviewing_pr (when no candidate matches)
                                  actively_implementing (when no candidate matches)

RULE 2: If `evidence_strength_for_implementation` is "moderate":
  → task_key = null
  → session_type = overhead
  (User is observing or has editor open passively. NOT task work.)

RULE 3: If `evidence_strength_for_implementation` is "strong" AND there is at
least one ticket_mention with `evidence_type` of `actively_editing` OR
`branch_name` whose `key` matches a candidate ticket:
  → task_key = <that candidate's key>
  → session_type = task

RULE 4: Otherwise:
  → task_key = null
  → session_type = overhead

CONFIDENCE:
  When rule 3 fires: confidence = 0.85–0.95 (depending on how many active_work_signals).
  When rule 1 fires with weak evidence_type=mentioned_in_chat or topic_match: confidence = 0.85–0.95.
  When rule 1 fires with no ticket_mentions at all: confidence = 0.90–0.95.
  When rule 2 or 4 fires: confidence = 0.70–0.85.

Output ONE valid JSON object. No markdown, no prose, no code fences.

SCHEMA:
{
  "task_key":     "<TICKET-NNN>" | null,
  "session_type": "task" | "overhead" | "untracked",
  "confidence":   <float 0.0-1.0>,
  "reasoning":    "<≤200 chars: cite the rule number and the deciding evidence>"
}
"""


class ExtractThenClassifyStrategy(EvalStrategy):
    """Two-stage classification — evidence extraction, then evidence-only classification.

    Stage 1 (extract): POST raw session text to /v1/chat/completions with the
    extraction system prompt. Output: structured JSON describing user_action,
    ticket_mentions (each with evidence_type), active_work_signals, and
    evidence_strength_for_implementation.

    Stage 2 (classify): POST the structured extraction + candidate ticket list
    (but NOT the raw session text) to /v1/chat/completions with the
    classification system prompt. Output: task_key + session_type + confidence
    + reasoning. The classifier is structurally pushed toward overhead/untracked
    when evidence is weak, breaking the "ticket-visible → task" reflex that
    drives optimism-bias in the single-shot baseline.

    Config keys (all optional, override via dict):
        endpoint                      — /v1/chat/completions URL
                                        (default: http://127.0.0.1:7823/v1/chat/completions)
        timeout                       — HTTP timeout seconds (default: 300)
        model                         — model label for telemetry
                                        (default: from MLX_MODEL_ID env var)
        extraction_temperature        — temp for stage 1 (default: 0.0)
        classification_temperature    — temp for stage 2 (default: 0.0)
        extraction_max_tokens         — max tokens stage 1 (default: 5000;
                                        Qwen3 chain-of-thought consumes 2-4k
                                        before emitting the answer JSON)
        classification_max_tokens     — max tokens stage 2 (default: 2000)
    """

    def __init__(self, config: dict[str, Any] | None = None) -> None:
        cfg = {
            "endpoint": os.environ.get(
                "MLX_SERVER_URL", "http://127.0.0.1:7823"
            ).rstrip("/") + "/v1/chat/completions",
            "timeout": 300,
            "model": os.environ.get("MLX_MODEL_ID", "Qwen3.5-9B-OptiQ-4bit"),
            "extraction_temperature":     0.0,
            "classification_temperature": 0.0,
            # Qwen3 emits chain-of-thought (~2-4k tokens) before the answer JSON.
            # Default extraction budget must comfortably cover thinking + output.
            # Lower extraction_max_tokens at your own risk — sub-3k will frequently
            # truncate before the JSON closes.
            "extraction_max_tokens":      5000,
            "classification_max_tokens":  2000,
        }
        if config:
            cfg.update(config)
        super().__init__("extract_then_classify", cfg)

    def classify_prompt(self, rendered_prompt: str) -> StrategyResult:
        t0 = time.time()

        # Split rendered prompt — everything before "CANDIDATE TICKETS:" goes to
        # stage 1, everything from "CANDIDATE TICKETS:" onwards goes to stage 2.
        sep = "CANDIDATE TICKETS:"
        sep_idx = rendered_prompt.rfind(sep)
        if sep_idx == -1:
            return self._error_result(
                "rendered prompt missing 'CANDIDATE TICKETS:' marker — "
                "cannot split into extraction / classification inputs",
                time.time() - t0,
            )
        session_block   = rendered_prompt[:sep_idx].rstrip()
        candidate_block = rendered_prompt[sep_idx:].rstrip()

        # Stage 1: extract structured evidence
        extraction, t1_elapsed, t1_err = self._post_and_parse_json(
            system_prompt=_EXTRACT_SYSTEM_PROMPT,
            user_prompt=session_block,
            temperature=float(self.config["extraction_temperature"]),
            max_tokens=int(self.config["extraction_max_tokens"]),
        )
        if t1_err:
            return self._error_result(
                f"stage1 extract: {t1_err}", time.time() - t0
            )

        # Stage 2: classify from extraction + candidates only
        classify_user_prompt = (
            "EXTRACTED EVIDENCE:\n"
            f"{json.dumps(extraction, ensure_ascii=False, indent=2)}\n\n"
            f"{candidate_block}\n"
        )
        classification, t2_elapsed, t2_err = self._post_and_parse_json(
            system_prompt=_CLASSIFY_SYSTEM_PROMPT,
            user_prompt=classify_user_prompt,
            temperature=float(self.config["classification_temperature"]),
            max_tokens=int(self.config["classification_max_tokens"]),
        )
        elapsed = time.time() - t0
        if t2_err:
            return self._error_result(
                f"stage2 classify: {t2_err}", elapsed
            )

        # Validate task_key is in the candidate list (or null). If the model
        # invented a ticket, drop it back to null + overhead.
        task_key = classification.get("task_key")
        if task_key is not None and task_key not in candidate_block:
            log.warning(
                "extract_then_classify: model returned task_key=%r not in candidates",
                task_key,
            )
            task_key = None

        return StrategyResult(
            task_key=task_key,
            confidence=float(classification.get("confidence", 0.0)),
            session_type=classification.get("session_type", "overhead"),
            reasoning=classification.get("reasoning", ""),
            elapsed_s=elapsed,
            method="http_extract_then_classify",
            strategy_name=self.name,
            extra={
                "extraction":           extraction,
                "extract_elapsed_s":    round(t1_elapsed, 3),
                "classify_elapsed_s":   round(t2_elapsed, 3),
                "extracted_action":     extraction.get("user_action"),
                "extracted_strength":   extraction.get("evidence_strength_for_implementation"),
            },
        )

    def _post_and_parse_json(
        self,
        *,
        system_prompt: str,
        user_prompt: str,
        temperature: float,
        max_tokens: int,
    ) -> "tuple[dict, float, str | None]":
        """POST to /v1/chat/completions and parse the assistant's text as JSON.

        Returns (parsed_dict, elapsed_seconds, error_or_none).
        On any HTTP, content, or JSON-parse error returns ({}, elapsed, error_str).
        """
        endpoint = self.config["endpoint"]
        timeout  = int(self.config["timeout"])
        payload = {
            "model":       self.config.get("model", "qwen"),
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user",   "content": user_prompt},
            ],
            "temperature": temperature,
            "max_tokens":  max_tokens,
        }
        t0 = time.time()
        try:
            req = urllib.request.Request(
                endpoint,
                data=json.dumps(payload).encode(),
                headers={"Content-Type": "application/json"},
                method="POST",
            )
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                data = json.loads(resp.read())
        except Exception as exc:
            return {}, time.time() - t0, f"HTTP error: {exc!s}"[:200]

        elapsed = time.time() - t0
        try:
            content = data["choices"][0]["message"]["content"]
        except (KeyError, IndexError, TypeError) as exc:
            return {}, elapsed, f"malformed response shape: {exc!s}"[:200]

        # Qwen3 emits a chain-of-thought wrapped in <think>…</think> before the
        # answer. The thinking section frequently contains a DRAFT JSON object
        # (often inside ```json fences) which would otherwise be picked up by a
        # naïve first-brace search. Strip everything up to and including the
        # LAST </think> marker so we only parse the post-thinking output.
        text = content
        if "</think>" in text:
            text = text.rsplit("</think>", 1)[1]
        text = text.strip()

        # Strip code fences if present.
        if text.startswith("```"):
            text = text.split("\n", 1)[1] if "\n" in text else text[3:]
            if text.rstrip().endswith("```"):
                text = text.rstrip()[:-3].rstrip()

        # Locate the first JSON object and parse it with raw_decode so any
        # trailing prose / additional objects don't break parsing.
        first_brace = text.find("{")
        if first_brace == -1:
            return {}, elapsed, f"no JSON object found in response: {text[:160]!r}"
        try:
            parsed, _ = json.JSONDecoder().raw_decode(text[first_brace:])
        except json.JSONDecodeError as exc:
            return {}, elapsed, f"JSON parse error: {exc!s} — snippet: {text[first_brace:first_brace+160]!r}"

        if not isinstance(parsed, dict):
            return {}, elapsed, f"response is not a JSON object: {type(parsed).__name__}"

        return parsed, elapsed, None

    def as_hyperparameters(self) -> dict[str, str | int | float]:
        return {
            "strategy": self.name,
            "model":    str(self.config.get("model", "")),
            "endpoint": str(self.config.get("endpoint", "")),
            "extraction_temperature":     float(self.config["extraction_temperature"]),
            "classification_temperature": float(self.config["classification_temperature"]),
            "extraction_max_tokens":      int(self.config["extraction_max_tokens"]),
            "classification_max_tokens":  int(self.config["classification_max_tokens"]),
        }


# ---------------------------------------------------------------------------
# Registry — maps EVAL_STRATEGY env var values to strategy classes.
# ---------------------------------------------------------------------------

REGISTRY: dict[str, type[EvalStrategy]] = {
    "direct_http":           DirectHttpStrategy,
    "extract_then_classify": ExtractThenClassifyStrategy,
}


def from_env() -> EvalStrategy:
    """Instantiate the strategy named by EVAL_STRATEGY (default: direct_http)."""
    name = os.environ.get("EVAL_STRATEGY", "direct_http")
    cls = REGISTRY.get(name)
    if cls is None:
        known = ", ".join(REGISTRY)
        raise ValueError(
            f"Unknown EVAL_STRATEGY={name!r}. Known strategies: {known}"
        )
    return cls()


def from_config(config: dict[str, Any]) -> EvalStrategy:
    """Instantiate a strategy from an experiment-config dict.

    Required key: 'strategy' (one of REGISTRY keys).
    All other dict entries are passed to the strategy constructor as its config
    (each strategy picks the keys it cares about and ignores the rest, so the
    same flat config can be reused across strategies). Non-string/int/float
    values like nested dicts are passed through unchanged.

    Reserved top-level keys (NOT forwarded to the strategy — they're consumed
    by the runner before strategy instantiation):
        name, description  — experiment identity (recorded in results.json)
        dataset_path       — Goldens file (runner picks this up directly)

    Everything else (model, endpoint, timeout, temperature, max_tokens,
    session_text_cap, prompt_version, ...) is passed through to the strategy
    constructor. Each strategy picks the keys it cares about and ignores the
    rest; as_hyperparameters() filters non-primitives. This means the config's
    `model` field flows into the strategy's hyperparameters (and from there
    into deepeval + OO trace attributes), so a config-declared model overrides
    the env-var default — even though the runner cannot enforce that the MLX
    server is actually serving that model (user must restart the server with
    MLX_MODEL_ID set; see services/scripts/install-mlx-server-daemon.sh).
    """
    name = config.get("strategy")
    if not name:
        raise ValueError("Config dict missing required 'strategy' key")
    cls = REGISTRY.get(name)
    if cls is None:
        known = ", ".join(REGISTRY)
        raise ValueError(f"Unknown strategy {name!r}. Known strategies: {known}")
    runner_keys = {"strategy", "name", "description", "dataset_path"}
    strategy_cfg = {k: v for k, v in config.items() if k not in runner_keys}
    return cls(strategy_cfg)
