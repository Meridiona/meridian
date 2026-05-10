"""Stage 3 — LLM tiebreaker (hermes AIAgent driven).

Runs only when Stage 2 returns `routing="queue"` — i.e. the embedding+rules
score gave us candidates but couldn't separate them confidently. Stage 3
asks the LLM, via hermes' `AIAgent`, to read the candidate ticket
descriptions + session evidence and pick one (or none).

Why hermes (not direct openai SDK):
* model + provider switching across OpenAI / Anthropic / Ollama / LM Studio
  / Bedrock / Gemini / xAI without code changes — pick via env vars
* prompt caching, context compression, deterministic logging
* same configuration surface as the rest of the meridian agent stack

We deliberately use AIAgent in its simplest mode:
* `enabled_toolsets=[]`  — no tools to call
* `max_iterations=1`     — one model round, no agent loop
* system prompt loaded from skills/activity/stage3-tiebreaker/SKILL.md
  so prompt iteration doesn't require code changes.
"""
from __future__ import annotations

import json
import logging
import os
import re
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from agents.config import MODEL, BASE_URL, API_KEY, load_skill

log = logging.getLogger("agents.stage3")

# Make `import run_agent` work — the hermes runtime lives at services/run_agent.py.
_REPO_ROOT = Path(__file__).parent.parent
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))


# ──────────────────────── Config / thresholds ─────────────────────────────────
STAGE3_AUTO_FLOOR  = float(os.environ.get("STAGE3_AUTO_FLOOR",  "0.65"))
STAGE3_QUEUE_FLOOR = float(os.environ.get("STAGE3_QUEUE_FLOOR", "0.40"))
STAGE3_MAX_TOKENS  = int(os.environ.get("STAGE3_MAX_TOKENS", "4000"))
STAGE3_SKILL_NAME  = os.environ.get("STAGE3_SKILL_NAME", "stage3-tiebreaker")


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


# ──────────────────────── Prompt builder ──────────────────────────────────────
_VSCODE_BANNER_RE = re.compile(
    r"\s+[—-]+\s+The following extensions want to relaunch.*$",
    re.IGNORECASE | re.DOTALL,
)


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
            name = _VSCODE_BANNER_RE.sub("", name).strip()
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


def _build_user_message(
    session: dict,
    dims_grouped: dict[str, set[str]],
    top_candidates: list,
    pm_task_lookup: dict[str, dict],
) -> str:
    return (
        "SESSION:\n"
        f"{_format_session(session)}\n"
        "\n"
        "OBSERVED DIMENSIONS (rule-extracted):\n"
        f"{_format_dimensions(dims_grouped)}\n"
        "\n"
        "CANDIDATE TICKETS:\n"
        f"{_format_candidates(top_candidates, pm_task_lookup)}"
    )


# ──────────────────────── Response parsing ────────────────────────────────────
_NULL_LITERALS = {"", "null", "none", "n/a", "nil", "undefined"}


def _extract_json(text: str) -> str | None:
    """Pull the first JSON object out of a possibly-fenced or chatty response.

    Tolerant of truncation: thinking-style models sometimes blow through
    max_tokens mid-JSON-string. We attempt to repair by closing dangling
    quotes and brace depth so the partial object still parses.
    """
    if not text:
        return None
    candidate = text.strip()

    fence = re.search(r"```(?:json)?\s*(\{.*?\})\s*```", candidate, re.DOTALL)
    if fence:
        return fence.group(1)

    m = re.search(r"\{.*\}", candidate, re.DOTALL)
    if m:
        return m.group()

    # Truncated case: response starts with a brace but never closed.
    start = candidate.find("{")
    if start >= 0:
        partial = candidate[start:]
        return _repair_truncated_json(partial)
    return None


def _repair_truncated_json(partial: str) -> str:
    """Best-effort: close any dangling string, then balance braces.

    Used when the model ran out of tokens partway through emitting the
    JSON object. The repaired string is at minimum a syntactically valid
    object so `json.loads` can pull out whatever fields completed.
    """
    out = partial
    # Walk the string tracking quote state (ignoring escaped quotes).
    in_string = False
    escape = False
    depth = 0
    for ch in out:
        if escape:
            escape = False
            continue
        if ch == "\\":
            escape = True
            continue
        if ch == '"':
            in_string = not in_string
            continue
        if in_string:
            continue
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth = max(0, depth - 1)
    # Close any open string first.
    if in_string:
        out += '"'
    # Then close any open braces.
    out += "}" * depth
    return out


def _parse_response(text: str, valid_keys: set[str]) -> tuple[str | None, float, str, str | None]:
    """Returns (task_key, confidence, reasoning, error). error is None on success."""
    if not text:
        return None, 0.0, "", "empty response"
    candidate = _extract_json(text)
    if candidate is None:
        return None, 0.0, "", "no JSON object in response"
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
    """Ask the configured LLM (via hermes AIAgent) to break the tie between
    Stage-2 candidates."""
    sid = int(session["id"])
    valid_keys = {c.task_key for c in top_candidates}
    if not valid_keys:
        return Stage3Result(
            session_id=sid, chosen_task_key=None, confidence=0.0,
            reasoning="no candidates", routing="skip", method="stage3_unavailable",
        )

    user_message = _build_user_message(session, dims_grouped, top_candidates, pm_task_lookup)
    log.debug("stage3 user message:\n%s", user_message)

    try:
        system_prompt = load_skill(STAGE3_SKILL_NAME)
    except FileNotFoundError as exc:
        return Stage3Result(
            session_id=sid, chosen_task_key=None, confidence=0.0,
            reasoning=str(exc), routing="skip", method="stage3_unavailable",
        )

    try:
        from run_agent import AIAgent
    except ImportError as exc:
        return Stage3Result(
            session_id=sid, chosen_task_key=None, confidence=0.0,
            reasoning=f"hermes AIAgent import failed: {exc}",
            routing="skip", method="stage3_unavailable",
        )

    log.info("stage3: model=%s base_url=%s skill=%s", MODEL, BASE_URL, STAGE3_SKILL_NAME)

    t0 = time.time()
    raw = ""
    try:
        agent = AIAgent(
            model=MODEL,
            base_url=BASE_URL,
            api_key=API_KEY or "none",
            ephemeral_system_prompt=system_prompt,
            enabled_toolsets=[],          # no tools — single-shot completion
            quiet_mode=True,
            skip_context_files=True,
            load_soul_identity=False,
            skip_memory=True,
            max_iterations=1,             # one model round, no tool loop
            max_tokens=STAGE3_MAX_TOKENS,
        )
        result = agent.run_conversation(user_message)
    except Exception as exc:
        elapsed = time.time() - t0
        log.warning("stage3 AIAgent failed: %s", exc)
        return Stage3Result(
            session_id=sid, chosen_task_key=None, confidence=0.0,
            reasoning=f"AIAgent run failed: {exc}", routing="skip",
            method="stage3_unavailable", elapsed_s=elapsed,
        )

    elapsed = time.time() - t0

    # AIAgent.run_conversation returns a dict; the visible answer lives under
    # `final_response` (string). Some failure modes return None there.
    raw = ""
    if isinstance(result, dict):
        raw = str(
            result.get("final_response")
            or result.get("response")
            or ""
        ).strip()

    log.debug("stage3 raw response (%.1fs): %s", elapsed, raw[:1000])

    task_key, confidence, reasoning, err = _parse_response(raw, valid_keys)
    if err:
        log.warning("stage3 invalid response: %s", err)
        return Stage3Result(
            session_id=sid, chosen_task_key=None, confidence=0.0,
            reasoning=err, routing="skip", method="stage3_invalid_response",
            raw_response=raw[:1000], elapsed_s=elapsed,
            debug={"error": err, "model": MODEL, "base_url": BASE_URL},
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
            "skill":         STAGE3_SKILL_NAME,
        },
    )


__all__ = ["Stage3Result", "stage3_decide", "STAGE3_AUTO_FLOOR", "STAGE3_QUEUE_FLOOR"]
