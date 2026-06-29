"""Activity report route — /activity_report.

Takes session_distiller output and produces a human-readable markdown worklog
entry using the MLX model in thinking mode with a capped thinking budget.
Written for a non-technical audience (PMs, devs, stakeholders). Token counts
are from the model's own generation response.
"""
from __future__ import annotations

import logging

from fastapi import APIRouter, HTTPException
from opentelemetry import trace
from opentelemetry.trace import Link
from pydantic import BaseModel

from agents import observability
from agents._state import app_state, model_sem

log = logging.getLogger("agents.server")

router = APIRouter()

_ACTIVITY_REPORT_MAX_TOKENS = 16384   # budget for <think> block + answer


class _ActivityReportRequest(BaseModel):
    body:        str
    label:       str
    db_path:     str | None = None
    max_tokens:  int = _ACTIVITY_REPORT_MAX_TOKENS
    traceparent: str | None = None


class _ActivityReportResponse(BaseModel):
    report:        str    # free-form markdown worklog
    input_tokens:  int
    output_tokens: int
    think_tokens:  int    # tokens spent in <think> block (stripped from report)
    elapsed_s:     float


def _fetch_coding_summaries(db_path: str, label: str) -> str:
    """Return a formatted block of coding-agent session summaries for the given local hour label.

    Queries app_sessions for Claude Code / Codex / Copilot / Cursor rows whose
    started_at or ended_at overlaps the local hour, using the same UTC-range
    conversion as session_distiller so the label always means local time.
    Returns an empty string if none found or db_path is None.
    """
    import sqlite3
    from datetime import datetime, timedelta, timezone

    try:
        local_tz = datetime.now().astimezone().tzinfo
        local_start = datetime.strptime(label, "%Y-%m-%dT%H").replace(tzinfo=local_tz)
        utc_start = local_start.astimezone(timezone.utc)
        utc_end   = (local_start + timedelta(hours=1)).astimezone(timezone.utc)
        utc_start_s = utc_start.strftime("%Y-%m-%dT%H:%M:%S")
        utc_end_s   = utc_end.strftime("%Y-%m-%dT%H:%M:%S")

        conn = sqlite3.connect(db_path)
        rows = conn.execute("""
            SELECT app_name, started_at, ended_at, session_summary
            FROM app_sessions
            WHERE app_name IN ('Claude Code', 'Codex', 'GitHub Copilot', 'Cursor Agent')
              AND task_method = 'summarised'
              AND session_summary IS NOT NULL
              AND (
                  (started_at >= ? AND started_at < ?)
                  OR (ended_at  >= ? AND ended_at  < ?)
              )
            ORDER BY started_at
        """, (utc_start_s, utc_end_s, utc_start_s, utc_end_s)).fetchall()
        conn.close()
    except Exception as exc:  # noqa: BLE001
        log.warning("activity_report: could not fetch coding summaries: %s", exc)
        return ""

    if not rows:
        return ""

    lines = [f"\n---\n\n## Coding Agent Sessions ({len(rows)} session{'s' if len(rows) != 1 else ''})\n"]
    for app_name, started_at, ended_at, summary in rows:
        lines.append(f"### [{app_name}] {started_at[:16]} – {ended_at[:16]}")
        lines.append(summary.strip())
        lines.append("")
    return "\n".join(lines)


@router.post("/activity_report", response_model=_ActivityReportResponse)
async def activity_report(req: _ActivityReportRequest) -> _ActivityReportResponse:
    """Human-readable worklog entry from distilled session body.

    Uses Qwen3.5-2B in thinking mode (enable_thinking=True, thinking_budget=4096)
    with repetition_penalty=1.1 and presence_penalty=1.5 to prevent output loops.
    The thinking budget caps the <think> block so it doesn't consume the full
    max_tokens window. The <think> block is stripped — only the final answer is
    returned. Token counts are from the model's own generation response.
    """
    from fastapi.concurrency import run_in_threadpool
    import time as _time

    m = app_state.get("mlx_module")
    if m is None:
        raise HTTPException(status_code=503, detail="MLX model is still loading")

    _parent_ctx = observability.extract_parent_context(req.traceparent)
    tracer = app_state.get("tracer") or trace.get_tracer("meridian-mlx-server")
    t_start = _time.time()

    from agents.prompts.activity_report import SYSTEM as _AR_SYSTEM

    body = req.body
    if req.db_path:
        coding_block = _fetch_coding_summaries(req.db_path, req.label)
        if coding_block:
            body = body + coding_block
            log.info("activity_report: appended %d coding-summary chars", len(coding_block))

    messages = [
        {"role": "system", "content": _AR_SYSTEM},
        {"role": "user",   "content": body},
    ]

    def _generate() -> tuple[str, int, int, int]:
        """Returns (report_markdown, input_tokens, output_tokens, think_tokens).

        Uses the server's already-loaded model (model.model is the raw MLX net;
        m._get_tokenizer() is the HF tokenizer with Qwen3.5 chat template).
        enable_thinking=True requires the HF tokenizer's apply_chat_template, not
        outlines' Chat() shim, so we call mlx_lm.generate directly.
        """
        from mlx_lm import generate
        from mlx_lm.sample_utils import make_sampler, make_logits_processors

        sampler = make_sampler(temp=1.0, top_p=0.95, top_k=20)
        logits_processors = make_logits_processors(
            repetition_penalty=1.1,
            repetition_context_size=64,
            presence_penalty=1.5,
        )
        hf_tokenizer = m._get_tokenizer()
        prompt_ids = hf_tokenizer.apply_chat_template(
            messages,
            add_generation_prompt=True,
            enable_thinking=True,
            thinking_budget=8192,
        )
        # A fresh HF tokenizer returns a BatchEncoding ({input_ids, attention_mask});
        # the mlx_lm wrapper returns a plain token list. generate() needs the list.
        if hasattr(prompt_ids, "keys") and "input_ids" in prompt_ids:
            prompt_ids = prompt_ids["input_ids"]
        input_tokens = len(prompt_ids)

        with m.model_session() as model:
            raw = generate(
                model.model, hf_tokenizer,
                prompt=prompt_ids,
                max_tokens=req.max_tokens,
                sampler=sampler,
                logits_processors=logits_processors,
                verbose=False,
            )

        # Strip <think>…</think> block; count its tokens
        think_tokens = 0
        if "</think>" in raw:
            think_part, answer = raw.split("</think>", 1)
            think_tokens = len(hf_tokenizer.encode(think_part + "</think>"))
            raw = answer.strip()

        output_tokens = len(hf_tokenizer.encode(raw))
        return raw, input_tokens, output_tokens, think_tokens

    _links = []
    if _parent_ctx is not None:
        _parent_span_ctx = trace.get_current_span(_parent_ctx).get_span_context()
        if _parent_span_ctx and _parent_span_ctx.is_valid:
            _links = [Link(_parent_span_ctx)]

    with tracer.start_as_current_span("activity_report", links=_links) as span:
        span.set_attribute("gen_ai.operation.name", "chat")
        span.set_attribute("gen_ai.system",         "mlx")
        span.set_attribute("distil_label", req.label)
        span.set_attribute("input_chars",  len(body))
        span.set_attribute("model",        m.MODEL_ID)
        span.set_attribute("is_error",     True)

        # Span 1: system prompt — llm_input → OO renders as dedicated "Input" panel
        with tracer.start_as_current_span("activity_report.prompt") as prompt_span:
            prompt_span.set_attribute("label",       req.label)
            prompt_span.set_attribute("total_chars", len(_AR_SYSTEM))
            prompt_span.set_attribute("llm_input",   _AR_SYSTEM)
            prompt_span.set_attribute("body",        _AR_SYSTEM)

        # Span 2: user input (distilled text)
        # llm_input → OO dedicated Input panel (preview); body → full text in OO Attributes section
        with tracer.start_as_current_span("activity_report.input") as input_span:
            input_span.set_attribute("label",       req.label)
            input_span.set_attribute("total_chars", len(body))
            input_span.set_attribute("llm_input",   body)
            input_span.set_attribute("body",        body)

        try:
            async with model_sem():
                report, input_tokens, output_tokens, think_tokens = await run_in_threadpool(_generate)
        except Exception as exc:  # noqa: BLE001
            span.set_status(trace.StatusCode.ERROR, str(exc))
            log.error(
                "activity_report: inference error label=%s: %s",
                req.label, exc,
                extra={"label": req.label},
            )
            raise HTTPException(status_code=500, detail=str(exc)) from exc

        elapsed = round(_time.time() - t_start, 2)
        span.set_attribute("input_tokens",  input_tokens)
        span.set_attribute("output_tokens", output_tokens)
        span.set_attribute("think_tokens",  think_tokens)
        span.set_attribute("output_chars",  len(report))
        span.set_attribute("elapsed_s",     elapsed)
        span.set_attribute("is_error",      False)

        # Span 3: model output — llm_output → OO renders as dedicated "Output" panel
        with tracer.start_as_current_span("activity_report.output") as output_span:
            output_span.set_attribute("label",        req.label)
            output_span.set_attribute("total_chars",  len(report))
            output_span.set_attribute("input_tokens", input_tokens)
            output_span.set_attribute("output_tokens",output_tokens)
            output_span.set_attribute("think_tokens", think_tokens)
            output_span.set_attribute("llm_output",   observability.preview(report, max_chars=8000))
        _msg_prefix = "worklog.activity_report" if req.traceparent else "activity_report"
        _extra = {
            "ar_label":        req.label,
            "ar_input_tokens":  input_tokens,
            "ar_output_tokens": output_tokens,
            "ar_think_tokens":  think_tokens,
            "ar_elapsed_s":     elapsed,
        }
        log.info(
            "%s: label=%s in_tok=%d out_tok=%d think_tok=%d elapsed=%.1fs",
            _msg_prefix, req.label, input_tokens, output_tokens, think_tokens, elapsed,
            extra=_extra,
        )
    return _ActivityReportResponse(
        report=report,
        input_tokens=input_tokens,
        output_tokens=output_tokens,
        think_tokens=think_tokens,
        elapsed_s=elapsed,
    )
