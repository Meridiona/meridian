"""Jira worklog poster — phase 1.

Single responsibility: turn a (task_key, time_spent_seconds, started_utc,
comment) into a successful `jira_add_worklog` call on the user's Jira
instance via the `mcp-atlassian` MCP server, returning the new worklog id.

Why MCP and not a direct REST call?

  * Auth + URL are already wired into the user's `services/.env` for
    mcp-atlassian; we'd otherwise duplicate that config.
  * mcp-atlassian's `jira_add_worklog` handles the Atlassian Document
    Format conversion of the Markdown comment for us — Jira Cloud
    doesn't accept plain Markdown over its raw API.
  * Future phases (comments, transitions) reuse the same boot, so the
    incremental cost is zero.

Lifecycle: one `uvx mcp-atlassian --transport=stdio` subprocess per
post. That's deliberately cheap-and-disposable rather than long-lived
— a stuck child is the worst kind of bug in a 1-hour cron, so we keep
the failure surface a single subprocess we own.
"""
from __future__ import annotations

import json
import logging
import os
import subprocess
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional
from zoneinfo import ZoneInfo

from agents.pm_update import config

log = logging.getLogger(__name__)

# Names mcp-atlassian expects in its env. Our .env uses JIRA_EMAIL for
# the human's address — mcp-atlassian wants JIRA_USERNAME. Map at boot.
_JIRA_ENV_KEYS = ("JIRA_URL", "JIRA_USERNAME", "JIRA_API_TOKEN")

# How long we'll wait for the MCP child to handshake + answer one tool call.
# 30s is generous — even a cold uvx download has finished by then.
_SUBPROCESS_TIMEOUT_S = int(os.environ.get("JIRA_MCP_TIMEOUT_S", "60"))


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
    worklog_id:        str
    issue_key:         str
    time_spent_jira:   str
    time_spent_seconds: int
    started_iso:       str
    raw_response:      dict[str, Any]


def post_worklog(
    *,
    task_key: str,
    time_spent_seconds: int,
    started_utc: datetime,
    comment: Optional[str] = None,
    timezone_name: Optional[str] = None,
) -> WorklogPostResult:
    """Post one worklog entry to Jira.

    Args:
        task_key: Jira issue key, e.g. "KAN-64". Validated by the MCP
            server with a `^[A-Z][A-Z0-9_]+-\\d+$` pattern.
        time_spent_seconds: Real (idle-discounted) seconds for this
            window. Must be >= 60 — Jira rejects shorter entries.
        started_utc: Window-start moment in UTC. Will be rendered to
            the local TZ in the Jira-expected format.
        comment: Optional Markdown comment (≤ a few sentences in phase
            1 — the worklog tab in Jira shows this in-line).
        timezone_name: Override for the started-time TZ. Defaults to
            `MERIDIAN_TZ` env var, then the host's `datetime.now()`
            tzinfo, then `Asia/Kolkata`.

    Returns:
        `WorklogPostResult` with the new worklog id.

    Raises:
        JiraConfigError on missing creds.
        JiraPostError on any MCP / Jira API failure.
    """
    if time_spent_seconds < 60:
        raise JiraPostError(
            f"time_spent_seconds={time_spent_seconds} below Jira's 60s minimum"
        )

    started_local = render_started_local(started_utc, tz_name=timezone_name)
    jira_time = seconds_to_jira_time(time_spent_seconds)

    env = _build_env()
    args: dict[str, Any] = {
        "issue_key":  task_key,
        "time_spent": jira_time,
        "started":    started_local,
    }
    if comment:
        args["comment"] = comment

    log.info(
        "jira_add_worklog: task=%s time_spent=%s started=%s comment_len=%d",
        task_key, jira_time, started_local, len(comment or ""),
    )
    raw = _call_mcp_tool("jira_add_worklog", args, env=env)
    worklog_id = _extract_worklog_id(raw)
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

    Jira accepts a small grammar: `Nw`, `Nd`, `Nh`, `Nm`. We emit
    hours+minutes because PM updates land at sub-day granularity.
    Anything under a minute rounds up — Jira rejects fractional minutes
    on the worklog API.
    """
    if seconds < 60:
        raise ValueError(f"seconds must be ≥ 60 for Jira worklog (got {seconds})")
    minutes_total = (seconds + 30) // 60        # round-to-nearest
    hours, minutes = divmod(minutes_total, 60)
    if hours and minutes:
        return f"{hours}h {minutes}m"
    if hours:
        return f"{hours}h"
    return f"{minutes}m"


def render_started_local(when_utc: datetime, *, tz_name: Optional[str] = None) -> str:
    """Render a UTC moment in the Jira worklog `started` format.

    Format: `YYYY-MM-DDTHH:MM:SS.mmm+ZZZZ` — milliseconds + ±HHMM offset,
    no colon in the offset. This is what mcp-atlassian's example string
    uses (`2023-08-01T12:00:00.000+0000`) and what the underlying Jira
    REST API expects.
    """
    if when_utc.tzinfo is None:
        when_utc = when_utc.replace(tzinfo=timezone.utc)

    tz = _resolve_tz(tz_name)
    local = when_utc.astimezone(tz)
    millis = f"{local.microsecond // 1000:03d}"
    # `%z` gives `+HHMM` on all modern Python versions — no colon, good.
    return local.strftime(f"%Y-%m-%dT%H:%M:%S.{millis}%z")


# ──────────────────────── Internals ────────────────────────────────────────────


def _resolve_tz(explicit: Optional[str]) -> ZoneInfo:
    """Pick the TZ for `started`: explicit > MERIDIAN_TZ > host > Asia/Kolkata."""
    candidates = [
        explicit,
        os.environ.get("MERIDIAN_TZ"),
    ]
    for name in candidates:
        if name:
            try:
                return ZoneInfo(name)
            except Exception as exc:                    # noqa: BLE001 — invalid TZ string
                log.warning("invalid TZ %r: %s", name, exc)

    host_tz = datetime.now().astimezone().tzinfo
    if host_tz is not None:
        # tzinfo from astimezone() is a fixed offset on some platforms;
        # but it always has tzname() so this is enough for strftime("%z").
        return host_tz                                  # type: ignore[return-value]

    return ZoneInfo("Asia/Kolkata")


def _build_env() -> dict[str, str]:
    """Inherit the process env, then re-key JIRA_EMAIL → JIRA_USERNAME.

    mcp-atlassian fails loudly if the trio isn't set, so we pre-check
    and raise a clearer error before spawning.
    """
    env = os.environ.copy()
    if "JIRA_EMAIL" in env and "JIRA_USERNAME" not in env:
        env["JIRA_USERNAME"] = env["JIRA_EMAIL"]

    missing = [k for k in _JIRA_ENV_KEYS if not env.get(k)]
    if missing:
        raise JiraConfigError(
            f"mcp-atlassian needs {missing} in env "
            f"(services/.env defines JIRA_URL/JIRA_EMAIL/JIRA_API_TOKEN)"
        )
    return env


def _call_mcp_tool(
    name: str,
    arguments: dict[str, Any],
    *,
    env: dict[str, str],
) -> dict[str, Any]:
    """Run one MCP tool call and return its `result` payload.

    Boots `uvx mcp-atlassian --transport=stdio`, performs the
    initialize handshake, calls the tool, then terminates the child.
    Exceptions surface to the caller — never silently retried.
    """
    cmd = ["uvx", "mcp-atlassian", "--transport=stdio"]
    proc = subprocess.Popen(
        cmd,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env,
    )
    try:
        _send(proc, {
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities":     {},
                "clientInfo":       {"name": "meridian-pm-update", "version": "1.0"},
            },
        })
        _recv(proc)                                     # init reply (discarded)
        _send(proc, {"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})

        _send(proc, {
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        })
        reply = _recv(proc)
    except subprocess.TimeoutExpired as exc:
        raise JiraPostError(f"mcp-atlassian timed out: {exc}") from exc
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()

    if "error" in reply:
        raise JiraPostError(f"jira_add_worklog returned error: {reply['error']}")

    result = reply.get("result")
    if not result:
        raise JiraPostError(f"jira_add_worklog returned no result: {reply}")

    # Tools return content as a list of typed parts. We always expect a
    # single text/JSON blob from jira_add_worklog.
    return _flatten_tool_result(result)


def _send(proc: subprocess.Popen, msg: dict[str, Any]) -> None:
    if proc.stdin is None:
        raise JiraPostError("mcp-atlassian subprocess has no stdin")
    proc.stdin.write(json.dumps(msg) + "\n")
    proc.stdin.flush()


def _recv(proc: subprocess.Popen) -> dict[str, Any]:
    """Read one JSON-RPC reply, bounded by SUBPROCESS_TIMEOUT.

    mcp-atlassian emits banner lines to *stderr* — those don't matter
    here. stdout is strictly newline-delimited JSON-RPC frames.
    """
    if proc.stdout is None:
        raise JiraPostError("mcp-atlassian subprocess has no stdout")

    # Python's readline has no timeout. Use communicate-like polling
    # via select to bound the wait per frame.
    import select
    deadline = _SUBPROCESS_TIMEOUT_S
    buf: list[str] = []
    while True:
        ready, _, _ = select.select([proc.stdout], [], [], deadline)
        if not ready:
            raise subprocess.TimeoutExpired(proc.args, deadline)
        chunk = proc.stdout.readline()
        if not chunk:
            stderr = proc.stderr.read() if proc.stderr else ""
            raise JiraPostError(f"mcp-atlassian closed stdout — stderr: {stderr[:500]}")
        chunk = chunk.strip()
        if not chunk:
            continue
        try:
            return json.loads(chunk)
        except json.JSONDecodeError:
            # Some MCP servers leak a banner line through stdout; tolerate.
            buf.append(chunk)
            if len(buf) > 5:
                raise JiraPostError(f"mcp-atlassian sent non-JSON: {buf!r}")


def _flatten_tool_result(result: dict[str, Any]) -> dict[str, Any]:
    """Pull the structured JSON out of a tools/call result envelope.

    MCP tools return `{"content": [{"type":"text","text": "<json>"}, ...]}`.
    `jira_add_worklog` always returns a single JSON-encoded text block.
    """
    content = result.get("content") or []
    for part in content:
        if part.get("type") == "text" and "text" in part:
            try:
                return json.loads(part["text"])
            except json.JSONDecodeError:
                # Server returned plain text — pass it back verbatim.
                return {"_raw_text": part["text"]}
    # Result with no content list — return the raw envelope.
    return result


def _extract_worklog_id(raw: dict[str, Any]) -> str:
    """Find the worklog id in mcp-atlassian's response.

    The server returns the Jira REST envelope shape. We probe the few
    locations Jira's API uses for the id, fail loudly if none match.
    """
    for key in ("id", "worklog_id", "worklogId"):
        if key in raw and raw[key]:
            return str(raw[key])
    nested = raw.get("worklog") or raw.get("data") or {}
    for key in ("id", "worklog_id", "worklogId"):
        if key in nested and nested[key]:
            return str(nested[key])
    raise JiraPostError(f"could not find worklog id in response: {raw!r}")
