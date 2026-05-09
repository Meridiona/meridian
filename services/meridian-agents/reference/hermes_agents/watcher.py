"""Watcher agent — captures user activity via Screenpipe MCP every 3 minutes.

Uses AIAgent from run_agent.py. Screenpipe MCP tools are discovered via MCPBridge
and injected into AIAgent's tool list. handle_function_call is patched once at
module level so both sequential and concurrent execution paths route screenpipe
calls to the MCP session.
"""
import asyncio
import json
import logging
import re
import sys
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path

from agents.config import (
    MODEL, BASE_URL, API_KEY,
    ACTIVITY_DIR, BUFFER_FILE, MAX_BUFFER_LINES,
    load_skill,
)

log = logging.getLogger("watcher")

for _noisy in ["httpx", "httpcore", "openai._base_client", "mcp"]:
    logging.getLogger(_noisy).setLevel(logging.WARNING)



# ── MCP bridge (initialised once per process) ──────────────────────────────────
class _MCPBridge:
    """Synchronous façade over screenpipe-mcp. Each tool call opens a fresh connection."""

    def __init__(self):
        self._tool_objs: dict = {}
        log.info("Connecting to Screenpipe MCP...")
        asyncio.run(self._discover())
        log.info("Screenpipe tools available: %s", list(self._tool_objs.keys()))

    async def _discover(self):
        from mcp import ClientSession, StdioServerParameters
        from mcp.client.stdio import stdio_client
        server = StdioServerParameters(command="npx", args=["-y", "screenpipe-mcp"])
        async with stdio_client(server) as (r, w):
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
        from mcp import ClientSession, StdioServerParameters
        from mcp.client.stdio import stdio_client
        server = StdioServerParameters(command="npx", args=["-y", "screenpipe-mcp"])
        async with stdio_client(server) as (r, w):
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


# Module-level singletons — initialised on first call to run_watcher()
_bridge: _MCPBridge | None = None
_patched: bool = False


def _ensure_bridge_and_patch():
    global _bridge, _patched
    if _bridge is None:
        _bridge = _MCPBridge()
    if not _patched:
        _patch_mcp(_bridge)
        _patched = True


def _patch_mcp(bridge: _MCPBridge):
    """Patch run_agent.handle_function_call to route screenpipe tools to MCPBridge."""
    # run_agent is in the hermes repo root; ensure it's importable
    repo_root = Path(__file__).parent.parent
    if str(repo_root) not in sys.path:
        sys.path.insert(0, str(repo_root))

    import run_agent as _ra
    _original = _ra.handle_function_call

    def _patched_handler(function_name, function_args, *args, **kwargs):
        if function_name in bridge.tool_names:
            return bridge.call_tool(function_name, function_args)
        return _original(function_name, function_args, *args, **kwargs)

    _ra.handle_function_call = _patched_handler
    log.debug("Patched run_agent.handle_function_call for screenpipe tools")


# ── Helpers ────────────────────────────────────────────────────────────────────
def _parse_event(text: str, now: datetime) -> dict:
    text = text.strip()
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        pass
    fence = re.search(r"```(?:json)?\s*(\{.*?\})\s*```", text, re.DOTALL)
    if fence:
        try:
            return json.loads(fence.group(1))
        except json.JSONDecodeError:
            pass
    match = re.search(r"\{.*\}", text, re.DOTALL)
    if match:
        try:
            return json.loads(match.group())
        except json.JSONDecodeError:
            pass
    log.warning("Could not parse JSON from LLM response")
    return _fallback(now, text[:300])


def _fallback(now: datetime, raw_summary: str = "") -> dict:
    return {
        "active_app":    "unknown",
        "active_window": "unknown",
        "inferred_task": "Could not capture activity",
        "confidence":    0.0,
        "meetings":      [],
        "app_breakdown": {},
        "raw_summary":   raw_summary,
    }


def _trim_buffer():
    if not BUFFER_FILE.exists():
        return
    lines = [ln for ln in BUFFER_FILE.read_text().strip().split("\n") if ln.strip()]
    if len(lines) > MAX_BUFFER_LINES:
        BUFFER_FILE.write_text("\n".join(lines[-MAX_BUFFER_LINES:]) + "\n")


# ── Public entry point ─────────────────────────────────────────────────────────
def run_watcher() -> dict:
    from run_agent import AIAgent

    _ensure_bridge_and_patch()

    now = datetime.now(timezone.utc)
    log.info("=" * 60)
    log.info("Watcher cycle — %s", now.strftime("%Y-%m-%d %H:%M:%S UTC"))
    log.info("Model: %s | Endpoint: %s", MODEL, BASE_URL)

    start = (now - timedelta(minutes=5)).isoformat()
    user_message = (
        f"Current time: {now.isoformat()}. "
        f"Capture user activity for the window {start} → {now.isoformat()}."
    )

    agent = AIAgent(
        model=MODEL,
        base_url=BASE_URL,
        api_key=API_KEY or "none",
        ephemeral_system_prompt=load_skill("watcher"),
        enabled_toolsets=[],
        quiet_mode=True,
        skip_context_files=True,
        load_soul_identity=True,
        max_iterations=8,
    )
    agent.tools = (agent.tools or []) + _bridge.get_schemas()
    agent.valid_tool_names |= _bridge.tool_names

    t0 = time.time()
    try:
        result = agent.run_conversation(user_message)
    except Exception as exc:
        log.error("Watcher error: %s", exc, exc_info=True)
        result = {}

    elapsed = time.time() - t0

    response_text = ""
    if isinstance(result, dict):
        response_text = result.get("final_response") or result.get("response") or ""

    event = _parse_event(response_text, now) if response_text else _fallback(now)
    event["timestamp"] = now.isoformat()

    log.info("─" * 40)
    log.info("app:        %s", event.get("active_app"))
    log.info("window:     %s", event.get("active_window"))
    log.info("task:       %s", event.get("inferred_task"))
    log.info("confidence: %.2f", event.get("confidence", 0.0))
    log.info("elapsed:    %.1fs", elapsed)
    if event.get("meetings"):
        log.info("meetings:   %s", event["meetings"])

    ACTIVITY_DIR.mkdir(parents=True, exist_ok=True)
    with open(BUFFER_FILE, "a") as f:
        f.write(json.dumps(event) + "\n")
    log.info("Saved to buffer.")

    _trim_buffer()
    return event


if __name__ == "__main__":
    logging.basicConfig(
        level=logging.DEBUG,
        format="%(asctime)s [%(levelname)-8s] %(message)s",
        datefmt="%H:%M:%S",
    )
    import json as _json
    print(_json.dumps(run_watcher(), indent=2))
