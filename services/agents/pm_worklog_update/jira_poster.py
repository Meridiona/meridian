# meridian — normalises screenpipe activity into structured app sessions
"""Jira worklog poster — direct REST API implementation.

Single responsibility: turn a (task_key, time_spent_seconds, started_utc,
comment) into a successful POST to the Jira Cloud worklog endpoint, returning
the new worklog id.

Uses only stdlib (urllib.request, base64, json, os) — no new pip dependencies.
Credentials are read from env vars loaded from services/.env via python-dotenv.
"""
from __future__ import annotations

import base64
import json
import logging
import os
import urllib.error
import urllib.request
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional
from zoneinfo import ZoneInfo

log = logging.getLogger(__name__)

# ──────────────────────── Errors ───────────────────────────────────────────────


class JiraPostError(RuntimeError):
    """Anything that prevents a worklog from landing in Jira.

    Caller decides whether to retry or downgrade to DRAFTED. We never
    silently swallow — the row in `pm_updates` reflects reality.
    """


class JiraConfigError(JiraPostError):
    """Missing or malformed Jira env vars. Permanent until env fixed."""


# ──────────────────────── Public API ───────────────────────────────────────────


@dataclass(frozen=True)
class WorklogPostResult:
    worklog_id:         str
    issue_key:          str
    time_spent_jira:    str
    time_spent_seconds: int
    started_iso:        str
    raw_response:       dict[str, Any]


def post_worklog(
    *,
    task_key: str,
    time_spent_seconds: int,
    started_utc: datetime,
    comment: Optional[str] = None,
    timezone_name: Optional[str] = None,
) -> WorklogPostResult:
    """Post one worklog entry to Jira via the REST API.

    Args:
        task_key: Jira issue key, e.g. "KAN-64".
        time_spent_seconds: Real (idle-discounted) seconds. Must be >= 60.
        started_utc: Window-start moment in UTC. Rendered to local TZ for Jira.
        comment: Optional plain-text comment; wrapped in ADF by this function.
        timezone_name: Override for the started-time TZ. Defaults to
            `MERIDIAN_TZ` env var, then the host's local tzinfo, then
            `Asia/Kolkata`.

    Returns:
        `WorklogPostResult` with the new worklog id.

    Raises:
        JiraConfigError on missing credentials.
        JiraPostError on any Jira API failure.
    """
    if time_spent_seconds < 60:
        raise JiraPostError(
            f"time_spent_seconds={time_spent_seconds} below Jira's 60s minimum"
        )

    started_local = render_started_local(started_utc, tz_name=timezone_name)
    jira_time = seconds_to_jira_time(time_spent_seconds)

    env = _load_env()

    payload: dict[str, Any] = {
        "timeSpent": jira_time,
        "started": started_local,
        "comment": _build_adf_comment(comment or ""),
    }

    log.info(
        "jira worklog POST: task=%s time_spent=%s started=%s comment_len=%d",
        task_key, jira_time, started_local, len(comment or ""),
    )

    raw = _post_to_jira_api(task_key, payload, env)
    worklog_id = str(raw["id"])

    return WorklogPostResult(
        worklog_id=worklog_id,
        issue_key=task_key,
        time_spent_jira=jira_time,
        time_spent_seconds=time_spent_seconds,
        started_iso=started_local,
        raw_response=raw,
    )


# ──────────────────────── Helpers (importable, unit-tested in isolation) ───────


def seconds_to_jira_time(seconds: int) -> str:
    """Convert seconds → Jira's time-spent string (e.g. '1h 30m').

    Emits hours+minutes. Rounds to nearest minute — Jira rejects fractional
    minutes on the worklog API.
    """
    if seconds < 60:
        raise ValueError(f"seconds must be >= 60 for Jira worklog (got {seconds})")
    minutes_total = (seconds + 30) // 60        # round-to-nearest
    hours, minutes = divmod(minutes_total, 60)
    if hours and minutes:
        return f"{hours}h {minutes}m"
    if hours:
        return f"{hours}h"
    return f"{minutes}m"


def render_started_local(when_utc: datetime, *, tz_name: Optional[str] = None) -> str:
    """Render a UTC moment as `YYYY-MM-DDTHH:MM:SS.mmm+HHMM` for the Jira worklog API.

    No colon in the tz offset — e.g. `2026-05-29T13:18:25.000+0530`.
    """
    if when_utc.tzinfo is None:
        when_utc = when_utc.replace(tzinfo=timezone.utc)

    tz = _resolve_tz(tz_name)
    local = when_utc.astimezone(tz)
    millis = f"{local.microsecond // 1000:03d}"
    # `%z` gives `+HHMM` on all modern Python versions — no colon, correct.
    return local.strftime(f"%Y-%m-%dT%H:%M:%S.{millis}%z")


# ──────────────────────── Internals ────────────────────────────────────────────


def _load_env() -> dict[str, str]:
    """Load services/.env via python-dotenv and return a validated env dict.

    Raises `JiraConfigError` if JIRA_URL, JIRA_EMAIL, or JIRA_API_TOKEN
    are absent after loading .env.
    """
    try:
        from dotenv import load_dotenv

        # Load in priority order (later = lower priority with override=False):
        # 1. repo root meridian/.env
        # 2. services/.env
        for env_path in [
            Path(__file__).parents[3] / ".env",
            Path(__file__).parents[3] / "services" / ".env",
        ]:
            if env_path.exists():
                load_dotenv(env_path, override=False)
    except ImportError:
        pass

    required = ("JIRA_URL", "JIRA_EMAIL", "JIRA_API_TOKEN")
    missing = [k for k in required if not os.environ.get(k)]
    if missing:
        raise JiraConfigError(
            f"Jira REST API needs {missing} in env "
            f"(set them in services/.env or the process environment)"
        )

    return {
        "JIRA_URL":       os.environ["JIRA_URL"].rstrip("/"),
        "JIRA_EMAIL":     os.environ["JIRA_EMAIL"],
        "JIRA_API_TOKEN": os.environ["JIRA_API_TOKEN"],
        "JIRA_CLOUD_ID":  os.environ.get(
            "JIRA_CLOUD_ID", "09073cd4-e793-45ce-a66d-cc9bb1963ca2"
        ),
    }


def _build_adf_comment(text: str) -> dict[str, Any]:
    """Wrap plain text in Atlassian Document Format (ADF) for Jira Cloud."""
    return {
        "type": "doc",
        "version": 1,
        "content": [
            {
                "type": "paragraph",
                "content": [{"type": "text", "text": text}],
            }
        ],
    }


def _post_to_jira_api(
    issue_key: str,
    payload: dict[str, Any],
    env: dict[str, str],
) -> dict[str, Any]:
    """POST a worklog payload to the Jira REST API.

    Endpoint: POST {JIRA_URL}/rest/api/3/issue/{issue_key}/worklog

    Args:
        issue_key: Jira issue key, e.g. "KAN-64".
        payload: Pre-built JSON body (timeSpent, started, comment in ADF).
        env: Validated env dict from `_load_env()`.

    Returns:
        Parsed JSON response body from Jira (contains `id` for the new worklog).

    Raises:
        JiraPostError for 400, 403, 404, 5xx and other HTTP errors.
        JiraPostError with retry_after info for 429.
    """
    url = f"{env['JIRA_URL']}/rest/api/3/issue/{issue_key}/worklog"

    credentials = f"{env['JIRA_EMAIL']}:{env['JIRA_API_TOKEN']}"
    auth_header = "Basic " + base64.b64encode(credentials.encode()).decode()

    body = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=body,
        method="POST",
        headers={
            "Authorization": auth_header,
            "Content-Type": "application/json",
            "Accept": "application/json",
        },
    )

    try:
        with urllib.request.urlopen(req) as resp:
            response_body = resp.read().decode("utf-8")
            return json.loads(response_body)

    except urllib.error.HTTPError as exc:
        status = exc.code
        try:
            error_body = exc.read().decode("utf-8")
        except Exception:  # noqa: BLE001
            error_body = "(unreadable)"

        if status == 400:
            raise JiraPostError(
                f"Jira rejected worklog (400 Bad Request) for {issue_key}: {error_body}"
            ) from exc
        if status == 403:
            raise JiraPostError(
                f"Jira denied worklog permission (403 Forbidden) for {issue_key}: {error_body}"
            ) from exc
        if status == 404:
            raise JiraPostError(
                f"Jira issue not found (404) for key {issue_key!r}: {error_body}"
            ) from exc
        if status == 429:
            retry_after = exc.headers.get("Retry-After", "unknown")
            raise JiraPostError(
                f"Jira rate limit hit (429). Retry-After: {retry_after}s. "
                f"issue={issue_key}"
            ) from exc
        if status >= 500:
            raise JiraPostError(
                f"Jira server error ({status}) for {issue_key}: {error_body}"
            ) from exc

        raise JiraPostError(
            f"Jira API returned unexpected status {status} for {issue_key}: {error_body}"
        ) from exc

    except urllib.error.URLError as exc:
        raise JiraPostError(
            f"Network error reaching Jira at {env['JIRA_URL']}: {exc.reason}"
        ) from exc


def _resolve_tz(explicit: Optional[str]) -> Any:
    """Pick the TZ for `started`: explicit > MERIDIAN_TZ > host > Asia/Kolkata."""
    candidates = [
        explicit,
        os.environ.get("MERIDIAN_TZ"),
    ]
    for name in candidates:
        if name:
            try:
                return ZoneInfo(name)
            except Exception as exc:  # noqa: BLE001 — invalid TZ string
                log.warning("invalid TZ %r: %s", name, exc)

    host_tz = datetime.now().astimezone().tzinfo
    if host_tz is not None:
        return host_tz  # type: ignore[return-value]

    return ZoneInfo("Asia/Kolkata")
