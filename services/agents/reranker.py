"""Single-slot cross-encoder reranker for the worklog pipeline.

Scores a worklog/activity query against PM-ticket documents with
Qwen3-Reranker-0.6B (the yes/no-logit trick: one forward pass, softmax over the
"yes"/"no" token logits at the final position). The reranker output is a HINT
only — the matching LLM makes the final call — so absolute calibration matters
less than the relative ranking it produces.

Single-slot discipline: this model must never be resident alongside the
generative model. ``score_candidates`` evicts the generative model first
(``evict_resident_model``), loads the reranker, scores, then unloads it so the
generative model can lazily reload for the matching step. One load+unload per
hourly worklog run — cheap, and it honours the one-model-at-a-time rule.
"""
from __future__ import annotations

import gc
import logging
import threading
import time
from typing import Any

from agents import model_registry

log = logging.getLogger("meridian.reranker")

# Reranker checkpoint — resolved from the model registry, still env-overridable
# via WORKLOG_RERANKER_ID (the registry owns that env var).
_RERANKER_ID = model_registry.reranker_id()

# Reranker prompt scaffold — matches the calibration used in the offline
# benchmark (services/tests/evals/cluster/test_matcher_benchmark.py).
_INSTR = (
    "Given a developer worklog (the Query), judge whether the work described "
    "advances the goal of the project-management ticket (the Document). Answer "
    "yes only if completing this work would make progress on that specific ticket."
)
_PREFIX = (
    "<|im_start|>system\nJudge whether the Document meets the requirements based "
    'on the Query and the Instruct provided. Note that the answer can only be "yes" '
    'or "no".<|im_end|>\n<|im_start|>user\n'
)
_SUFFIX = "<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"

_model: Any = None
_tok: Any = None
_yes_id: int | None = None
_no_id: int | None = None
_lock = threading.Lock()


def _load() -> None:
    global _model, _tok, _yes_id, _no_id
    if _model is not None:
        return
    from mlx_lm import load

    log.info("reranker: loading %s", _RERANKER_ID)
    t0 = time.time()
    _model, _tok = load(_RERANKER_ID)
    _yes_id = _tok.encode("yes", add_special_tokens=False)[0]
    _no_id = _tok.encode("no", add_special_tokens=False)[0]
    log.info("reranker: loaded in %.1fs", time.time() - t0)


def unload() -> None:
    """Free the reranker so the generative model can reload. Idempotent."""
    global _model, _tok, _yes_id, _no_id
    if _model is None:
        return
    _model = _tok = _yes_id = _no_id = None
    gc.collect()
    try:
        import mlx.core as mx

        mx.clear_cache()
    except Exception:  # noqa: BLE001
        pass
    log.info("reranker: unloaded")


def _score_one(query: str, doc: str) -> float:
    import mlx.core as mx
    import mlx.nn as nn

    ids = _tok.encode(
        f"{_PREFIX}<Instruct>: {_INSTR}\n<Query>: {query}\n<Document>: {doc}{_SUFFIX}",
        add_special_tokens=False,
    )
    lg = _model(mx.array([ids]))[0, -1, :]
    p = nn.softmax(mx.array([lg[_no_id].item(), lg[_yes_id].item()]))[1].item()
    mx.clear_cache()
    return float(p)


def score_candidates(query: str, candidates: list[dict]) -> list[dict]:
    """Score ``query`` against each candidate ticket; return descending by score.

    ``candidates``: ``[{"task_key": str, "doc": str}, ...]`` where ``doc`` is the
    rendered ticket text. Returns ``[{"task_key", "score"}, ...]`` sorted high→low.
    Evicts the generative model first and unloads the reranker afterwards so only
    one model is ever resident.
    """
    if not candidates:
        return []
    from agents.mlx_classifier import evict_resident_model

    with _lock:
        evict_resident_model()
        _load()
        try:
            scored = [
                {"task_key": c["task_key"], "score": _score_one(query, c["doc"])}
                for c in candidates
            ]
        finally:
            unload()
    scored.sort(key=lambda x: -x["score"])
    return scored
