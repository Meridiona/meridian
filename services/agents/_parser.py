"""JSON response parsing helpers for task_classifier_agent — internal, not part of the public API."""
from __future__ import annotations

import json
import re

from ._prompts import parse_dimensions

_NULL_LITERALS = {"", "null", "none", "n/a", "nil", "undefined"}


def extract_json(text: str) -> str | None:
    """Pull the first JSON object out of a possibly-fenced or chatty response.

    Tolerant of truncation: thinking-style models sometimes blow through
    max_tokens mid-JSON-string. We attempt to repair by closing dangling
    quotes and brace depth so the partial object still parses.
    """
    if not text:
        return None
    candidate = text.strip()

    # When the model is truncated mid-response and then continued, the combined
    # final_response may contain multiple ```json blocks. The first block is the
    # incomplete truncated draft; the last is the correct answer. Try all fence
    # candidates in reverse order and return the last well-formed one.
    fences = list(re.finditer(r"```(?:json)?\s*(\{[^`]*?\})\s*```", candidate, re.DOTALL))
    for fence in reversed(fences):
        content = fence.group(1)
        sanitized = re.sub(r"[\x00-\x1f]", " ", content)
        try:
            json.loads(sanitized)
            return content
        except json.JSONDecodeError:
            continue

    m = re.search(r"\{.*\}", candidate, re.DOTALL)
    if m:
        return m.group()

    start = candidate.find("{")
    if start >= 0:
        partial = candidate[start:]
        return _repair_truncated_json(partial)
    return None


def _repair_truncated_json(partial: str) -> str:
    """Best-effort: close any dangling string, strip trailing commas inside
    the *current* object scope, then balance braces.

    Common truncation shapes handled:
      `{"k": "v"`              → `{"k": "v"}`
      `{"k": "v", "k2":`        → `{"k": "v"}`     (drop the orphan key)
      `{"k": "v", "k2": "v2`    → `{"k": "v", "k2": "v2"}`
      `{"k": 0.85,`             → `{"k": 0.85}`     (strip dangling comma)
    """
    out = partial
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

    if in_string:
        out += '"'

    if depth > 0 and not in_string:
        tail = out.rstrip()
        last_comma = tail.rfind(",")
        last_brace = tail.rfind("{")
        last_close = tail.rfind("}")
        if last_comma > max(last_brace, last_close):
            after_comma = tail[last_comma + 1:].strip()
            looks_unfinished = (
                after_comma == ""
                or after_comma.endswith(":")
                or (after_comma.count('"') == 1)
                or (after_comma.endswith(":") is False and ":" in after_comma
                    and after_comma.split(":", 1)[1].strip() == "")
            )
            if looks_unfinished:
                out = tail[:last_comma]

    open_braces = 0
    in_str = False
    esc = False
    for ch in out:
        if esc:
            esc = False
            continue
        if ch == "\\":
            esc = True
            continue
        if ch == '"':
            in_str = not in_str
            continue
        if in_str:
            continue
        if ch == "{":
            open_braces += 1
        elif ch == "}":
            open_braces = max(0, open_braces - 1)
    out += "}" * open_braces
    return out


_VALID_SESSION_TYPES = {"task", "overhead", "untracked"}


def routing_for(
    confidence: float,
    task_key: str | None,
    auto_floor: float,
    queue_floor: float,
) -> str:
    """Map (confidence, task_key) to a routing label.

    "auto"  — high-confidence match, write immediately
    "queue" — medium-confidence, needs review
    "skip"  — no task_key or confidence too low
    """
    if task_key is None:
        return "skip"
    if confidence >= auto_floor:
        return "auto"
    if confidence >= queue_floor:
        return "queue"
    return "skip"


def parse_response(
    text: str,
    valid_keys: set[str],
) -> tuple[str | None, float, str, dict[str, list[str]], str, str | None]:
    """Returns (task_key, confidence, reasoning, dimensions, session_type, error). error is None on success."""
    if not text:
        return None, 0.0, "", {}, "overhead", "empty response"
    candidate = extract_json(text)
    if candidate is None:
        return None, 0.0, "", {}, "overhead", "no JSON object in response"
    # Strip all ASCII control characters (0x00-0x1f) that are invalid as raw bytes
    # inside JSON strings. Literal newlines/tabs in the model's reasoning field
    # (from OCR content) cause json.loads to reject the response.
    candidate = re.sub(r"[\x00-\x1f]", " ", candidate)
    try:
        obj = json.loads(candidate)
    except json.JSONDecodeError as exc:
        return None, 0.0, "", {}, "overhead", f"json decode failed: {exc}"
    if not isinstance(obj, dict):
        return None, 0.0, "", {}, "overhead", "response was not a JSON object"

    raw_key = obj.get("task_key")
    if isinstance(raw_key, str) and raw_key.strip().lower() in _NULL_LITERALS:
        raw_key = None
    if raw_key is not None and raw_key not in valid_keys:
        return None, 0.0, "", {}, "overhead", f"task_key {raw_key!r} not in candidate set"

    try:
        confidence = float(obj.get("confidence", 0.0))
    except (TypeError, ValueError):
        confidence = 0.0
    confidence = max(0.0, min(1.0, confidence))

    reasoning  = str(obj.get("reasoning") or "")[:500]
    dimensions = parse_dimensions(obj)

    raw_st = str(obj.get("session_type") or "").strip().lower()
    session_type = raw_st if raw_st in _VALID_SESSION_TYPES else ("task" if raw_key else "overhead")

    return raw_key, confidence, reasoning, dimensions, session_type, None


