"""Synthesizer agent — reads activity buffer, infers context, updates knowledge map.

Uses AIAgent from run_agent.py. The two Python-side tools (write_current_context,
upsert_context_map_node) are injected into AIAgent's tool list and routed via a
handle_function_call patch — same pattern as agents/watcher.py.
"""
import json
import logging
import sys
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path

from agents.config import (
    MODEL, BASE_URL, API_KEY,
    BUFFER_FILE, CONTEXT_MAP_FILE, CURRENT_CONTEXT_FILE,
    BUFFER_WINDOW_MINUTES,
    load_skill,
)

log = logging.getLogger("synthesizer")

for _noisy in ["httpx", "httpcore", "openai._base_client"]:
    logging.getLogger(_noisy).setLevel(logging.WARNING)


_TOOL_SCHEMAS = [
    {
        "type": "function",
        "function": {
            "name": "write_current_context",
            "description": "Write the inferred current context to disk.",
            "parameters": {
                "type": "object",
                "properties": {
                    "active_project":    {"type": ["string", "null"], "description": "Project name or null"},
                    "jira_key":          {"type": ["string", "null"], "description": "Jira ticket key e.g. PROJ-123, or null"},
                    "inferred_task":     {"type": "string",           "description": "One sentence: what is the user doing"},
                    "confidence":        {"type": "number",           "description": "0.0-1.0"},
                    "trigger_jira_sync": {"type": "boolean",          "description": "True if Jira should be updated"},
                    "tags":              {"type": "array", "items": {"type": "string"}},
                },
                "required": ["inferred_task", "confidence", "trigger_jira_sync"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "upsert_context_map_node",
            "description": "Add or update a node in the persistent knowledge map.",
            "parameters": {
                "type": "object",
                "properties": {
                    "id":    {"type": "string", "description": "Slug id e.g. project_backend-api"},
                    "type":  {"type": "string", "enum": ["project", "task", "tool", "pattern", "ticket"]},
                    "label": {"type": "string"},
                },
                "required": ["id", "type", "label"],
            },
        },
    },
]


# ── Python-side tool bridge ────────────────────────────────────────────────────
class _ToolBridge:
    """Executes write_current_context and upsert_context_map_node as Python functions.

    Per-cycle state (now, context_map) is set via init_cycle() before each run.
    """

    def __init__(self):
        self._now: datetime | None = None
        self._context_map: dict = {}

    def init_cycle(self, now: datetime, context_map: dict):
        self._now = now
        self._context_map = context_map

    @property
    def tool_names(self) -> set:
        return {"write_current_context", "upsert_context_map_node"}

    def call_tool(self, name: str, args: dict) -> str:
        log.info("  bridge tool: %s args=%s", name, json.dumps(args)[:300])
        if name == "write_current_context":
            return self._write_current_context(args)
        if name == "upsert_context_map_node":
            return self._upsert_context_map_node(args)
        return f"Unknown tool: {name}"

    def _write_current_context(self, args: dict) -> str:
        if "inferred_task" not in args or "confidence" not in args or "trigger_jira_sync" not in args:
            missing = [k for k in ("inferred_task", "confidence", "trigger_jira_sync") if k not in args]
            msg = f"ERROR: write_current_context missing required field(s): {missing}. Retry with all required fields."
            log.warning("  %s", msg)
            return msg
        ctx = {
            "timestamp":         self._now.isoformat(),
            "active_project":    args.get("active_project"),
            "jira_key":          args.get("jira_key"),
            "inferred_task":     args.get("inferred_task", ""),
            "confidence":        args.get("confidence", 0.0),
            "trigger_jira_sync": args.get("trigger_jira_sync", False),
            "tags":              args.get("tags", []),
        }
        CURRENT_CONTEXT_FILE.write_text(json.dumps(ctx, indent=2))
        log.info("  wrote current_context.json — project=%s jira=%s conf=%.2f sync=%s",
                 ctx["active_project"], ctx["jira_key"], ctx["confidence"], ctx["trigger_jira_sync"])
        return "current_context.json written successfully"

    def _upsert_context_map_node(self, args: dict) -> str:
        missing = [k for k in ("id", "type", "label") if k not in args or args.get(k) in (None, "")]
        if missing:
            msg = f"ERROR: upsert_context_map_node missing required field(s): {missing}. Retry with id, type, and label all set."
            log.warning("  %s", msg)
            return msg
        node_id = args["id"]
        existing = next((n for n in self._context_map["nodes"] if n["id"] == node_id), None)
        if existing:
            existing["last_seen"]  = self._now.isoformat()
            existing["frequency"]  = existing.get("frequency", 0) + 1
            log.info("  updated node: %s (freq=%d)", node_id, existing["frequency"])
        else:
            self._context_map["nodes"].append({
                "id":             node_id,
                "type":           args["type"],
                "label":          args["label"],
                "last_seen":      self._now.isoformat(),
                "frequency":      1,
                "confidence_avg": 0.7,
            })
            log.info("  added node: %s (%s)", node_id, args["type"])
        self._context_map["last_updated"] = self._now.isoformat()
        CONTEXT_MAP_FILE.write_text(json.dumps(self._context_map, indent=2))
        return f"Node {node_id} upserted"


# Module-level singletons
_bridge: _ToolBridge | None = None
_patched: bool = False


def _ensure_bridge_and_patch():
    global _bridge, _patched
    if _bridge is None:
        _bridge = _ToolBridge()
    if not _patched:
        _patch_tools(_bridge)
        _patched = True


def _patch_tools(bridge: _ToolBridge):
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
    log.debug("Patched run_agent.handle_function_call for synthesizer tools")


# ── State helpers ──────────────────────────────────────────────────────────────
def _read_buffer(minutes: int) -> list[dict]:
    if not BUFFER_FILE.exists():
        return []
    cutoff = datetime.now(timezone.utc) - timedelta(minutes=minutes)
    events = []
    for line in BUFFER_FILE.read_text().strip().splitlines():
        if not line.strip():
            continue
        try:
            ev = json.loads(line)
            ts = datetime.fromisoformat(ev.get("timestamp", ""))
            if ts.tzinfo is None:
                ts = ts.replace(tzinfo=timezone.utc)
            if ts >= cutoff:
                events.append(ev)
        except (json.JSONDecodeError, ValueError):
            continue
    return events


def _load_json(path: Path, default) -> dict:
    if path.exists():
        try:
            return json.loads(path.read_text())
        except json.JSONDecodeError:
            pass
    return default


# ── Public entry point ─────────────────────────────────────────────────────────
def run_synthesizer() -> dict:
    from run_agent import AIAgent

    _ensure_bridge_and_patch()

    now    = datetime.now(timezone.utc)
    events = _read_buffer(BUFFER_WINDOW_MINUTES)

    log.info("=" * 60)
    log.info("Synthesizer cycle — %s", now.strftime("%Y-%m-%d %H:%M:%S UTC"))
    log.info("Model: %s | Endpoint: %s", MODEL, BASE_URL)

    if not events:
        log.warning("No events in buffer for the last %d minutes — nothing to synthesize",
                    BUFFER_WINDOW_MINUTES)
        return {}

    context_map  = _load_json(CONTEXT_MAP_FILE,     {"nodes": [], "edges": [], "last_updated": None})
    prev_context = _load_json(CURRENT_CONTEXT_FILE, {})

    log.info("%d events in buffer (last %d min)", len(events), BUFFER_WINDOW_MINUTES)

    # Set per-cycle state on the bridge before running the agent
    _bridge.init_cycle(now, context_map)

    user_message = (
        f"Analyze the following activity data and update the context.\n\n"
        f"RECENT ACTIVITY ({len(events)} events, last {BUFFER_WINDOW_MINUTES} min):\n"
        f"{json.dumps(events, indent=2)}\n\n"
        f"CURRENT CONTEXT MAP:\n{json.dumps(context_map, indent=2)}\n\n"
        f"PREVIOUS CONTEXT:\n{json.dumps(prev_context, indent=2)}"
    )

    agent = AIAgent(
        model=MODEL,
        base_url=BASE_URL,
        api_key=API_KEY or "none",
        ephemeral_system_prompt=load_skill("synthesizer"),
        enabled_toolsets=[],
        quiet_mode=True,
        skip_context_files=True,
        load_soul_identity=True,
        max_iterations=10,
    )
    agent.tools = (agent.tools or []) + _TOOL_SCHEMAS
    agent.valid_tool_names |= _bridge.tool_names

    t0 = time.time()
    try:
        result = agent.run_conversation(user_message)
    except Exception as exc:
        log.error("Synthesizer error: %s", exc, exc_info=True)
        result = {}

    elapsed = time.time() - t0
    ctx = _load_json(CURRENT_CONTEXT_FILE, {})

    if ctx:
        log.info("─" * 40)
        log.info("project:    %s", ctx.get("active_project"))
        log.info("jira_key:   %s", ctx.get("jira_key"))
        log.info("task:       %s", ctx.get("inferred_task"))
        log.info("confidence: %.2f", ctx.get("confidence", 0.0))
        log.info("jira_sync:  %s", ctx.get("trigger_jira_sync"))
        log.info("elapsed:    %.1fs", elapsed)

    return ctx


if __name__ == "__main__":
    import json as _json
    logging.basicConfig(
        level=logging.DEBUG,
        format="%(asctime)s [%(levelname)-8s] %(message)s",
        datefmt="%H:%M:%S",
    )
    print(_json.dumps(run_synthesizer(), indent=2))
