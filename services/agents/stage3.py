"""Stage 3 — LLM tiebreaker.

Runs only when Stage 2 returns `routing="queue"` — i.e. the embedding+rules
score gave us a candidate but couldn't separate the top-K confidently. Stage
3 hands the top candidates and the session evidence to a small LLM and asks
for a single JSON decision. No tool calls, no conversation loop, one prompt
in / one structured object out.

Provider/model switching: the LLM endpoint is whatever HERMES_BASE_URL +
HERMES_MODEL + (OLLAMA_API_KEY | ANTHROPIC_API_KEY) point at. Same
environment hermes uses, but we talk to it directly via the OpenAI SDK
because we don't need the AIAgent runtime — there are no tools to route.

Default model is whatever's already in services/.env (today: nemotron-3-super
on Ollama Cloud). To switch to a local gemma3 or qwen2.5, just point
HERMES_BASE_URL at http://localhost:11434/v1 and set HERMES_MODEL.
"""
from __future__ import annotations

import json
import logging
import os
import re
import time
from dataclasses import dataclass, field
from typing import Any

from agents.config import MODEL, BASE_URL, API_KEY

log = logging.getLogger("agents.stage3")

# ──────────────────────── Config / thresholds ─────────────────────────────────
# Stage 3's confidence is what the LLM reports. We use it to decide whether
# the result deserves auto-dispatch (high) or stays in the queue for human
# review (medium). Below the QUEUE floor we treat as no decision and revert
# to Stage 2's queue verdict.
STAGE3_AUTO_FLOOR  = float(os.environ.get("STAGE3_AUTO_FLOOR",  "0.65"))
STAGE3_QUEUE_FLOOR = float(os.environ.get("STAGE3_QUEUE_FLOOR", "0.40"))
STAGE3_MAX_TOKENS  = int(os.environ.get("STAGE3_MAX_TOKENS", "2000"))
STAGE3_TIMEOUT_S   = float(os.environ.get("STAGE3_TIMEOUT_S",  "60"))


# ──────────────────────── Result type ─────────────────────────────────────────
@dataclass
class Stage3Result:
    session_id: int
    chosen_task_key: str | None
    confidence: float
    reasoning: str
    routing: str                      # 'auto' | 'queue' | 'skip'
    method: str                       # 'stage3_llm' | 'stage3_unavailable' | 'stage3_invalid_response'
    raw_response: str = ""
    elapsed_s: float = 0.0
    debug: dict = field(default_factory=dict)


# ──────────────────────── OpenAI client cache ─────────────────────────────────
_client = None


def _get_client():
    """Lazy-load an OpenAI-compatible client pointed at the configured provider.

    Works against Ollama (cloud or local), OpenAI, Anthropic-via-openai-shim,
    LM Studio, vLLM, anything that speaks OpenAI's chat-completions API.
    """
    global _client
    if _client is not None:
        return _client
    try:
        from openai import OpenAI
    except ImportError as exc:
        raise ImportError(
            "Stage 3 needs the `openai` Python SDK. "
            "Install with: pip install 'openai>=2,<3'"
        ) from exc
    base_url = (BASE_URL or "").rstrip("/")
    api_key  = API_KEY or "none"
    log.info("stage3: using model=%s  base_url=%s", MODEL, base_url or "(default)")
    _client = OpenAI(base_url=base_url or None, api_key=api_key, timeout=STAGE3_TIMEOUT_S)
    return _client


# ──────────────────────── Prompt builder ──────────────────────────────────────
def _format_session(session: dict) -> str:
    parts: list[str] = []
    parts.append(f"app: {session.get('app_name') or '?'}")
    cat = session.get("category")
    cat_conf = session.get("confidence")
    if cat:
        parts.append(f"category: {cat} (confidence {round(cat_conf or 0.0, 2)})")
    dur = session.get("duration_s")
    if dur is not None:
        parts.append(f"duration: {dur}s")
    titles = session.get("window_titles") or []
    if titles:
        parts.append("top windows:")
        for t in titles[:5]:
            if isinstance(t, dict):
                name = t.get("window_name") or t.get("title") or ""
                cnt  = t.get("count", 1)
            elif isinstance(t, (list, tuple)) and t:
                name, cnt = t[0], (t[1] if len(t) > 1 else 1)
            else:
                name, cnt = str(t), 1
            # Strip the noisy VS Code extension banner.
            name = re.sub(
                r"\s+[—-]+\s+The following extensions want to relaunch.*$",
                "",
                name,
                flags=re.IGNORECASE | re.DOTALL,
            ).strip()
            parts.append(f"  • {name} (×{cnt})")
    ocr = session.get("ocr_samples") or []
    audio = session.get("audio_snippets") or []
    parts.append(f"ocr_samples: {len(ocr)} captured")
    if audio:
        parts.append(f"audio_snippets: {len(audio)} captured")
    return "\n".join(parts)


def _format_dimensions(dims_grouped: dict[str, set[str]]) -> str:
    if not dims_grouped:
        return "(none)"
    out: list[str] = []
    for dim in ("activity", "intent", "engagement", "collaboration",
                "tool", "topic", "practice"):
        vals = sorted(dims_grouped.get(dim, set()))
        if not vals:
            continue
        out.append(f"  - {dim}: {', '.join(vals[:8])}"
                   + (f" (+{len(vals) - 8} more)" if len(vals) > 8 else ""))
    return "\n".join(out) or "(none)"


def _format_candidates(top_candidates: list, pm_task_lookup: dict[str, dict]) -> str:
    """top_candidates is a list of CandidateBreakdown from stage2."""
    rows: list[str] = []
    for i, c in enumerate(top_candidates, start=1):
        task = pm_task_lookup.get(c.task_key, {})
        title = (task.get("title") or "").strip()
        desc  = (task.get("description_text") or "").strip()
        if len(desc) > 240:
            desc = desc[:240] + "…"
        rows.append(
            f"{i}. {c.task_key} (cosine={c.cosine:.2f}, dim_overlap={c.dim_overlap:.2f}, "
            f"score={c.score:.2f})\n"
            f"   title: {title}\n"
            f"   description: {desc or '(empty)'}"
        )
    return "\n\n".join(rows) if rows else "(no candidates)"


def _build_prompt(
    session: dict,
    dims_grouped: dict[str, set[str]],
    top_candidates: list,
    pm_task_lookup: dict[str, dict],
) -> str:
    return (
        "You are tagging a screen-activity session with the most likely Jira ticket.\n"
        "Pick exactly one ticket from CANDIDATES, or return null if NONE fits the session.\n"
        "\n"
        "SESSION:\n"
        f"{_format_session(session)}\n"
        "\n"
        "OBSERVED DIMENSIONS (rule-extracted):\n"
        f"{_format_dimensions(dims_grouped)}\n"
        "\n"
        "CANDIDATE TICKETS (top by embedding similarity, all from this user's open queue):\n"
        f"{_format_candidates(top_candidates, pm_task_lookup)}\n"
        "\n"
        "Respond with VALID JSON only, no markdown fences, exactly this shape:\n"
        '{"task_key": "<KAN-N or null>",\n'
        ' "confidence": <number 0..1>,\n'
        ' "reasoning": "<1-2 sentences>"}\n'
        "\n"
        "Rules:\n"
        "- task_key MUST be one of the candidates above, or null.\n"
        "- confidence reflects how clearly the session matches that ticket; "
        "use < 0.40 only if you are not confident at all.\n"
        "- If the session looks like overhead (idle / chrome / unrelated), "
        'return {"task_key": null, "confidence": 0.0, "reasoning": "..."}.\n'
        "- Output JSON. Nothing else."
    )


# ──────────────────────── Response parsing ────────────────────────────────────
_NULL_LITERALS = {"", "null", "none", "n/a", "nil", "undefined"}


def _parse_response(text: str, valid_keys: set[str]) -> tuple[str | None, float, str, str | None]:
    """Returns (task_key, confidence, reasoning, error). error is None on success."""
    if not text:
        return None, 0.0, "", "empty response"
    # Accept JSON either bare or wrapped in a fenced block.
    candidate = text.strip()
    fence = re.search(r"```(?:json)?\s*(\{.*?\})\s*```", candidate, re.DOTALL)
    if fence:
        candidate = fence.group(1)
    else:
        m = re.search(r"\{.*\}", candidate, re.DOTALL)
        if m:
            candidate = m.group()
    try:
        obj = json.loads(candidate)
    except json.JSONDecodeError as exc:
        return None, 0.0, "", f"json decode failed: {exc}"
    if not isinstance(obj, dict):
        return None, 0.0, "", "response was not a JSON object"

    raw_key = obj.get("task_key")
    if isinstance(raw_key, str) and raw_key.strip().lower() in _NULL_LITERALS:
        raw_key = None
    if raw_key is not None and raw_key not in valid_keys:
        return None, 0.0, "", f"task_key {raw_key!r} not in candidate set"

    try:
        confidence = float(obj.get("confidence", 0.0))
    except (TypeError, ValueError):
        confidence = 0.0
    confidence = max(0.0, min(1.0, confidence))

    reasoning = str(obj.get("reasoning") or "")[:500]
    return raw_key, confidence, reasoning, None


def _routing_for(confidence: float, task_key: str | None) -> str:
    if task_key is None:
        return "skip"
    if confidence >= STAGE3_AUTO_FLOOR:
        return "auto"
    if confidence >= STAGE3_QUEUE_FLOOR:
        return "queue"
    return "skip"


# ──────────────────────── Public entry ────────────────────────────────────────
def stage3_decide(
    session: dict,
    dims_grouped: dict[str, set[str]],
    top_candidates: list,
    pm_task_lookup: dict[str, dict],
) -> Stage3Result:
    """Ask the configured LLM to break the tie between Stage-2 candidates."""
    sid = int(session["id"])
    valid_keys = {c.task_key for c in top_candidates}
    if not valid_keys:
        return Stage3Result(
            session_id=sid, chosen_task_key=None, confidence=0.0,
            reasoning="no candidates", routing="skip", method="stage3_unavailable",
        )

    prompt = _build_prompt(session, dims_grouped, top_candidates, pm_task_lookup)
    log.debug("stage3 prompt:\n%s", prompt)

    try:
        client = _get_client()
    except ImportError as exc:
        return Stage3Result(
            session_id=sid, chosen_task_key=None, confidence=0.0,
            reasoning=str(exc), routing="skip", method="stage3_unavailable",
        )

    t0 = time.time()
    raw = ""

    def _call(use_json_format: bool):
        kwargs: dict[str, Any] = {
            "model":       MODEL,
            "messages":    [{"role": "user", "content": prompt}],
            "temperature": 0.0,
            "max_tokens":  STAGE3_MAX_TOKENS,
        }
        if use_json_format:
            kwargs["response_format"] = {"type": "json_object"}
        return client.chat.completions.create(**kwargs)

    def _extract_text(resp) -> str:
        """Pull JSON-bearing text out of the response, accommodating
        thinking-style models that put chain-of-thought in `reasoning_content`
        and the actual answer in `content`."""
        if not resp.choices:
            return ""
        msg = resp.choices[0].message
        content = (getattr(msg, "content", None) or "").strip()
        if content:
            return content
        # Fallback: some providers expose reasoning fields on the message.
        for attr in ("reasoning_content", "reasoning", "thinking"):
            v = getattr(msg, attr, None)
            if isinstance(v, str) and v.strip():
                return v.strip()
        # Last-ditch: dict form (raw model output).
        try:
            d = msg.model_dump()
        except Exception:
            d = {}
        for k in ("content", "reasoning_content", "reasoning", "thinking", "text"):
            v = d.get(k)
            if isinstance(v, str) and v.strip():
                return v.strip()
        return ""

    try:
        # First try JSON mode. If the provider rejects it OR returns empty,
        # retry without — common failure mode with Ollama Cloud / smaller
        # models that don't enforce json_object reliably.
        try:
            resp = _call(use_json_format=True)
            raw = _extract_text(resp)
            if not raw:
                log.info("stage3: empty content with json_object — retrying without")
                resp = _call(use_json_format=False)
                raw = _extract_text(resp)
        except Exception as exc_first:
            log.info("stage3: json_object call failed (%s) — retrying without", exc_first)
            resp = _call(use_json_format=False)
            raw = _extract_text(resp)
        # If we got something but it ends mid-string, the LLM blew through max_tokens.
        # Log finish_reason for visibility.
        finish = getattr(resp.choices[0], "finish_reason", "") if resp and resp.choices else ""
        if finish and finish != "stop":
            log.warning("stage3: finish_reason=%s — response may be truncated", finish)
    except Exception as exc:
        elapsed = time.time() - t0
        log.warning("stage3 call failed: %s", exc)
        return Stage3Result(
            session_id=sid, chosen_task_key=None, confidence=0.0,
            reasoning=f"LLM call failed: {exc}", routing="skip",
            method="stage3_unavailable", elapsed_s=elapsed,
        )

    elapsed = time.time() - t0
    log.debug("stage3 raw response (%.1fs): %s", elapsed, raw[:1000])

    task_key, confidence, reasoning, err = _parse_response(raw, valid_keys)
    if err:
        log.warning("stage3 invalid response: %s", err)
        return Stage3Result(
            session_id=sid, chosen_task_key=None, confidence=0.0,
            reasoning=err, routing="skip", method="stage3_invalid_response",
            raw_response=raw[:1000], elapsed_s=elapsed,
            debug={"error": err},
        )

    routing = _routing_for(confidence, task_key)
    return Stage3Result(
        session_id=sid,
        chosen_task_key=task_key,
        confidence=confidence,
        reasoning=reasoning,
        routing=routing,
        method="stage3_llm",
        raw_response=raw[:1000],
        elapsed_s=elapsed,
        debug={
            "model":         MODEL,
            "base_url":      BASE_URL,
            "n_candidates":  len(valid_keys),
            "auto_floor":    STAGE3_AUTO_FLOOR,
            "queue_floor":   STAGE3_QUEUE_FLOOR,
        },
    )


__all__ = ["Stage3Result", "stage3_decide", "STAGE3_AUTO_FLOOR", "STAGE3_QUEUE_FLOOR"]
