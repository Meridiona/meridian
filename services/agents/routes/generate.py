"""Worklog + ticket generation routes — /generate_worklog and /propose_ticket.

Both use the FSM (grammar-constrained) JSON pattern of /classify_tasks: Qwen3.5-2B
generates against a Pydantic schema via :func:`agents.structured.generate_structured`,
so the emitted object is always structurally valid (thinking is off for JSON calls).
These replace the former agno schema-agents in the worklog pipeline's finalize stage —
the pipeline now calls these endpoints over HTTP exactly as the classifier calls
/classify_tasks.

  /generate_worklog — one ticket (matched OR newly proposed) → a WorklogDraft (schema WorklogOut).
  /propose_ticket   — the hour's residual work → a new Task/Bug, or an abstention (schema ProposeOut).
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
from agents.structured import generate_structured
from agents.schemas import WorklogOut, ProposeOut

log = logging.getLogger("agents.server")

router = APIRouter()

# Generation output is short (a worklog narrative or a small ticket). FSM + the
# bounded schemas close the object well before this; it's only a hard backstop.
_MAX_TOKENS = 4096


# ── Request / response models ─────────────────────────────────────────────────

class _WorklogRequest(BaseModel):
    report:         str
    distilled_body: str = ""
    task_key:       str
    title:          str
    description:    str = ""
    why:            str = ""
    is_new:         bool = False   # True when drafting for a freshly proposed ticket
    max_tokens:     int = _MAX_TOKENS
    traceparent:    str | None = None


class _WorklogResponse(BaseModel):
    summary:       str
    what_shipped:  list[str] = []
    decisions:     list[str] = []
    confidence:    float = 0.0
    input_tokens:  int = 0
    output_tokens: int = 0
    think_tokens:  int = 0
    elapsed_s:     float = 0.0


class _MatchedTicket(BaseModel):
    task_key: str
    title:    str = ""
    why:      str = ""


class _ProposeRequest(BaseModel):
    report:         str
    distilled_body: str = ""
    matched:        list[_MatchedTicket] = []
    max_tokens:     int = _MAX_TOKENS
    traceparent:    str | None = None


class _ProposeResponse(BaseModel):
    should_propose: bool = False
    issue_type:     str = "Task"
    title:          str = ""
    description:    str = ""
    reasoning:      str = ""
    input_tokens:   int = 0
    output_tokens:  int = 0
    think_tokens:   int = 0
    elapsed_s:      float = 0.0


# ── Output-format suffixes (appended to the system prompt) ─────────────────────

_WORKLOG_JSON_FORMAT = """

OUTPUT FORMAT
Respond with a single JSON object and nothing else — no markdown, no prose outside the JSON:
{
  "summary": "<2-4 sentence worklog comment>",
  "what_shipped": ["<short point>"],
  "decisions": [],
  "confidence": <0.0-1.0>
}
Leave any list empty when nothing applies."""

_PROPOSE_JSON_FORMAT = """

OUTPUT FORMAT
Respond with a single JSON object and nothing else — no markdown, no prose outside the JSON:
{
  "should_propose": <true|false>,
  "issue_type": "Task" | "Bug",
  "title": "<imperative, <=80 chars>",
  "description": "<2-4 sentences>",
  "reasoning": "<1-2 sentences: why this is a NEW ticket>"
}
When should_propose is false, set the other fields to empty strings ("") — propose nothing."""


# ── Helpers ───────────────────────────────────────────────────────────────────

def _clamp01(x) -> float:
    try:
        return max(0.0, min(1.0, float(x)))
    except (TypeError, ValueError):
        return 0.0


def _str_list(v) -> list[str]:
    """Coerce a parsed JSON value into a clean list of non-empty strings."""
    if not isinstance(v, list):
        return []
    return [s.strip() for s in v if isinstance(s, str) and s.strip()]


# ── /generate_worklog ─────────────────────────────────────────────────────────

@router.post("/generate_worklog", response_model=_WorklogResponse)
async def generate_worklog(req: _WorklogRequest) -> _WorklogResponse:
    """Draft a worklog for ONE ticket (matched or newly proposed)."""
    from fastapi.concurrency import run_in_threadpool
    import time as _time

    m = app_state.get("mlx_module")
    if m is None:
        raise HTTPException(status_code=503, detail="MLX model is still loading")

    from agents.worklog_pipeline.prompts.worklog import SYSTEM as WORKLOG_SYSTEM

    _parent_ctx = observability.extract_parent_context(req.traceparent)
    tracer = app_state.get("tracer") or trace.get_tracer("meridian-mlx-server")
    t_start = _time.time()

    ticket_kind = "newly proposed ticket" if req.is_new else "matched ticket"
    system_prompt = WORKLOG_SYSTEM + _WORKLOG_JSON_FORMAT
    user_content = (
        f"TICKET ({ticket_kind}): {req.task_key} — {req.title}\n{req.description}\n\n"
        f"WHY THIS HOUR MAPS TO THIS TICKET: {req.why}\n\n"
        f"ACTIVITY SUMMARY (last hour):\n{req.report}\n\n"
        f"CAPTURE DETAIL (grounding):\n{req.distilled_body[:8000]}"
    )
    messages = [
        {"role": "system", "content": system_prompt},
        {"role": "user",   "content": user_content},
    ]

    with tracer.start_as_current_span(
        "generate_worklog",
        context=_parent_ctx if _parent_ctx is not None else _otel_context.Context(),
    ) as span:
        span.set_attribute("task_key", req.task_key)
        span.set_attribute("is_new",   req.is_new)
        span.set_attribute("is_error", True)
        try:
            async with model_sem():
                res = await run_in_threadpool(
                    generate_structured, m, messages,
                    output_type=WorklogOut, max_tokens=req.max_tokens)
        except Exception as exc:  # noqa: BLE001
            span.set_status(trace.StatusCode.ERROR, str(exc))
            log.error("generate_worklog: inference error for %s: %s", req.task_key, exc)
            raise HTTPException(status_code=500, detail=str(exc)) from exc

        raw_json = res.text
        in_tok, out_tok, think_tok = res.input_tokens, res.output_tokens, res.think_tokens
        elapsed = round(_time.time() - t_start, 2)
        try:
            p = json.loads(raw_json)
        except (json.JSONDecodeError, TypeError) as exc:
            log.warning("generate_worklog: JSON parse failed for %s (%s) raw=%r",
                        req.task_key, exc, raw_json[:300])
            p = {}

        span.set_attribute("is_error", False)
        span.set_attribute("input_tokens", in_tok)
        span.set_attribute("output_tokens", out_tok)
        span.set_attribute("think_tokens", think_tok)
        span.set_attribute("elapsed_s", elapsed)
        observability.record_fsm_params(
            span, temp=DEFAULT_TEMP, max_tokens=req.max_tokens, schema="WorklogOut",
            model=m.MODEL_ID if hasattr(m, "MODEL_ID") else "")
        span.set_attribute("llm_output", observability.preview(raw_json, max_chars=4000))
        observability.record_llm_io(
            tracer, "generate_worklog",
            system_prompt=system_prompt, llm_input=user_content, llm_output=raw_json,
            input_tokens=in_tok, output_tokens=out_tok, think_tokens=think_tok)
        log.info("generate_worklog: fsm %s in_tok=%d out_tok=%d elapsed=%.1fs",
                 req.task_key, in_tok, out_tok, elapsed)

    return _WorklogResponse(
        summary=str(p.get("summary", "")),
        what_shipped=_str_list(p.get("what_shipped")),
        decisions=_str_list(p.get("decisions")),
        confidence=_clamp01(p.get("confidence", 0.0)),
        input_tokens=in_tok, output_tokens=out_tok, think_tokens=think_tok, elapsed_s=elapsed,
    )


# ── /propose_ticket ───────────────────────────────────────────────────────────

def _render_matched(matched: list[_MatchedTicket]) -> str:
    if not matched:
        return "(none — no existing ticket matched this hour)"
    lines = []
    for t in matched:
        head = f"{t.task_key}: {t.title}" if t.title.strip() else t.task_key
        lines.append(f"- {head} — {t.why}" if t.why.strip() else f"- {head}")
    return "\n".join(lines)


@router.post("/propose_ticket", response_model=_ProposeResponse)
async def propose_ticket(req: _ProposeRequest) -> _ProposeResponse:
    """Decide whether the hour's residual work needs a new Task/Bug, and draft it."""
    from fastapi.concurrency import run_in_threadpool
    import time as _time

    m = app_state.get("mlx_module")
    if m is None:
        raise HTTPException(status_code=503, detail="MLX model is still loading")

    from agents.worklog_pipeline.prompts.propose_ticket import SYSTEM as PROPOSE_SYSTEM

    _parent_ctx = observability.extract_parent_context(req.traceparent)
    tracer = app_state.get("tracer") or trace.get_tracer("meridian-mlx-server")
    t_start = _time.time()

    system_prompt = PROPOSE_SYSTEM + _PROPOSE_JSON_FORMAT
    user_content = (
        f"ACTIVITY SUMMARY (last hour):\n{req.report}\n\n"
        f"ALREADY MATCHED TICKETS (do not duplicate):\n{_render_matched(req.matched)}\n\n"
        f"CAPTURE DETAIL (grounding):\n{req.distilled_body[:6000]}"
    )
    messages = [
        {"role": "system", "content": system_prompt},
        {"role": "user",   "content": user_content},
    ]

    with tracer.start_as_current_span(
        "propose_ticket",
        context=_parent_ctx if _parent_ctx is not None else _otel_context.Context(),
    ) as span:
        span.set_attribute("matched", len(req.matched))
        span.set_attribute("is_error", True)
        try:
            async with model_sem():
                res = await run_in_threadpool(
                    generate_structured, m, messages,
                    output_type=ProposeOut, max_tokens=req.max_tokens)
        except Exception as exc:  # noqa: BLE001
            span.set_status(trace.StatusCode.ERROR, str(exc))
            log.error("propose_ticket: inference error: %s", exc)
            raise HTTPException(status_code=500, detail=str(exc)) from exc

        raw_json = res.text
        in_tok, out_tok, think_tok = res.input_tokens, res.output_tokens, res.think_tokens
        elapsed = round(_time.time() - t_start, 2)
        try:
            p = json.loads(raw_json)
        except (json.JSONDecodeError, TypeError) as exc:
            log.warning("propose_ticket: JSON parse failed (%s) raw=%r", exc, raw_json[:300])
            p = {}

        should = bool(p.get("should_propose", False))
        itype = "Bug" if str(p.get("issue_type", "Task")).strip().lower() == "bug" else "Task"

        span.set_attribute("is_error", False)
        span.set_attribute("should_propose", should)
        span.set_attribute("issue_type", itype)
        span.set_attribute("input_tokens", in_tok)
        span.set_attribute("output_tokens", out_tok)
        span.set_attribute("think_tokens", think_tok)
        span.set_attribute("elapsed_s", elapsed)
        observability.record_fsm_params(
            span, temp=DEFAULT_TEMP, max_tokens=req.max_tokens, schema="ProposeOut",
            model=m.MODEL_ID if hasattr(m, "MODEL_ID") else "")
        span.set_attribute("llm_output", observability.preview(raw_json, max_chars=4000))
        observability.record_llm_io(
            tracer, "propose_ticket",
            system_prompt=system_prompt, llm_input=user_content, llm_output=raw_json,
            input_tokens=in_tok, output_tokens=out_tok, think_tokens=think_tok)
        log.info("propose_ticket: fsm should=%s type=%s in_tok=%d out_tok=%d elapsed=%.1fs",
                 should, itype, in_tok, out_tok, elapsed)

    # Abstention: hand back a clean negative regardless of any stray drafted fields.
    if not should:
        return _ProposeResponse(
            should_propose=False, reasoning=str(p.get("reasoning", "")),
            input_tokens=in_tok, output_tokens=out_tok, think_tokens=think_tok, elapsed_s=elapsed,
        )
    return _ProposeResponse(
        should_propose=True,
        issue_type=itype,
        title=str(p.get("title", "")).strip()[:80],
        description=str(p.get("description", "")).strip(),
        reasoning=str(p.get("reasoning", "")).strip()[:300],
        input_tokens=in_tok, output_tokens=out_tok, think_tokens=think_tok, elapsed_s=elapsed,
    )
