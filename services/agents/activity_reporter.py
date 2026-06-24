"""Activity reporter — HTTP client for the /activity_report MLX server endpoint.

Converts session_distiller output into a human-readable worklog entry suitable
for PM updates and standup notes. The report covers all activity types: coding,
research, debugging, reading, reviewing — written for a non-technical audience.

Public API
----------
    report_activity(body, label, ...)  -> ActivityReport
"""
from __future__ import annotations

import json
import logging
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Optional

from agents import observability

log    = logging.getLogger(__name__)
tracer = observability.setup("meridian-activity-reporter")

_DEFAULT_SERVER      = "http://127.0.0.1:7823"
_DEFAULT_MAX_TOKENS  = 4096
_REQUEST_TIMEOUT_S   = 300


@dataclass(frozen=True)
class ActivityReport:
    """Human-readable worklog entry for one time window."""
    label:         str
    report:        str    # free-form markdown — ready to paste into a PM tool
    input_tokens:  int
    output_tokens: int
    think_tokens:  int    # tokens spent in <think> block (stripped from report)
    elapsed_s:     float


def report_activity(
    body: str,
    label: str,
    server_url: str = _DEFAULT_SERVER,
    max_tokens: int = _DEFAULT_MAX_TOKENS,
    traceparent: Optional[str] = None,
) -> ActivityReport:
    """POST distilled session body to /activity_report and return a worklog entry.

    Args:
        body:        Distilled hour body from distil_hour() / distil_range().
        label:       Human label for the window (e.g. '2026-06-23T13').
        server_url:  Base URL of the MLX server (default: http://127.0.0.1:7823).
        max_tokens:  Generation budget — 4096 gives a full worklog with all sections.
        traceparent: W3C traceparent to propagate the caller's trace context.

    Returns:
        ActivityReport with free-form markdown report and token counts.

    Raises:
        urllib.error.URLError: if the server is unreachable.
        ValueError:            if the server returns a non-200 status or bad JSON.
    """
    from opentelemetry.trace import StatusCode

    t_start = time.monotonic()

    payload = json.dumps({
        "body":        body,
        "label":       label,
        "max_tokens":  max_tokens,
        "traceparent": traceparent,
    }).encode()

    req = urllib.request.Request(
        f"{server_url.rstrip('/')}/activity_report",
        data=payload,
        headers={"Content-Type": "application/json"},
        method="POST",
    )

    with tracer.start_as_current_span("activity_reporter.call") as span:
        span.set_attribute("distil_label",     label)
        span.set_attribute("input_chars",      len(body))
        span.set_attribute("server_url",       server_url)
        span.set_attribute("is_error",         True)

        try:
            with urllib.request.urlopen(req, timeout=_REQUEST_TIMEOUT_S) as resp:
                raw = resp.read()
        except urllib.error.URLError as exc:
            span.set_status(StatusCode.ERROR, str(exc))
            log.error(
                "activity_reporter: server unreachable at %s: %s",
                server_url, exc,
                extra={"label": label},
            )
            raise

        try:
            data = json.loads(raw)
        except json.JSONDecodeError as exc:
            span.set_status(StatusCode.ERROR, f"bad JSON: {exc}")
            raise ValueError(f"activity_report: bad JSON response: {exc}") from exc

        report        = data.get("report", "").strip()
        input_tokens  = int(data.get("input_tokens",  0))
        output_tokens = int(data.get("output_tokens", 0))
        think_tokens  = int(data.get("think_tokens",  0))
        elapsed       = round(time.monotonic() - t_start, 2)

        span.set_attribute("input_tokens",  input_tokens)
        span.set_attribute("output_tokens", output_tokens)
        span.set_attribute("think_tokens",  think_tokens)
        span.set_attribute("output_chars",  len(report))
        span.set_attribute("elapsed_s",     elapsed)
        span.set_attribute("is_error",      False)

    log.info(
        "activity_reporter: label=%s in_tok=%d out_tok=%d think_tok=%d elapsed=%.1fs",
        label, input_tokens, output_tokens, think_tokens, elapsed,
        extra={
            "label":         label,
            "input_tokens":  input_tokens,
            "output_tokens": output_tokens,
            "think_tokens":  think_tokens,
        },
    )

    return ActivityReport(
        label=label,
        report=report,
        input_tokens=input_tokens,
        output_tokens=output_tokens,
        think_tokens=think_tokens,
        elapsed_s=elapsed,
    )
