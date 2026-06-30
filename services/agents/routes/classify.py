"""Task classification route — /classify_tasks.

Matches one hour of distilled activity against candidate PM tasks using
Qwen3.5-2B with FSM (grammar-constrained) JSON generation via
:func:`agents.structured.generate_structured` + the :class:`agents.schemas.ClassifyOut`
schema. Thinking is OFF for JSON calls — the FSM guarantees a structurally valid
object (reasoning + matches), eliminating the parse failures the native-thinking +
budget-cap path leaves behind. Callers receive a ClassificationResult-shaped dict.
"""
from __future__ import annotations

import json
import logging

from fastapi import APIRouter, HTTPException
from opentelemetry import trace
from opentelemetry.context import context as _otel_context
from pydantic import BaseModel

from agents import observability
from agents._state import app_state, model_sem
from agents.thinking import DEFAULT_TEMP

log = logging.getLogger("agents.server")

router = APIRouter()

# Generation cap. FSM + the bounded ClassifyOut schema close the object well before
# this; it's only a hard backstop (no thinking tokens are spent under FSM).
_MAX_TOKENS = 4096

# FSM-classify sampling temperature. Kept at the shared structured default (0.1).
# scripts/eval-classify.py on the (tiny, noisy ±10%) 7-case decoy set: temp 0.1 holds
# FN=0 across repeats but takes occasional decoy FPs; temp 0.7 sometimes resists the
# decoy but ALSO drops real matches (FN=2 in a repeat). For a worklog product a missed
# real match (silent drop) is the cardinal sin — far worse than a recoverable decoy FP —
# so we favour the FN=0 setting. Decoy reasoning is what re-adding thinking will improve.
# Overridable per request (the tuning harness sweeps temp explicitly).
_CLASSIFY_TEMP = DEFAULT_TEMP


class _Candidate(BaseModel):
    task_key:     str
    title:        str
    doc:          str = ""
    rerank_score: float = 0.0


class _ClassifyRequest(BaseModel):
    report:      str
    candidates:  list[_Candidate]
    tier:        int = 1
    tier_note:   str = ""
    max_tokens:  int = _MAX_TOKENS
    traceparent: str | None = None
    # Overrides for the tuning harness (scripts/sweep-classify.py, eval-classify.py);
    # None = use the unified defaults in agents.thinking. Production never sets these.
    temp:            float | None = None
    thinking_budget: int | None = None
    enable_thinking: bool | None = None


class _ClassifyResponse(BaseModel):
    reasoning:     str
    matches:       list[dict]   # [{task_key, confidence, why}]
    input_tokens:  int
    output_tokens: int
    think_tokens:  int
    elapsed_s:     float


def _render_candidates(candidates: list[_Candidate]) -> str:
    lines = []
    for c in candidates:
        hint = f"  [reranker hint: {c.rerank_score:.2f}]" if c.rerank_score else ""
        lines.append(f"- {c.task_key}: {c.title}{hint}")
        if c.doc and c.doc.strip() != c.title.strip():
            lines.append(f"    {c.doc}")
    return "\n".join(lines)


_JSON_FORMAT = """

OUTPUT FORMAT
Respond with a single JSON object and nothing else — no markdown, no prose outside the JSON:
{
  "reasoning": "<your 2–4 sentence analysis>",
  "matches": [
    {"task_key": "<KEY>", "confidence": <0.0–1.0>, "why": "<one line explaining the concrete work>"}
  ]
}
If nothing matches, set "matches" to an empty array []."""


@router.post("/classify_tasks", response_model=_ClassifyResponse)
async def classify_tasks(req: _ClassifyRequest) -> _ClassifyResponse:
    """Classify one hour of activity against candidate PM tasks.

    Qwen3.5-2B with FSM (grammar-constrained) JSON via
    :func:`agents.structured.generate_structured` + :class:`agents.schemas.ClassifyOut`.
    Thinking is off; the FSM guarantees a valid object, so the returned JSON always
    parses. ``temp`` is overridable per request for the tuning harness;
    ``thinking_budget`` / ``enable_thinking`` are accepted but now no-ops under FSM.
    """
    from fastapi.concurrency import run_in_threadpool
    import time as _time

    from agents.structured import generate_structured
    from agents.schemas import ClassifyOut

    m = app_state.get("mlx_module")
    if m is None:
        raise HTTPException(status_code=503, detail="MLX model is still loading")

    from agents.worklog_pipeline.prompts.classify_tasks import CLASSIFY_SYSTEM

    _parent_ctx = observability.extract_parent_context(req.traceparent)
    tracer = app_state.get("tracer") or trace.get_tracer("meridian-mlx-server")
    t_start = _time.time()

    system_prompt = CLASSIFY_SYSTEM + _JSON_FORMAT
    user_content = (
        f"ACTIVITY SUMMARY (last hour):\n{req.report}\n\n"
        f"CANDIDATE TASKS ({req.tier_note}):\n{_render_candidates(req.candidates)}"
    )
    messages = [
        {"role": "system", "content": system_prompt},
        {"role": "user",   "content": user_content},
    ]

    # FSM (grammar-constrained) generation — thinking is OFF for JSON calls; the
    # schema guarantees structure, so there's no <think> block and no budget cap.
    # thinking_budget / enable_thinking request overrides are now no-ops (kept on the
    # model for backward-compat with the tuning harness); only temp still applies.
    _temp = req.temp if req.temp is not None else _CLASSIFY_TEMP

    with tracer.start_as_current_span(
        "classify_tasks",
        context=_parent_ctx if _parent_ctx is not None else _otel_context.Context(),
    ) as span:
        span.set_attribute("tier",          req.tier)
        span.set_attribute("tier_note",     req.tier_note)
        span.set_attribute("candidates",    len(req.candidates))
        span.set_attribute("model",         m.MODEL_ID if hasattr(m, "MODEL_ID") else "")
        span.set_attribute("is_error",      True)

        try:
            async with model_sem():
                res = await run_in_threadpool(
                    generate_structured, m, messages,
                    output_type=ClassifyOut, max_tokens=req.max_tokens, temp=_temp)
        except Exception as exc:  # noqa: BLE001
            span.set_status(trace.StatusCode.ERROR, str(exc))
            log.error("classify_tasks: inference error tier=%d: %s", req.tier, exc)
            raise HTTPException(status_code=500, detail=str(exc)) from exc

        raw_json = res.text
        input_tokens, output_tokens, think_tokens = res.input_tokens, res.output_tokens, res.think_tokens
        elapsed = round(_time.time() - t_start, 2)

        try:
            parsed = json.loads(raw_json)
            reasoning = parsed.get("reasoning", "")
            matches = parsed.get("matches", [])
        except (json.JSONDecodeError, AttributeError) as exc:
            log.warning("classify_tasks: JSON parse failed tier=%d (%s) raw=%r", req.tier, exc, raw_json[:300])
            reasoning = ""
            matches = []

        # Hallucination guard: the model occasionally returns a task_key that was
        # never offered as a candidate (e.g. a key it saw in the report text). A
        # match to a non-candidate is always invalid — drop it. This is a hard
        # correctness invariant, independent of the prompt.
        _valid_keys = {c.task_key for c in req.candidates}
        if isinstance(matches, list):
            _kept, _dropped = [], []
            for mtc in matches:
                k = mtc.get("task_key") if isinstance(mtc, dict) else None
                if k in _valid_keys:
                    # FSM enforces JSON grammar but not numeric value bounds — the
                    # model sometimes emits confidence as a 0–100 percentage. Normalise
                    # to [0,1] so downstream thresholds behave.
                    c = mtc.get("confidence")
                    try:
                        cf = float(c)
                        if cf > 1.0:
                            cf = cf / 100.0
                        mtc["confidence"] = max(0.0, min(1.0, cf))
                    except (TypeError, ValueError):
                        mtc["confidence"] = 0.0
                    _kept.append(mtc)
                else:
                    _dropped.append(mtc)
            if _dropped:
                log.warning("classify_tasks: dropped %d hallucinated key(s) tier=%d: %s",
                            len(_dropped), req.tier,
                            [m.get("task_key") if isinstance(m, dict) else m for m in _dropped])
                span.set_attribute("hallucinated_keys", len(_dropped))
            matches = _kept
        else:
            matches = []

        span.set_attribute("is_error",      False)
        span.set_attribute("input_tokens",  input_tokens)
        span.set_attribute("output_tokens", output_tokens)
        span.set_attribute("think_tokens",  think_tokens)
        span.set_attribute("matches",       len(matches))
        span.set_attribute("elapsed_s",     elapsed)
        observability.record_fsm_params(
            span, temp=_temp, max_tokens=req.max_tokens, schema="ClassifyOut",
            model=m.MODEL_ID if hasattr(m, "MODEL_ID") else "")
        span.set_attribute("llm_output",    observability.preview(raw_json, max_chars=4000))
        # Prompt / input / output child spans (same shape as activity_report).
        observability.record_llm_io(
            tracer, "classify_tasks",
            system_prompt=system_prompt, llm_input=user_content, llm_output=raw_json,
            input_tokens=input_tokens, output_tokens=output_tokens, think_tokens=think_tokens)

        log.info(
            "classify_tasks: fsm tier=%d candidates=%d matches=%d in_tok=%d out_tok=%d elapsed=%.1fs",
            req.tier, len(req.candidates), len(matches),
            input_tokens, output_tokens, elapsed,
        )

    return _ClassifyResponse(
        reasoning=reasoning,
        matches=matches,
        input_tokens=input_tokens,
        output_tokens=output_tokens,
        think_tokens=think_tokens,
        elapsed_s=elapsed,
    )
