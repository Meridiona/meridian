"""Synthesizer agent — reads completed app sessions from meridian.db, tags each
session with the most likely Jira task + category, and updates the user's
current focus.

Architecture (per-session loop with phase-gated tool bridge):

    1. Read up to SESSION_BATCH_LIMIT unprocessed sessions, plus pm_tasks /
       active_session / context_graph_nodes / previous_context.
    2. **Tag phase** — for *each* session, run a focused AIAgent.run_conversation()
       with the `synthesizer-tag` skill. The bridge accepts only
       write_session_summary, match_session_to_task, and upsert_context_node;
       any out-of-phase tool call returns an error string the model can recover
       from. This guarantees we never get the model's "summary of everything"
       drift that happens with a 20-session bundle.
    3. After every session has a match (Python backfills any the model missed),
       switch to **focus phase** and run *one* AIAgent call with the
       `synthesizer-focus` skill. The bridge now only allows write_current_context
       (and optional upsert_context_node).
    4. Advance the cursor and finalise the agent_run row.

The four DB-backed tools the LLM can call:
  - write_session_summary(session_id, summary_json)
  - match_session_to_task(session_id, task_key, confidence, session_type, routing)
  - upsert_context_node(node_id, node_type, label)
  - write_current_context(inferred_task, confidence, trigger_jira_sync, ...)

pm_tasks is populated by the Rust intelligence/providers/jira.rs job. If the
local table is empty (e.g. before the Rust job ever ran), we fall back to the
Atlassian MCP via agents.jira_mcp.
"""
from __future__ import annotations

import json
import logging
import sqlite3
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
from pathlib import Path

from agents import db
from agents.config import (
    MODEL, BASE_URL, API_KEY,
    SESSION_BATCH_LIMIT, CONTEXT_NODES_LIMIT,
    MIN_LLM_DURATION_S, ONLY_TODAY,
    SYNTHESIZER_WORKERS,
    LLM_RETRY_ATTEMPTS, LLM_RETRY_BACKOFF_S,
    load_skill, today_start_utc_iso,
)

log = logging.getLogger("synthesizer")

for _noisy in ["httpx", "httpcore", "openai._base_client"]:
    logging.getLogger(_noisy).setLevel(logging.WARNING)


VALID_SESSION_TYPES = {"task", "overhead", "unknown"}
VALID_ROUTINGS = {"auto", "queue", "skip"}
VALID_NODE_TYPES = {"project", "task", "tool", "pattern", "ticket"}

# Phase names used by the bridge to gate tool calls.
PHASE_TAG = "tag"
PHASE_FOCUS = "focus"

PHASE_TOOLS: dict[str, set[str]] = {
    PHASE_TAG: {
        "write_session_summary",
        "match_session_to_task",
        "upsert_context_node",
    },
    PHASE_FOCUS: {
        "upsert_context_node",
        "write_current_context",
    },
}


# ── Tool schemas ───────────────────────────────────────────────────────────────
_TAG_TOOL_SCHEMAS = [
    {
        "type": "function",
        "function": {
            "name": "write_session_summary",
            "description": (
                "Write a 2-3 sentence summary of one app session. summary_json "
                'must be a JSON-encoded object: {"summary":"...","tags":["..."]}.'
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "session_id":   {"type": "integer"},
                    "summary_json": {"type": "string"},
                },
                "required": ["session_id", "summary_json"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "match_session_to_task",
            "description": (
                "Tag the session. task_key MUST be either null or a key from "
                "pm_tasks. session_type ∈ {task, overhead, unknown}. "
                "routing ∈ {auto, queue, skip}."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "session_id":   {"type": "integer"},
                    "task_key":     {"type": ["string", "null"]},
                    "confidence":   {"type": "number"},
                    "session_type": {"type": "string", "enum": ["task", "overhead", "unknown"]},
                    "routing":      {"type": "string", "enum": ["auto", "queue", "skip"]},
                },
                "required": ["session_id", "confidence", "session_type", "routing"],
            },
        },
    },
    {
        "type": "function",
        "function": {
            "name": "upsert_context_node",
            "description": "Add or update a node in the persistent knowledge graph.",
            "parameters": {
                "type": "object",
                "properties": {
                    "node_id":   {"type": "string"},
                    "node_type": {"type": "string", "enum": list(VALID_NODE_TYPES)},
                    "label":     {"type": "string"},
                },
                "required": ["node_id", "node_type", "label"],
            },
        },
    },
]

_FOCUS_TOOL_SCHEMAS = [
    _TAG_TOOL_SCHEMAS[2],  # upsert_context_node
    {
        "type": "function",
        "function": {
            "name": "write_current_context",
            "description": (
                "Write the user's current focus snapshot. MUST be the final "
                "tool call. Sets trigger_jira_sync (bool) which the jira-keeper reads."
            ),
            "parameters": {
                "type": "object",
                "properties": {
                    "active_project":    {"type": ["string", "null"]},
                    "jira_key":          {"type": ["string", "null"]},
                    "inferred_task":     {"type": "string"},
                    "confidence":        {"type": "number"},
                    "trigger_jira_sync": {"type": "boolean"},
                    "tags":              {"type": "array", "items": {"type": "string"}},
                },
                "required": ["inferred_task", "confidence", "trigger_jira_sync"],
            },
        },
    },
]


# ── Tool bridge ────────────────────────────────────────────────────────────────
class _ToolBridge:
    """Routes the four synthesizer tools into meridian.db, gated by phase.

    State is set up by `init_cycle` (per cycle), then by `set_phase` /
    `set_session_scope` (per LLM call). Out-of-phase tool calls return an
    error string so the model can recover within its iteration budget.
    """

    ALL_TOOL_NAMES = (
        PHASE_TOOLS[PHASE_TAG] | PHASE_TOOLS[PHASE_FOCUS]
    )

    def __init__(self):
        # Per-thread state — phase, scoped session, and the worker's
        # sqlite connection live here so parallel tag-phase calls don't
        # clobber each other's scope.
        self._tls = threading.local()
        # Cycle-wide bookkeeping (shared across threads, guarded by _lock).
        self._lock = threading.Lock()
        self.run_id: int = 0
        self.valid_task_keys: set[str] = set()
        self.expected_session_ids: set[int] = set()
        self.tagged_sessions: dict[int, dict] = {}
        self.summaries_written: int = 0
        self.dispatches_queued: int = 0
        self.context_written: bool = False
        self.last_context: dict = {}

    # ---- thread-local accessors ---------------------------------------------
    @property
    def phase(self) -> str:
        return getattr(self._tls, "phase", PHASE_TAG)

    @property
    def scoped_session_id(self) -> int | None:
        return getattr(self._tls, "scoped_session_id", None)

    @scoped_session_id.setter
    def scoped_session_id(self, value: int | None) -> None:
        self._tls.scoped_session_id = value

    @property
    def conn(self) -> sqlite3.Connection | None:
        return getattr(self._tls, "conn", None)

    @conn.setter
    def conn(self, value: sqlite3.Connection | None) -> None:
        self._tls.conn = value

    # ---- lifecycle -----------------------------------------------------------
    def init_cycle(
        self,
        conn: sqlite3.Connection,
        *,
        run_id: int,
        pm_tasks: list[dict],
        sessions: list[dict],
    ) -> None:
        """Set up cycle-wide state. The supplied connection is the
        main-thread connection — workers open their own."""
        self.conn = conn
        self.run_id = run_id
        self.valid_task_keys = {t["task_key"] for t in pm_tasks}
        self.expected_session_ids = {int(s["id"]) for s in sessions}
        self.tagged_sessions = {}
        self.summaries_written = 0
        self.dispatches_queued = 0
        self.context_written = False
        self.last_context = {}
        self.set_phase(PHASE_TAG)

    def set_phase(self, phase: str) -> None:
        if phase not in PHASE_TOOLS:
            raise ValueError(f"Unknown phase: {phase}")
        self._tls.phase = phase
        self._tls.scoped_session_id = None

    def set_session_scope(self, session_id: int) -> None:
        """Restrict tag-phase tool calls to a specific session id."""
        self._tls.scoped_session_id = int(session_id)

    @property
    def tool_names(self) -> set:
        return self.ALL_TOOL_NAMES

    # ---- dispatcher ----------------------------------------------------------
    def call_tool(self, name: str, args: dict) -> str:
        if name not in PHASE_TOOLS[self.phase]:
            allowed = sorted(PHASE_TOOLS[self.phase])
            log.warning(
                "  bridge: blocked %s in phase=%s (allowed=%s)",
                name, self.phase, allowed,
            )
            return (
                f"ERROR: {name} is not allowed in the {self.phase!r} phase. "
                f"Allowed tools right now: {allowed}."
            )
        log.info("  bridge tool [%s]: %s args=%s", self.phase, name, json.dumps(args)[:300])
        try:
            if name == "write_session_summary":
                return self._write_session_summary(args)
            if name == "match_session_to_task":
                return self._match_session_to_task(args)
            if name == "upsert_context_node":
                return self._upsert_context_node(args)
            if name == "write_current_context":
                return self._write_current_context(args)
        except Exception as exc:
            log.exception("bridge tool %s failed", name)
            return f"ERROR: {name} raised {exc!r}"
        return f"Unknown tool: {name}"

    # ---- individual tool handlers --------------------------------------------
    def _check_session_id(self, raw: object) -> tuple[int | None, str | None]:
        if raw is None:
            return None, "session_id is required."
        try:
            sid = int(raw)
        except (TypeError, ValueError):
            return None, f"session_id must be an integer, got {raw!r}."
        if sid not in self.expected_session_ids:
            return None, (
                f"session_id {sid} is not in this cycle's sessions[]. "
                f"Valid ids: {sorted(self.expected_session_ids)}"
            )
        if self.scoped_session_id is not None and sid != self.scoped_session_id:
            return None, (
                f"This call only handles session {self.scoped_session_id}, "
                f"not {sid}. Skip cross-session tagging."
            )
        return sid, None

    def _write_session_summary(self, args: dict) -> str:
        sid, err = self._check_session_id(args.get("session_id"))
        if err:
            return f"ERROR: {err}"
        raw = args.get("summary_json")
        if raw is None:
            return "ERROR: summary_json is required."
        if isinstance(raw, dict):
            payload = raw
        else:
            try:
                payload = json.loads(raw)
            except (json.JSONDecodeError, TypeError) as exc:
                return f"ERROR: summary_json is not valid JSON: {exc}"
        if not isinstance(payload, dict) or "summary" not in payload:
            return 'ERROR: summary_json must be {"summary":"...","tags":[...]}.'
        db.write_session_summary(
            self.conn,
            session_id=sid,
            agent_run_id=self.run_id,
            summary_json=payload,
        )
        with self._lock:
            self.summaries_written += 1
        return f"Summary written for session {sid}."

    def _match_session_to_task(self, args: dict) -> str:
        sid, err = self._check_session_id(args.get("session_id"))
        if err:
            return f"ERROR: {err}"

        session_type = args.get("session_type")
        routing = args.get("routing")
        if session_type not in VALID_SESSION_TYPES:
            return f"ERROR: session_type must be one of {sorted(VALID_SESSION_TYPES)}."
        if routing not in VALID_ROUTINGS:
            return f"ERROR: routing must be one of {sorted(VALID_ROUTINGS)}."

        task_key = args.get("task_key")
        # Models often emit literal-string sentinels ("None", "null", "n/a")
        # instead of JSON null. Coerce them all to None up front.
        if isinstance(task_key, str) and task_key.strip().lower() in (
            "", "null", "none", "n/a", "nil", "undefined",
        ):
            task_key = None
        if task_key is not None and task_key not in self.valid_task_keys:
            return (
                f"ERROR: task_key {task_key!r} is not in pm_tasks. "
                f"Valid keys: {sorted(self.valid_task_keys) or 'NONE'}"
            )

        try:
            confidence = float(args.get("confidence", 0.0))
        except (TypeError, ValueError):
            return "ERROR: confidence must be a number."
        confidence = max(0.0, min(1.0, confidence))

        record = {
            "session_id":   sid,
            "task_key":     task_key,
            "confidence":   confidence,
            "session_type": session_type,
            "routing":      routing,
        }
        with self._lock:
            self.tagged_sessions[sid] = record

        # ticket_links is the authoritative session→task map (one row per session).
        db.write_ticket_link(
            self.conn,
            session_id=sid,
            task_key=task_key,
            confidence=confidence,
            session_type=session_type,
            routing=routing,
        )

        # dispatch_queue is the outbox the jira-keeper drains. Only matched
        # sessions with non-skip routing belong here.
        if task_key and routing in ("auto", "queue"):
            db.enqueue_dispatch(
                self.conn,
                session_id=sid,
                agent_run_id=self.run_id,
                task_key=task_key,
                provider="jira",
                payload={
                    "routing":      routing,
                    "session_type": session_type,
                    "confidence":   confidence,
                },
            )
            with self._lock:
                self.dispatches_queued += 1

        return f"Tagged session {sid} → {task_key or '∅'} ({session_type}/{routing}, {confidence:.2f})."

    def _upsert_context_node(self, args: dict) -> str:
        nid = args.get("node_id") or args.get("id")
        ntype = args.get("node_type") or args.get("type")
        label = args.get("label")
        if not nid or not ntype or not label:
            return "ERROR: upsert_context_node requires node_id, node_type, label."
        if ntype not in VALID_NODE_TYPES:
            return f"ERROR: node_type must be one of {sorted(VALID_NODE_TYPES)}."
        db.upsert_context_node(self.conn, node_id=nid, node_type=ntype, label=label)
        return f"Node {nid} upserted."

    def _write_current_context(self, args: dict) -> str:
        # Belt-and-braces: write_current_context is only routable here when the
        # phase is `focus`, but we still gate on tagging completeness so the
        # model can't trigger Jira on an incomplete cycle.
        missing = self.expected_session_ids - set(self.tagged_sessions.keys())
        if missing:
            return (
                f"ERROR: cannot write current_context — sessions {sorted(missing)} "
                "are still untagged. Finish tagging in the previous phase first."
            )
        for k in ("inferred_task", "confidence", "trigger_jira_sync"):
            if k not in args:
                return f"ERROR: write_current_context missing required field {k!r}."
        try:
            confidence = float(args["confidence"])
        except (TypeError, ValueError):
            return "ERROR: confidence must be a number."
        confidence = max(0.0, min(1.0, confidence))
        jira_key = args.get("jira_key")
        if isinstance(jira_key, str) and jira_key.strip().lower() in (
            "", "null", "none", "n/a", "nil", "undefined",
        ):
            jira_key = None
        ctx = {
            "active_project":    args.get("active_project"),
            "jira_key":          jira_key,
            "inferred_task":     str(args.get("inferred_task", ""))[:1000],
            "confidence":        confidence,
            "trigger_jira_sync": bool(args.get("trigger_jira_sync", False)),
            "tags":              list(args.get("tags") or []),
        }
        if ctx["jira_key"] and ctx["jira_key"] not in self.valid_task_keys:
            log.warning(
                "write_current_context: jira_key %s not in pm_tasks — clearing",
                ctx["jira_key"],
            )
            ctx["jira_key"] = None
            ctx["trigger_jira_sync"] = False
        db.write_activity_context(self.conn, **ctx)
        self.last_context = ctx
        self.context_written = True
        return "current context written."


# ── Module-level singletons ────────────────────────────────────────────────────
_bridge: _ToolBridge | None = None
_patched: bool = False


def _ensure_bridge_and_patch() -> _ToolBridge:
    global _bridge, _patched
    if _bridge is None:
        _bridge = _ToolBridge()
    if not _patched:
        _patch_run_agent(_bridge)
        _patched = True
    return _bridge


def _patch_run_agent(bridge: _ToolBridge) -> None:
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


# ── pm_tasks fallback ──────────────────────────────────────────────────────────
def _fallback_fetch_pm_tasks(conn: sqlite3.Connection) -> list[dict]:
    """When pm_tasks is empty, pull the user's open Jira issues via mcp-atlassian
    and seed the local cache so subsequent cycles don't pay this cost."""
    log.info("pm_tasks is empty — falling back to Atlassian MCP")
    try:
        from agents import jira_mcp
        rows = jira_mcp.fetch_open_tasks()
    except Exception as exc:
        log.warning("Jira MCP fallback failed: %s", exc)
        return []
    if not rows:
        log.warning("Jira MCP fallback returned 0 issues")
        return []
    for row in rows:
        if not row.get("task_key"):
            continue
        db.upsert_pm_task(conn, **row)
    log.info("Seeded %d pm_tasks from Atlassian MCP", len(rows))
    return db.fetch_pm_tasks(conn)


# ── Debug-logging helpers ──────────────────────────────────────────────────────
def _truncate(value: object, max_len: int = 80) -> str:
    text = str(value or "")
    text = text.replace("\n", " ")
    if len(text) > max_len:
        return text[: max_len - 1] + "…"
    return text


def _session_digest(s: dict) -> str:
    titles = s.get("window_titles") or []
    top_title = ""
    if titles:
        first = titles[0]
        if isinstance(first, (list, tuple)) and first:
            top_title = str(first[0])
        elif isinstance(first, dict):
            top_title = str(first.get("title", ""))
        else:
            top_title = str(first)
    return (
        f"id={s.get('id')} app={_truncate(s.get('app_name'), 24)} "
        f"dur={s.get('duration_s')}s "
        f"titles={len(titles)} ocr={len(s.get('ocr_samples') or [])} "
        f"audio={len(s.get('audio_snippets') or [])} "
        f"cat={s.get('category')}/{round(s.get('confidence') or 0.0, 2)} "
        f"top=\"{_truncate(top_title, 60)}\""
    )


def _task_digest(t: dict) -> str:
    return (
        f"{t.get('task_key'):<10} "
        f"[{_truncate(t.get('status'), 14):<14}] "
        f"({_truncate(t.get('status_category'), 14):<14}) "
        f"{_truncate(t.get('title'), 80)}"
    )


def _log_db_inputs(
    *,
    sessions: list[dict],
    active_session: dict | None,
    pm_tasks: list[dict],
    nodes: list[dict],
    previous_context: dict,
) -> None:
    """Print a human-readable digest of what was just pulled from meridian.db.

    Full JSON payloads are emitted at DEBUG level for deep inspection.
    """
    log.info("DB inputs ─────────────────────────────────────────────")

    log.info("• %d unprocessed app_sessions (cap=%d):", len(sessions), SESSION_BATCH_LIMIT)
    for s in sessions:
        log.info("    - %s", _session_digest(s))

    if active_session:
        log.info("• active_session: %s", _session_digest({**active_session, "duration_s": "open"}))
    else:
        log.info("• active_session: none")

    log.info("• %d pm_tasks (provider=jira, status_category!=done):", len(pm_tasks))
    for t in pm_tasks:
        log.info("    - %s", _task_digest(t))

    log.info("• %d context_graph_nodes:", len(nodes))
    for n in nodes[:10]:
        log.info(
            "    - %s [%s] freq=%s last=%s",
            n.get("node_id"), n.get("node_type"),
            n.get("frequency"), n.get("last_seen"),
        )
    if len(nodes) > 10:
        log.info("    … and %d more", len(nodes) - 10)

    if previous_context:
        log.info(
            "• previous_context: project=%s jira=%s conf=%.2f task=\"%s\" sync=%s",
            previous_context.get("active_project"),
            previous_context.get("jira_key"),
            previous_context.get("confidence") or 0.0,
            _truncate(previous_context.get("inferred_task"), 60),
            previous_context.get("trigger_jira_sync"),
        )
    else:
        log.info("• previous_context: empty")

    log.info("───────────────────────────────────────────────────────")
    log.debug("sessions raw:        %s", json.dumps(sessions, default=str)[:4000])
    log.debug("pm_tasks raw:        %s", json.dumps(pm_tasks, default=str)[:4000])
    log.debug("active_session raw:  %s", json.dumps(active_session, default=str)[:2000])
    log.debug("context_nodes raw:   %s", json.dumps(nodes, default=str)[:2000])
    log.debug("previous_context raw:%s", json.dumps(previous_context, default=str)[:2000])


def _log_bundle(label: str, bundle: dict, *, info_keys: tuple = ()) -> None:
    """Log the agent-bound bundle. INFO-level digest + DEBUG-level full payload."""
    raw = json.dumps(bundle, default=str)
    log.info("→ %s bundle: %d chars (%d top-level keys: %s)",
             label, len(raw), len(bundle), sorted(bundle.keys()))
    for k in info_keys:
        v = bundle.get(k)
        if isinstance(v, list):
            log.info("    %s = list(len=%d) preview=%s", k, len(v), _truncate(v, 200))
        elif isinstance(v, dict):
            log.info("    %s = keys(%s) preview=%s",
                     k, sorted(v.keys()), _truncate(v, 200))
        else:
            log.info("    %s = %s", k, _truncate(v, 200))
    log.debug("%s bundle full: %s", label, raw[:6000])


# ── Pre-filter ────────────────────────────────────────────────────────────────
def _is_trivial_overhead(s: dict) -> bool:
    """Return True if the session is so clearly idle/system noise that the LLM
    has no chance of finding a Jira match in it.

    Conservative — we'd rather pay an extra LLM call than wrongly auto-skip
    real work. Two rules only:

      * Duration below MIN_LLM_DURATION_S (default 30s).
      * No window titles AND no OCR samples AND no audio snippets — there is
        literally no signal for the LLM to match against.

    The earlier "idle_personal + low confidence + <120s" rule was dropped
    because the Rust categorizer reports `idle_personal/0.0` for almost
    every short session, which made the rule swallow real work (Chrome
    browsing, IDE flips, etc.).
    """
    duration = int(s.get("duration_s") or 0)
    titles   = s.get("window_titles") or []
    ocr      = s.get("ocr_samples")    or []
    audio    = s.get("audio_snippets") or []

    if duration < MIN_LLM_DURATION_S:
        return True
    if not titles and not ocr and not audio:
        return True
    return False


def _split_prefiltered(sessions: list[dict]) -> tuple[list[dict], list[dict]]:
    keep: list[dict] = []
    skip: list[dict] = []
    for s in sessions:
        (skip if _is_trivial_overhead(s) else keep).append(s)
    return keep, skip


def _auto_tag_overhead(
    bridge: "_ToolBridge",
    sessions: list[dict],
    *,
    run_id: int,
    conn: sqlite3.Connection,
) -> None:
    """Write `overhead/skip` ticket_links + a stock summary for every prefiltered session."""
    bridge.set_phase(PHASE_TAG)
    for s in sessions:
        bridge.scoped_session_id = None
        bridge._match_session_to_task({  # noqa: SLF001
            "session_id":   int(s["id"]),
            "task_key":     None,
            "confidence":   0.0,
            "session_type": "overhead",
            "routing":      "skip",
        })
        db.write_session_summary(
            conn,
            session_id=int(s["id"]),
            agent_run_id=run_id,
            summary_json={
                "summary": (
                    f"Trivial {s.get('app_name', '?')} session "
                    f"({s.get('duration_s', 0)}s). Auto-classified as overhead."
                ),
                "tags": ["auto-overhead"],
            },
        )
        bridge.summaries_written += 1


# ── AIAgent helpers ────────────────────────────────────────────────────────────
def _make_agent(skill_name: str, schemas: list[dict], max_iterations: int):
    """Instantiate a fresh AIAgent for one focused conversation."""
    from run_agent import AIAgent
    agent = AIAgent(
        model=MODEL,
        base_url=BASE_URL,
        api_key=API_KEY or "none",
        ephemeral_system_prompt=load_skill(skill_name),
        enabled_toolsets=[],
        quiet_mode=True,
        skip_context_files=True,
        load_soul_identity=True,
        max_iterations=max_iterations,
    )
    agent.tools = (agent.tools or []) + schemas
    valid = {s["function"]["name"] for s in schemas}
    agent.valid_tool_names |= valid
    return agent


# ── Tag phase ──────────────────────────────────────────────────────────────────
def _looks_like_rate_limit(exc: BaseException) -> bool:
    text = str(exc).lower()
    return (
        "429" in text
        or "rate limit" in text
        or "too many requests" in text
        or "rate_limit_exceeded" in text
    )


def _run_with_backoff(fn, *, attempts: int, backoff_s: float, label: str):
    """Invoke `fn` up to `attempts` times, sleeping `backoff_s * 2^k` between
    tries when the exception looks like a rate-limit error. Other exceptions
    bubble immediately."""
    last_exc: BaseException | None = None
    for k in range(attempts):
        try:
            return fn()
        except Exception as exc:
            last_exc = exc
            if k == attempts - 1 or not _looks_like_rate_limit(exc):
                raise
            wait = backoff_s * (2 ** k)
            log.warning("%s: rate-limited (%s) — retry %d/%d in %.1fs",
                        label, exc, k + 1, attempts, wait)
            time.sleep(wait)
    if last_exc:
        raise last_exc
    return None


def _tag_one_session(
    bridge: _ToolBridge,
    *,
    session: dict,
    pm_tasks: list[dict],
    previous_context: dict,
    context_nodes: list[dict],
) -> bool:
    """Run one focused AIAgent call to tag exactly one session.

    Each worker thread opens its own sqlite connection (sqlite3 connections
    are not thread-safe) and writes via the shared bridge. The bridge's
    phase + scoped_session_id are thread-local, so multiple workers can
    operate concurrently without colliding.

    Returns True if the model emitted both a match and a summary for this
    session, False otherwise (Python backfill picks up the slack).
    """
    sid = int(session["id"])

    # Per-thread DB connection. Closed at the end so we don't leak fds.
    worker_conn: sqlite3.Connection | None = None
    try:
        worker_conn = sqlite3.connect(str(Path(db.MERIDIAN_DB).expanduser()),
                                      isolation_level=None, timeout=15.0)
        worker_conn.row_factory = sqlite3.Row
        worker_conn.execute("PRAGMA journal_mode=WAL;")
        worker_conn.execute("PRAGMA foreign_keys=ON;")
        bridge.conn = worker_conn  # thread-local

        bridge.set_phase(PHASE_TAG)
        bridge.set_session_scope(sid)

        bundle = {
            "now":                 datetime.now(timezone.utc).isoformat(),
            "session":             session,
            "pm_tasks":            pm_tasks,
            "previous_context":    previous_context,
            "context_graph_nodes": context_nodes,
        }
        _log_bundle(
            f"tag-phase session {sid}",
            bundle,
            info_keys=("session", "pm_tasks", "previous_context"),
        )

        user_message = (
            f"Tag session {sid}. Call match_session_to_task and write_session_summary "
            "exactly once each, plus any helpful upsert_context_node calls. Do not "
            "write current_context — that's a later phase. Bundle:\n\n"
            + json.dumps(bundle, indent=2, default=str)
        )

        with bridge._lock:
            pre_summaries = bridge.summaries_written

        agent = _make_agent("synthesizer-tag", _TAG_TOOL_SCHEMAS, max_iterations=6)
        try:
            _run_with_backoff(
                lambda: agent.run_conversation(user_message),
                attempts=LLM_RETRY_ATTEMPTS,
                backoff_s=LLM_RETRY_BACKOFF_S,
                label=f"tag session {sid}",
            )
        except Exception as exc:
            log.error("Tag-phase LLM error on session %s: %s", sid, exc, exc_info=True)

        with bridge._lock:
            post_match = sid in bridge.tagged_sessions
            summary_added = bridge.summaries_written > pre_summaries
        return post_match and summary_added
    finally:
        if worker_conn is not None:
            try:
                worker_conn.close()
            except Exception:
                pass
        bridge.conn = None


def _run_tag_phase(
    bridge: _ToolBridge,
    *,
    llm_sessions: list[dict],
    pm_tasks: list[dict],
    previous_context: dict,
    context_nodes: list[dict],
) -> int:
    """Tag every LLM-eligible session, in parallel up to SYNTHESIZER_WORKERS.

    Sequential when SYNTHESIZER_WORKERS == 1 (no thread pool overhead).
    Each worker opens its own sqlite connection and gets its own thread-local
    bridge state, so the cycle-wide counters/maps are the only shared state
    (guarded by bridge._lock).
    """
    total = len(llm_sessions)
    if total == 0:
        log.info("Tag phase: nothing to do (0 LLM-eligible sessions)")
        return 0

    workers = min(SYNTHESIZER_WORKERS, total)
    log.info("Tag phase starting — %d session(s), %d worker(s)", total, workers)

    if workers <= 1:
        ok_count = 0
        for idx, s in enumerate(llm_sessions, start=1):
            log.info("─" * 40)
            log.info("Tag %d/%d — session %s (%s, %ss)",
                     idx, total, s["id"], s["app_name"], s.get("duration_s"))
            if _tag_one_session(
                bridge,
                session=s,
                pm_tasks=pm_tasks,
                previous_context=previous_context,
                context_nodes=context_nodes,
            ):
                ok_count += 1
        return ok_count

    ok_count = 0
    completed = 0
    completed_lock = threading.Lock()

    def _worker(s: dict) -> bool:
        nonlocal completed
        sid = int(s["id"])
        try:
            return _tag_one_session(
                bridge,
                session=s,
                pm_tasks=pm_tasks,
                previous_context=previous_context,
                context_nodes=context_nodes,
            )
        finally:
            with completed_lock:
                completed += 1
                log.info(
                    "Tag %d/%d done — session %s (%s, %ss)",
                    completed, total, sid, s["app_name"], s.get("duration_s"),
                )

    with ThreadPoolExecutor(max_workers=workers, thread_name_prefix="syn-tag") as pool:
        futures = {pool.submit(_worker, s): int(s["id"]) for s in llm_sessions}
        for fut in as_completed(futures):
            sid = futures[fut]
            try:
                if fut.result():
                    ok_count += 1
            except Exception as exc:
                log.error("Worker for session %s raised: %s", sid, exc, exc_info=True)
    return ok_count


def _backfill_missing_tags(bridge: _ToolBridge) -> int:
    """For every session the model failed to tag, write `unknown/skip`."""
    missing = bridge.expected_session_ids - set(bridge.tagged_sessions.keys())
    for sid in sorted(missing):
        log.warning(
            "synthesizer: model did not tag session %d — backfilling unknown/skip",
            sid,
        )
        # Allow the bridge to write even though scope is set: clear scope first.
        bridge.set_session_scope(0) if False else None  # noqa
        bridge.scoped_session_id = None
        bridge._match_session_to_task({  # noqa: SLF001 — internal call
            "session_id":   sid,
            "task_key":     None,
            "confidence":   0.0,
            "session_type": "unknown",
            "routing":      "skip",
        })
    return len(missing)


# ── Focus phase ────────────────────────────────────────────────────────────────
def _focus_pass(
    bridge: _ToolBridge,
    *,
    sessions: list[dict],
    active_session: dict | None,
    pm_tasks: list[dict],
    previous_context: dict,
    context_nodes: list[dict],
) -> None:
    """Roll the just-tagged sessions plus active_session into a current-focus snapshot."""
    bridge.set_phase(PHASE_FOCUS)
    bridge.scoped_session_id = None

    recent_tags = [
        bridge.tagged_sessions[int(s["id"])]
        for s in sessions
        if int(s["id"]) in bridge.tagged_sessions
    ]
    bundle = {
        "now":                 datetime.now(timezone.utc).isoformat(),
        "recent_tags":         recent_tags,
        "active_session":      active_session,
        "pm_tasks":            pm_tasks,
        "previous_context":    previous_context,
        "context_graph_nodes": context_nodes,
    }
    _log_bundle(
        "focus-phase",
        bundle,
        info_keys=("recent_tags", "active_session", "pm_tasks", "previous_context"),
    )

    user_message = (
        "Focus phase. Every session has been tagged. Roll the tags + active_session "
        "into a single write_current_context call. That call MUST be your final tool "
        "call. Bundle:\n\n"
        + json.dumps(bundle, indent=2, default=str)
    )

    agent = _make_agent("synthesizer-focus", _FOCUS_TOOL_SCHEMAS, max_iterations=4)
    try:
        agent.run_conversation(user_message)
    except Exception as exc:
        log.error("Focus-phase LLM error: %s", exc, exc_info=True)

    if not bridge.context_written:
        log.warning("synthesizer: focus phase did not call write_current_context — writing fallback")
        prev = previous_context or {}
        bridge._write_current_context({  # noqa: SLF001
            "active_project":    prev.get("active_project"),
            "jira_key":          prev.get("jira_key") if prev.get("jira_key") in bridge.valid_task_keys else None,
            "inferred_task":     prev.get("inferred_task") or "Unable to infer activity.",
            "confidence":        max(0.0, min(0.85, (prev.get("confidence") or 0.0) - 0.1)),
            "trigger_jira_sync": False,
            "tags":              prev.get("tags") or [],
        })


# ── Public entry point ─────────────────────────────────────────────────────────
def run_synthesizer() -> dict:
    bridge = _ensure_bridge_and_patch()

    log.info("=" * 60)
    log.info("Synthesizer cycle — %s", datetime.now(timezone.utc).isoformat())
    log.info("Model: %s | Endpoint: %s", MODEL, BASE_URL)

    with db.connection() as conn:
        run_id = db.start_agent_run(conn)
        try:
            since_iso = today_start_utc_iso() if ONLY_TODAY else None
            sessions  = db.fetch_unprocessed_sessions(
                conn, SESSION_BATCH_LIMIT, since_iso=since_iso,
            )
            active    = db.fetch_active_session(conn)
            pm_tasks  = db.fetch_pm_tasks(conn)
            if not pm_tasks:
                pm_tasks = _fallback_fetch_pm_tasks(conn)
            nodes     = db.fetch_context_graph_nodes(conn, CONTEXT_NODES_LIMIT)
            prev      = db.fetch_activity_context(conn)

            log.info(
                "%d session(s) to analyse | active=%s | %d pm_tasks | %d nodes",
                len(sessions),
                active["app_name"] if active else "—",
                len(pm_tasks),
                len(nodes),
            )
            if since_iso:
                log.info("filter: ONLY_TODAY since=%s", since_iso)

            bridge.init_cycle(conn, run_id=run_id, pm_tasks=pm_tasks, sessions=sessions)

            # Pre-filter: auto-tag obvious overhead in Python so we don't
            # burn LLM iterations on idle blips.
            llm_sessions, prefiltered = _split_prefiltered(sessions)
            if prefiltered:
                log.info(
                    "Pre-filter: auto-tagging %d trivial session(s) as overhead/skip",
                    len(prefiltered),
                )
                _auto_tag_overhead(bridge, prefiltered, run_id=run_id, conn=conn)
            log.info("LLM-eligible sessions: %d", len(llm_sessions))

            _log_db_inputs(
                sessions=sessions,
                active_session=active,
                pm_tasks=pm_tasks,
                nodes=nodes,
                previous_context=prev,
            )

            bridge.init_cycle(conn, run_id=run_id, pm_tasks=pm_tasks, sessions=sessions)

            t0 = time.time()
            tag_ok = _run_tag_phase(
                bridge,
                llm_sessions=llm_sessions,
                pm_tasks=pm_tasks,
                previous_context=prev,
                context_nodes=nodes,
            )

            backfilled = _backfill_missing_tags(bridge)
            log.info("─" * 40)
            log.info(
                "Tag phase done: %d/%d clean (LLM), %d auto-overhead, %d backfilled",
                tag_ok, len(llm_sessions), len(prefiltered), backfilled,
            )

            _focus_pass(
                bridge,
                sessions=sessions,
                active_session=active,
                pm_tasks=pm_tasks,
                previous_context=prev,
                context_nodes=nodes,
            )

            if sessions:
                db.advance_cursor(conn, db.session_id_max(sessions))

            db.complete_agent_run(
                conn,
                run_id,
                "success",
                sessions_processed=len(sessions),
                summaries_written=bridge.summaries_written,
                links_written=len(bridge.tagged_sessions),
                dispatches_queued=bridge.dispatches_queued,
            )

            elapsed = time.time() - t0
            ctx = db.fetch_activity_context(conn)
            log.info("─" * 40)
            log.info("sessions:   %d (%d backfilled unknown/skip)", len(sessions), backfilled)
            log.info("summaries:  %d written", bridge.summaries_written)
            log.info("dispatches: %d queued", bridge.dispatches_queued)
            log.info("project:    %s", ctx.get("active_project"))
            log.info("jira_key:   %s", ctx.get("jira_key"))
            log.info("task:       %s", (ctx.get("inferred_task") or "")[:80])
            log.info("confidence: %.2f", ctx.get("confidence", 0.0))
            log.info("jira_sync:  %s", ctx.get("trigger_jira_sync"))
            log.info("elapsed:    %.1fs", elapsed)

            return ctx
        except Exception as exc:
            log.error("Synthesizer cycle failed: %s", exc, exc_info=True)
            db.complete_agent_run(conn, run_id, "failed", error=str(exc))
            raise


if __name__ == "__main__":
    logging.basicConfig(
        level=logging.DEBUG,
        format="%(asctime)s [%(levelname)-8s] %(message)s",
        datefmt="%H:%M:%S",
    )
    print(json.dumps(run_synthesizer(), indent=2, default=str))
