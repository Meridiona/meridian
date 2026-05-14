"""Jira progress updater — posts timed activity summaries to in-progress Jira issues.

Fetches in-progress tickets via mcp-atlassian, pulls session data for each ticket
from the local Meridian MCP server, generates a bullet-point summary via hermes,
and posts the comment back to Jira. All updates are logged in jira_update_log for
idempotent deduplication per (task_key, period_start, period_end) slot.
"""
from __future__ import annotations

import asyncio
import json
import logging
import os
import re
import sqlite3
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

from dotenv import load_dotenv

from agents import db
from agents._hermes_setup import ensure_hermes_importable
from agents.config import (
    API_KEY,
    BASE_URL,
    JIRA_POST_NO_ACTIVITY,
    MERIDIAN_DB,
    MERIDIAN_MCP_PATH,
    MODEL,
    REPO_ROOT,
    load_skill,
)

log = logging.getLogger("agents.jira_updater")

_IN_PROGRESS_JQL = (
    "assignee = currentUser() AND statusCategory = indeterminate ORDER BY updated DESC"
)


# ── Result type ────────────────────────────────────────────────────────────────
@dataclass
class UpdateResult:
    task_key: str
    had_activity: bool
    duration_s: int
    session_count: int
    comment_body: str
    comment_id: str | None
    state: str  # 'sent' | 'failed' | 'skipped' | 'dry_run' | 'no_activity_skipped'
    error: str | None = None


# ── Atlassian MCP client ───────────────────────────────────────────────────────
class _AtlassianMCPClient:
    """Wraps uvx mcp-atlassian. Holds one subprocess open for the lifetime of
    the object so the FastMCP banner only prints once per run_update call."""

    def __init__(self) -> None:
        self.url = os.environ.get("JIRA_URL", "")
        self.email = os.environ.get("JIRA_EMAIL", "")
        self.token = os.environ.get("JIRA_API_TOKEN", "")
        missing = [
            k
            for k, v in (
                ("JIRA_URL", self.url),
                ("JIRA_EMAIL", self.email),
                ("JIRA_API_TOKEN", self.token),
            )
            if not v
        ]
        if missing:
            raise RuntimeError(
                f"Atlassian MCP unavailable — missing env: {', '.join(missing)}"
            )

    def _server_params(self):
        from mcp import StdioServerParameters

        return StdioServerParameters(
            command="uvx",
            args=["mcp-atlassian", "--jira-url", self.url],
            env={
                **os.environ,
                "JIRA_URL": self.url,
                "JIRA_USERNAME": self.email,
                "JIRA_API_TOKEN": self.token,
                # Suppress FastMCP startup banner and toolset warning.
                "FASTMCP_BANNER": "0",
                "TOOLSETS": "all",
            },
        )

    async def _run(self, calls: list[tuple[str, dict]]) -> list[str]:
        """Open one subprocess and execute all calls sequentially."""
        from mcp import ClientSession
        from mcp.client.stdio import stdio_client

        results = []
        async with stdio_client(self._server_params()) as (r, w):
            async with ClientSession(r, w) as session:
                await session.initialize()
                for tool, args in calls:
                    result = await session.call_tool(tool, args)
                    if not result.content:
                        results.append("{}")
                        continue
                    first = result.content[0]
                    results.append(getattr(first, "text", str(first)) or "{}")
        return results

    def fetch_in_progress(self) -> list[dict]:
        log.info("fetching in-progress tasks (JQL: %s)", _IN_PROGRESS_JQL)
        raw = asyncio.run(
            self._run([("jira_search", {"jql": _IN_PROGRESS_JQL, "limit": 50})])
        )[0]
        issues = _parse_search_result(raw)
        return [_normalise_issue(i, self.url) for i in issues]

    def add_comment(self, task_key: str, body: str) -> str:
        log.info("posting comment to %s", task_key)
        raw = asyncio.run(
            self._run([("jira_add_comment", {"issue_key": task_key, "body": body})])
        )[0]
        try:
            data = json.loads(raw)
            if isinstance(data, dict):
                return str(data.get("id") or data.get("comment_id") or "")
        except (json.JSONDecodeError, TypeError):
            pass
        return ""


def _parse_search_result(raw: str) -> list[dict]:
    try:
        data = json.loads(raw)
    except (json.JSONDecodeError, TypeError):
        match = re.search(r"\{.*\}|\[.*\]", raw, re.DOTALL)
        if not match:
            log.warning("atlassian mcp: response was not JSON: %s", raw[:200])
            return []
        try:
            data = json.loads(match.group())
        except json.JSONDecodeError:
            return []
    if isinstance(data, list):
        return data
    if isinstance(data, dict):
        for key in ("issues", "results", "items"):
            if isinstance(data.get(key), list):
                return data[key]
    log.warning("atlassian mcp: unexpected response shape: %s", str(data)[:200])
    return []


def _normalise_issue(issue: dict, base_url: str) -> dict:
    # mcp-atlassian returns a flat structure (key, summary, status at top level)
    # not the nested Jira REST API format (fields.summary, fields.status.name).
    status = issue.get("status") or {}
    if isinstance(status, dict):
        status_name = status.get("name") or ""
    else:
        status_name = str(status)
    return {
        "task_key": issue.get("key", ""),
        "title": issue.get("summary") or "",
        "status": status_name,
        "url": f"{base_url.rstrip('/')}/browse/{issue.get('key', '')}",
    }


# ── Meridian MCP client ────────────────────────────────────────────────────────
class _MeridianMCPClient:
    """Wraps the local Node.js meridian-mcp server."""

    def __init__(self) -> None:
        self.mcp_path = Path(MERIDIAN_MCP_PATH)
        if not self.mcp_path.exists():
            raise RuntimeError(
                f"Meridian MCP not found at {self.mcp_path}. "
                "Run `npm run build` in packages/meridian-mcp/."
            )

    def _server_params(self):
        from mcp import StdioServerParameters

        return StdioServerParameters(
            command="node",
            args=[str(self.mcp_path)],
            env={**os.environ, "MERIDIAN_DB": str(MERIDIAN_DB)},
        )

    async def _call(self, tool: str, args: dict) -> str:
        from mcp import ClientSession
        from mcp.client.stdio import stdio_client

        async with stdio_client(self._server_params()) as (r, w):
            async with ClientSession(r, w) as session:
                await session.initialize()
                result = await session.call_tool(tool, args)
                if not result.content:
                    return ""
                first = result.content[0]
                return getattr(first, "text", str(first)) or ""

    def get_task_sessions(self, task_key: str, from_time: str, to_time: str) -> str:
        return asyncio.run(
            self._call(
                "get-task-sessions",
                {
                    "task_key": task_key,
                    "from_time": from_time,
                    "to_time": to_time,
                    "include_content": True,
                },
            )
        )


# ── Hermes summary generator ───────────────────────────────────────────────────
class _HermesUpdater:
    """Single-shot hermes AIAgent call — no tools, one iteration."""

    def generate(self, user_message: str) -> str:
        ensure_hermes_importable()
        # Force services/.env after hermes's own dotenv loading which may
        # override OLLAMA_* vars with stale values from ~/.hermes/.env.
        load_dotenv(REPO_ROOT / ".env", override=True)
        from run_agent import AIAgent

        model    = os.environ.get("OLLAMA_MODEL",   MODEL)
        base_url = os.environ.get("OLLAMA_HOST",    BASE_URL)
        api_key  = os.environ.get("OLLAMA_API_KEY", API_KEY)

        agent = AIAgent(
            model=model,
            base_url=base_url,
            api_key=api_key or "none",
            ephemeral_system_prompt=load_skill("jira-updater"),
            enabled_toolsets=[],
            quiet_mode=True,
            skip_context_files=True,
            skip_memory=True,
            load_soul_identity=False,
            max_iterations=1,
            max_tokens=1500,
        )
        result = agent.run_conversation(user_message)
        return result.get("final_response", "").strip()


# ── Helpers ────────────────────────────────────────────────────────────────────
_SUMMARY_RE = re.compile(
    r"(\d+)\s+session\(s\),\s*(?:(\d+)h\s*)?(\d+)m\s+total", re.IGNORECASE
)


def _parse_mcp_summary(mcp_text: str) -> tuple[bool, int, int]:
    """Returns (had_activity, session_count, duration_s)."""
    if "No sessions linked" in mcp_text:
        return False, 0, 0
    m = _SUMMARY_RE.search(mcp_text)
    if not m:
        return True, 1, 0
    count = int(m.group(1))
    hours = int(m.group(2) or 0)
    minutes = int(m.group(3))
    return True, count, hours * 3600 + minutes * 60


def _fmt_time(iso: str) -> str:
    """Parse ISO 8601 UTC and return HH:MM."""
    try:
        dt = datetime.fromisoformat(iso.replace("Z", "+00:00"))
        return dt.strftime("%H:%M")
    except ValueError:
        return iso


def _fmt_date(iso: str) -> str:
    """Parse ISO 8601 UTC and return 'Month Day' (e.g. 'May 14')."""
    try:
        dt = datetime.fromisoformat(iso.replace("Z", "+00:00"))
        return dt.strftime("%B %-d")
    except ValueError:
        return iso


def _fmt_duration(duration_s: int) -> str:
    hours, rem = divmod(duration_s, 3600)
    minutes = rem // 60
    if hours:
        return f"{hours}h {minutes}m"
    return f"{minutes}m"


def _build_user_message(
    task: dict, mcp_text: str, from_time: str, to_time: str
) -> str:
    return (
        f"TASK: {task['task_key']} — {task['title']}\n"
        f"Status: {task['status']}\n"
        f"Period: {_fmt_time(from_time)}–{_fmt_time(to_time)} UTC\n"
        "\n"
        "ACTIVITY DATA:\n"
        f"{mcp_text}\n"
        "\n"
        "Write 3–5 bullet points summarising what was accomplished. "
        "Bullet points only, no preamble."
    )


def _format_comment(
    task: dict,
    summary: str,
    from_time: str,
    to_time: str,
    duration_s: int,
    session_count: int,
) -> str:
    header = (
        f"📊 *Progress Update* — {_fmt_time(from_time)}–{_fmt_time(to_time)}, "
        f"{_fmt_date(from_time)}"
    )
    if duration_s == 0 and session_count == 0:
        return f"{header}\n\nNo activity recorded in this period."
    footer = (
        f"_⏱ {_fmt_duration(duration_s)} active · "
        f"{session_count} session(s) · Via Meridian_"
    )
    return f"{header}\n\n{summary}\n\n{footer}"


# ── Public entry point ─────────────────────────────────────────────────────────
def run_update(
    from_time: str,
    to_time: str,
    task_filter: str | None = None,
    dry_run: bool = False,
) -> list[UpdateResult]:
    """Main entry point — safe to call from a thread (via run_in_executor)."""
    conn = sqlite3.connect(str(MERIDIAN_DB))
    conn.row_factory = sqlite3.Row
    try:
        jira = _AtlassianMCPClient()
        meridian = _MeridianMCPClient()

        tasks = jira.fetch_in_progress()
        log.info("found %d in-progress task(s): %s", len(tasks), [t["task_key"] for t in tasks])
        if task_filter:
            tasks = [t for t in tasks if t["task_key"] == task_filter]
            if not tasks:
                log.warning(
                    "task %s not found in in-progress tasks — check its status in Jira",
                    task_filter,
                )
                return []
        if not tasks:
            log.info("no in-progress tasks found")
            return []

        results: list[UpdateResult] = []
        for task in tasks:
            result = _process_task(
                conn, jira, meridian, task, task["task_key"], from_time, to_time, dry_run
            )
            results.append(result)
        return results
    finally:
        conn.close()


def _process_task(
    conn: sqlite3.Connection,
    jira: _AtlassianMCPClient,
    meridian: _MeridianMCPClient,
    task: dict,
    task_key: str,
    from_time: str,
    to_time: str,
    dry_run: bool,
) -> UpdateResult:
    if db.get_last_update(conn, task_key, from_time, to_time):
        log.info("skipping %s: already posted for this slot", task_key)
        return UpdateResult(
            task_key=task_key, had_activity=False, duration_s=0,
            session_count=0, comment_body="", comment_id=None, state="skipped",
        )

    mcp_text = meridian.get_task_sessions(task_key, from_time, to_time)
    had_activity, session_count, duration_s = _parse_mcp_summary(mcp_text)

    if had_activity:
        user_msg = _build_user_message(task, mcp_text, from_time, to_time)
        try:
            summary = _HermesUpdater().generate(user_msg)
        except Exception as exc:
            log.error("hermes failed for %s: %s", task_key, exc)
            summary = "Could not generate summary."
    else:
        summary = "No activity recorded in this period."

    if not had_activity and not JIRA_POST_NO_ACTIVITY:
        return UpdateResult(
            task_key=task_key, had_activity=False, duration_s=0,
            session_count=0, comment_body="", comment_id=None,
            state="no_activity_skipped",
        )

    comment_body = _format_comment(
        task, summary, from_time, to_time, duration_s, session_count
    )
    update_id = db.log_jira_update(
        conn,
        task_key=task_key,
        period_start=from_time,
        period_end=to_time,
        session_count=session_count,
        duration_s=duration_s,
        had_activity=had_activity,
        comment_body=comment_body,
    )

    if dry_run:
        print(f"\n{'=' * 60}\n{task_key}\n{'=' * 60}\n{comment_body}\n")
        return UpdateResult(
            task_key=task_key, had_activity=had_activity, duration_s=duration_s,
            session_count=session_count, comment_body=comment_body,
            comment_id=None, state="dry_run",
        )

    try:
        comment_id = jira.add_comment(task_key, comment_body)
        db.mark_update_sent(conn, update_id, comment_id)
        log.info("posted update for %s (comment_id=%s)", task_key, comment_id)
        return UpdateResult(
            task_key=task_key, had_activity=had_activity, duration_s=duration_s,
            session_count=session_count, comment_body=comment_body,
            comment_id=comment_id, state="sent",
        )
    except Exception as exc:
        db.mark_update_failed(conn, update_id, str(exc))
        log.error("failed to post for %s: %s", task_key, exc)
        return UpdateResult(
            task_key=task_key, had_activity=had_activity, duration_s=duration_s,
            session_count=session_count, comment_body=comment_body,
            comment_id=None, state="failed", error=str(exc),
        )


__all__ = ["UpdateResult", "run_update"]
