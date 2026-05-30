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
# Registry — maps EVAL_STRATEGY env var values to strategy classes.
# ---------------------------------------------------------------------------

REGISTRY: dict[str, type[EvalStrategy]] = {
    "direct_http": DirectHttpStrategy,
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
