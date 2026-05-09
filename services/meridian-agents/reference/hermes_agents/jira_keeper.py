"""Jira Keeper agent — syncs work context to Jira via mcp-atlassian MCP server.

Mirrors agents/watcher.py exactly:
  MCPBridge connects to `uvx mcp-atlassian` (stdio) the same way watcher connects
  to `npx screenpipe-mcp`. AIAgent calls Jira tools directly through the bridge.
  mark_sync_complete is a Python-side tool that updates state files.

Requires in .env:  JIRA_URL, JIRA_EMAIL, JIRA_API_TOKEN
"""
import asyncio
import json
import logging
import os
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

from agents.config import (
    MODEL, BASE_URL, API_KEY,
    JIRA_STATE_FILE, CURRENT_CONTEXT_FILE, JIRA_DIR,
    CONFIDENCE_THRESHOLD,
    load_skill,
)

log = logging.getLogger("jira_keeper")

for _noisy in ["httpx", "httpcore", "openai._base_client", "mcp"]:
    logging.getLogger(_noisy).setLevel(logging.WARNING)


_MARK_SYNC_COMPLETE_SCHEMA = {
    "type": "function",
    "function": {
        "name": "mark_sync_complete",
        "description": "Finalise the sync cycle — updates jira_state.json and clears trigger_jira_sync.",
        "parameters": {
            "type": "object",
            "properties": {
                "actions_taken": {"type": "array", "items": {"type": "string"}},
                "jira_key":      {"type": ["string", "null"]},
                "skipped":       {"type": "boolean"},
                "skip_reason":   {"type": ["string", "null"]},
            },
            "required": ["actions_taken", "skipped"],
        },
    },
}


# ── MCP bridge (Jira via mcp-atlassian) ───────────────────────────────────────
class _MCPBridge:
    """Synchronous façade over mcp-atlassian — same pattern as watcher's screenpipe bridge."""

    def __init__(self):
        self._tool_objs: dict = {}
        self._jira_url   = os.environ.get("JIRA_URL", "")
        self._jira_email = os.environ.get("JIRA_EMAIL", "")
        self._jira_token = os.environ.get("JIRA_API_TOKEN", "")

        if not all([self._jira_url, self._jira_email, self._jira_token]):
            missing = [k for k, v in [
                ("JIRA_URL", self._jira_url),
                ("JIRA_EMAIL", self._jira_email),
                ("JIRA_API_TOKEN", self._jira_token),
            ] if not v]
            raise RuntimeError(f"Missing Jira credentials in .env: {', '.join(missing)}")

        log.info("Connecting to mcp-atlassian...")
        asyncio.run(self._discover())
        log.info("Jira MCP tools available: %s", list(self._tool_objs.keys()))

    def _server_params(self):
        from mcp import StdioServerParameters
        return StdioServerParameters(
            command="uvx",
            args=["mcp-atlassian", "--jira-url", self._jira_url],
            env={
                **os.environ,
                "JIRA_URL":       self._jira_url,
                "JIRA_USERNAME":  self._jira_email,
                "JIRA_API_TOKEN": self._jira_token,
            },
        )

    async def _discover(self):
        from mcp.client.stdio import stdio_client
        from mcp import ClientSession
        async with stdio_client(self._server_params()) as (r, w):
            async with ClientSession(r, w) as session:
                await session.initialize()
                resp = await session.list_tools()
                for t in resp.tools:
                    self._tool_objs[t.name] = t

    @property
    def tool_names(self) -> set:
        return set(self._tool_objs.keys())

    def get_schemas(self) -> list:
        schemas = []
        for t in self._tool_objs.values():
            schema = dict(t.inputSchema) if hasattr(t, "inputSchema") and t.inputSchema else {}
            schemas.append({
                "type": "function",
                "function": {
                    "name":        t.name,
                    "description": t.description or "",
                    "parameters":  schema or {"type": "object", "properties": {}},
                },
            })
        return schemas

    def call_tool(self, name: str, args: dict) -> str:
        log.info("  → tool call: %s(%s)", name, json.dumps(args)[:120])
        result = asyncio.run(self._call_async(name, args))
        log.info("  ← %s: %d chars returned", name, len(result))
        log.debug("     %s", result[:400])
        return result

    async def _call_async(self, name: str, args: dict) -> str:
        from mcp.client.stdio import stdio_client
        from mcp import ClientSession
        async with stdio_client(self._server_params()) as (r, w):
            async with ClientSession(r, w) as session:
                await session.initialize()
                result = await session.call_tool(name, args)
                if result.content:
                    raw = (
                        result.content[0].text
                        if hasattr(result.content[0], "text")
                        else str(result.content[0])
                    )
                    return raw or "{}"
        return "{}"


# ── Python-side state bridge ───────────────────────────────────────────────────
class _StateBridge:
    """Executes mark_sync_complete — updates jira_state.json and current_context.json."""

    def __init__(self):
        self._now: datetime | None = None
        self._current_ctx: dict = {}
        self.sync_result: dict = {}

    def init_cycle(self, now: datetime, current_ctx: dict):
        self._now = now
        self._current_ctx = current_ctx
        self.sync_result = {}

    @property
    def tool_names(self) -> set:
        return {"mark_sync_complete"}

    def call_tool(self, name: str, args: dict) -> str:
        log.info("  state tool: %s args=%s", name, json.dumps(args)[:300])
        if name == "mark_sync_complete":
            return self._mark_sync_complete(args)
        return f"Unknown tool: {name}"

    def _mark_sync_complete(self, args: dict) -> str:
        if "actions_taken" not in args or "skipped" not in args:
            missing = [k for k in ("actions_taken", "skipped") if k not in args]
            msg = f"ERROR: mark_sync_complete missing required field(s): {missing}. Retry with actions_taken and skipped set."
            log.warning("  %s", msg)
            return msg
        skipped  = args.get("skipped", False)
        actions  = args.get("actions_taken", [])
        jira_key = args.get("jira_key")

        self.sync_result = {
            "status":   "skipped" if skipped else "completed",
            "jira_key": jira_key,
            "actions":  actions,
        }

        JIRA_DIR.mkdir(parents=True, exist_ok=True)
        state = _load_json(JIRA_STATE_FILE, {"tickets": {}, "last_sync": None})
        state["last_sync"] = self._now.isoformat()
        if jira_key and not skipped:
            ticket = state["tickets"].setdefault(jira_key, {"syncs": 0})
            ticket["last_sync"]   = self._now.isoformat()
            ticket["syncs"]       = ticket.get("syncs", 0) + 1
            ticket["last_action"] = actions[-1] if actions else ""
        JIRA_STATE_FILE.write_text(json.dumps(state, indent=2))

        ctx = dict(self._current_ctx)
        ctx["trigger_jira_sync"] = False
        ctx["last_synced"]       = self._now.isoformat()
        CURRENT_CONTEXT_FILE.write_text(json.dumps(ctx, indent=2))

        summary = "skipped" if skipped else f"{len(actions)} action(s): {'; '.join(actions)}"
        log.info("  sync finalised — %s", summary)
        return f"Sync complete: {summary}"


# Module-level singletons
_mcp_bridge:   _MCPBridge | None = None
_state_bridge: _StateBridge | None = None
_patched: bool = False


def _ensure_bridges_and_patch():
    global _mcp_bridge, _state_bridge, _patched
    if _mcp_bridge is None:
        _mcp_bridge = _MCPBridge()
    if _state_bridge is None:
        _state_bridge = _StateBridge()
    if not _patched:
        _patch_tools(_mcp_bridge, _state_bridge)
        _patched = True


def _patch_tools(mcp_bridge: _MCPBridge, state_bridge: _StateBridge):
    repo_root = Path(__file__).parent.parent
    if str(repo_root) not in sys.path:
        sys.path.insert(0, str(repo_root))

    import run_agent as _ra
    _original = _ra.handle_function_call
    all_mcp_names = mcp_bridge.tool_names

    def _patched_handler(function_name, function_args, *args, **kwargs):
        if function_name in all_mcp_names:
            return mcp_bridge.call_tool(function_name, function_args)
        if function_name in state_bridge.tool_names:
            return state_bridge.call_tool(function_name, function_args)
        return _original(function_name, function_args, *args, **kwargs)

    _ra.handle_function_call = _patched_handler
    log.debug("Patched run_agent.handle_function_call for Jira MCP + state tools")


def _load_json(path: Path, default) -> dict:
    if path.exists():
        try:
            return json.loads(path.read_text())
        except json.JSONDecodeError:
            pass
    return default


# ── Public entry point ─────────────────────────────────────────────────────────
def run_jira_keeper() -> dict:
    from run_agent import AIAgent

    _ensure_bridges_and_patch()

    now = datetime.now(timezone.utc)
    log.info("=" * 60)
    log.info("Jira Keeper cycle — %s", now.strftime("%Y-%m-%d %H:%M:%S UTC"))
    log.info("Model: %s | Endpoint: %s", MODEL, BASE_URL)

    current_ctx = _load_json(CURRENT_CONTEXT_FILE, {})
    if not current_ctx:
        log.warning("current_context.json not found — nothing to sync")
        return {"status": "skipped", "reason": "no context"}

    confidence = current_ctx.get("confidence", 0.0)
    if not current_ctx.get("trigger_jira_sync") or confidence < CONFIDENCE_THRESHOLD:
        log.info("Skipping — trigger_jira_sync=%s confidence=%.2f",
                 current_ctx.get("trigger_jira_sync"), confidence)
        return {"status": "skipped", "reason": "threshold not met"}

    jira_state = _load_json(JIRA_STATE_FILE, {"tickets": {}, "last_sync": None})
    _state_bridge.init_cycle(now, current_ctx)

    user_message = (
        f"Current time: {now.isoformat()}\n\n"
        f"CURRENT CONTEXT:\n{json.dumps(current_ctx, indent=2)}\n\n"
        f"JIRA STATE:\n{json.dumps(jira_state, indent=2)}\n\n"
        "Sync Jira, then call mark_sync_complete."
    )

    agent = AIAgent(
        model=MODEL,
        base_url=BASE_URL,
        api_key=API_KEY or "none",
        ephemeral_system_prompt=load_skill("jira-keeper"),
        enabled_toolsets=[],
        quiet_mode=True,
        skip_context_files=True,
        load_soul_identity=True,
        max_iterations=12,
    )
    agent.tools = (agent.tools or []) + _mcp_bridge.get_schemas() + [_MARK_SYNC_COMPLETE_SCHEMA]
    agent.valid_tool_names |= _mcp_bridge.tool_names | _state_bridge.tool_names

    t0 = time.time()
    try:
        agent.run_conversation(user_message)
    except Exception as exc:
        log.error("Jira Keeper error: %s", exc, exc_info=True)

    elapsed = time.time() - t0
    result = _state_bridge.sync_result or {"status": "error", "reason": "mark_sync_complete not called"}

    log.info("─" * 40)
    log.info("status:   %s", result.get("status"))
    log.info("jira_key: %s", result.get("jira_key"))
    log.info("actions:  %s", result.get("actions", []))
    log.info("elapsed:  %.1fs", elapsed)

    return result


if __name__ == "__main__":
    import json as _json
    logging.basicConfig(
        level=logging.DEBUG,
        format="%(asctime)s [%(levelname)-8s] %(message)s",
        datefmt="%H:%M:%S",
    )
    print(_json.dumps(run_jira_keeper(), indent=2))
