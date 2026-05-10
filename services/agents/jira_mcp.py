"""Jira fallback fetcher — hits the Atlassian MCP (mcp-atlassian via uvx)
to pull the user's open issues when the local pm_tasks table is empty.

The Rust daemon owns the long-term cache (intelligence/providers/jira.rs writes
into pm_tasks every 30 min); this module only kicks in when that cache hasn't
been populated yet — typically on first boot, or when the user runs the
synthesizer before configuring the Rust-side Jira credentials.

Usage:
    from agents.jira_mcp import fetch_open_tasks
    tasks = fetch_open_tasks()  # [{ task_key, title, ... }, ...]

Requires JIRA_URL, JIRA_EMAIL, JIRA_API_TOKEN in the environment.
"""
from __future__ import annotations

import asyncio
import json
import logging
import os
import re
from typing import Any

log = logging.getLogger("agents.jira_mcp")


# ── Helpers ────────────────────────────────────────────────────────────────────
def _adf_to_text(node: Any) -> str:
    """Flatten an ADF document into plain text. Best-effort, no formatting."""
    if not node:
        return ""
    if isinstance(node, str):
        return node
    if isinstance(node, dict):
        if node.get("type") == "text":
            return node.get("text", "")
        return "".join(_adf_to_text(c) for c in node.get("content", []))
    if isinstance(node, list):
        return "".join(_adf_to_text(c) for c in node)
    return ""


def _normalise(issue: dict, base_url: str) -> dict:
    fields = issue.get("fields", {}) or {}
    status = fields.get("status") or {}
    cat = (status.get("statusCategory") or {}).get("key") or "new"
    return {
        "task_key":         issue.get("key", ""),
        "provider":         "jira",
        "title":            fields.get("summary") or "",
        "description_text": _adf_to_text(fields.get("description")),
        "status":           status.get("name") or "",
        "status_category":  cat,
        "issue_type":       (fields.get("issuetype") or {}).get("name") or "",
        "project_key":      (fields.get("project") or {}).get("key") or "",
        "url":              f"{base_url.rstrip('/')}/browse/{issue.get('key', '')}",
        "updated_at":       fields.get("updated") or "",
    }


# ── MCP plumbing ───────────────────────────────────────────────────────────────
class _AtlassianMCP:
    """Thin wrapper around `uvx mcp-atlassian`. One stdio connection per call."""

    def __init__(self):
        self.url   = os.environ.get("JIRA_URL", "")
        self.email = os.environ.get("JIRA_EMAIL", "")
        self.token = os.environ.get("JIRA_API_TOKEN", "")
        if not (self.url and self.email and self.token):
            missing = [k for k, v in (
                ("JIRA_URL", self.url),
                ("JIRA_EMAIL", self.email),
                ("JIRA_API_TOKEN", self.token),
            ) if not v]
            raise RuntimeError(
                f"Jira MCP fallback unavailable — missing env: {', '.join(missing)}"
            )

    def _server_params(self):
        from mcp import StdioServerParameters
        return StdioServerParameters(
            command="uvx",
            args=["mcp-atlassian", "--jira-url", self.url],
            env={
                **os.environ,
                "JIRA_URL":       self.url,
                "JIRA_USERNAME":  self.email,
                "JIRA_API_TOKEN": self.token,
            },
        )

    async def _call(self, tool: str, args: dict) -> str:
        from mcp.client.stdio import stdio_client
        from mcp import ClientSession
        async with stdio_client(self._server_params()) as (r, w):
            async with ClientSession(r, w) as session:
                await session.initialize()
                result = await session.call_tool(tool, args)
                if not result.content:
                    return "{}"
                first = result.content[0]
                return getattr(first, "text", str(first)) or "{}"

    def search(self, jql: str, limit: int = 50) -> list[dict]:
        log.info("jira fallback: JQL %s (limit=%d)", jql, limit)
        raw = asyncio.run(self._call(
            "jira_search",
            {"jql": jql, "limit": limit},
        ))
        return _parse_search_result(raw)


def _parse_search_result(raw: str) -> list[dict]:
    """mcp-atlassian's jira_search returns a JSON object; tolerate a few shapes."""
    try:
        data = json.loads(raw)
    except (json.JSONDecodeError, TypeError):
        # If the server wrapped JSON inside text, peel a fenced block.
        match = re.search(r"\{.*\}|\[.*\]", raw, re.DOTALL)
        if not match:
            log.warning("jira fallback: response was not JSON: %s", raw[:200])
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
    log.warning("jira fallback: unexpected response shape: %s", str(data)[:200])
    return []


# ── Public API ─────────────────────────────────────────────────────────────────
DEFAULT_JQL = "assignee = currentUser() AND statusCategory != Done ORDER BY updated DESC"


def fetch_open_tasks(jql: str = DEFAULT_JQL, limit: int = 50) -> list[dict]:
    """Pull open Jira tasks from the Atlassian MCP. Returns rows shaped for
    db.upsert_pm_task / direct insertion into the pm_tasks table."""
    mcp = _AtlassianMCP()
    issues = mcp.search(jql, limit=limit)
    return [_normalise(i, mcp.url) for i in issues]


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(message)s")
    rows = fetch_open_tasks()
    print(json.dumps(rows, indent=2))
