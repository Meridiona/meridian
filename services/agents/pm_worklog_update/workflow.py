"""The pm_worklog_update_cycle Workflow.

This is the spine of the package. It wires the steps described in the
architecture doc:

  Collect → Condition(heavy/light) → Synthesise → Ground → Route

Designed to be invoked one of two ways:

  1. **CLI / standalone** — `cli.py` calls `run_cycle()` directly.
  2. **Daemon** (later)   — `jira_updater_daemon` will import
     `run_cycle_async()` and fire it on the hourly cron.

The Workflow's `session_state` carries cross-cycle context (earlier
posts today, the high-water mark of the last posted session) so the
synth has continuity without re-querying the DB.

No status-transition logic: this workflow only generates Jira comments.
Status changes stay a human decision.
"""
from __future__ import annotations

import json
import logging
from datetime import datetime, timezone
from pathlib import Path
from typing import TYPE_CHECKING, Optional

from agents.pm_worklog_update import agents as pm_agents
from agents.pm_worklog_update import config, db
from agents.pm_worklog_update.hooks import build_grounded_narrative, risk_flagger
from agents.pm_worklog_update.learning import build_learning_machine
from agents.pm_worklog_update.models import (
    BulletWithEvidence,
    GroundedNarrative,
    JiraUpdate,
    RiskFlag,
    RouteOutcome,
    SessionBundle,
    UpdateState,
)

if TYPE_CHECKING:
    from agno.workflow.workflow import Workflow

log = logging.getLogger(__name__)


# ──────────────────────── Workflow factory ─────────────────────────────────────


def build_workflow(*, debug_mode: bool = False, debug_level: int = 1) -> "Workflow":
    """Construct the Workflow object once per process.

    Heavy imports (agno) live inside this function — the module itself
    stays importable without the optional dependency.

    Args:
        debug_mode: Pass through to Workflow + every Agent's `debug_mode`.
        debug_level: 1 (default) or 2 (verbose) — only honoured when
            debug_mode is True.
    """
    from agno.db.sqlite import SqliteDb
    from agno.workflow.step import Step
    from agno.workflow.workflow import Workflow

    agno_db = SqliteDb(
        db_file=str(config.MERIDIAN_DB),
        session_table="agno_workflow_sessions",
        memory_table="agno_pm_worklog_memories",
        metrics_table="agno_pm_worklog_metrics",
        eval_table="agno_pm_worklog_eval_runs",
        approvals_table="agno_pm_worklog_approvals",
    )

    synth = pm_agents.build_synth_agent(
        db=agno_db,
        debug_mode=debug_mode, debug_level=debug_level,
    )

    workflow = Workflow(
        name="pm_worklog_update_cycle",
        description="Hourly Jira worklog update from classified sessions",
        db=agno_db,
        store_executor_outputs=True,           # full audit per step
        store_events=True,                     # capture LLM messages for run logs
        debug_mode=debug_mode,
        debug_level=debug_level,
        steps=[
            Step(name="collect",    executor=_step_collect),
            Step(name="synthesise", agent=synth),
            Step(name="ground",     executor=_step_ground),
            Step(name="route",      executor=_step_route),
        ],
        session_state={
            "task_key": None,
            "earlier_today_summaries": [],
            "last_posted_session_id": None,
        },
    )
    return workflow


# ──────────────────────── Public entry points ──────────────────────────────────


def run_cycle(
    *,
    task_key: str,
    window_start: datetime,
    window_end: datetime,
    cycle_index: int = 0,
    dry_run: bool = True,
    debug_mode: bool = False,
    debug_level: int = 1,
) -> RouteOutcome:
    """Synchronous one-shot — used by the CLI."""
    db.init_schema()

    log_path, fh = _open_run_log(task_key)
    try:
        workflow = build_workflow(debug_mode=debug_mode, debug_level=debug_level)
        bundle = db.fetch_session_bundle(
            task_key=task_key,
            window_start=window_start,
            window_end=window_end,
            cycle_index=cycle_index,
        )
        log.info(
            "pm_worklog cycle starting — task=%s sessions=%d real_s=%d",
            task_key, len(bundle.sessions), bundle.real_seconds,
        )

        user_message = _render_workflow_input(bundle)
        fh.stream.write(f"\n--- USER MESSAGE ---\n{user_message}\n--- END ---\n")
        fh.stream.flush()

        response = workflow.run(
            input=user_message,
            additional_data={
                "task_key":     task_key,
                "window_start": bundle.window_start,
                "window_end":   bundle.window_end,
                "cycle_index":  cycle_index,
                "dry_run":      dry_run,
                "bundle":       bundle.model_dump(),
            },
        )
        outcome = _extract_outcome(response)
        log.info("pm_worklog cycle done — state=%s reason=%s", outcome.state.value, outcome.reason)
        _append_run_summary(fh, response=response, outcome=outcome)
    finally:
        logging.getLogger().removeHandler(fh)
        for name in ("agno", "agno-workflow", "agno-team", "agno-tools"):
            logging.getLogger(name).removeHandler(fh)
        fh.close()
        log.info("pm_worklog run log: %s", log_path)

    return outcome


async def run_cycle_async(**kwargs) -> RouteOutcome:
    """Async variant for the daemon path (later)."""
    db.init_schema()
    workflow = build_workflow(
        debug_mode=kwargs.get("debug_mode", False),
        debug_level=kwargs.get("debug_level", 1),
    )
    bundle = db.fetch_session_bundle(
        task_key=kwargs["task_key"],
        window_start=kwargs["window_start"],
        window_end=kwargs["window_end"],
        cycle_index=kwargs.get("cycle_index", 0),
    )
    response = await workflow.arun(
        input=_render_workflow_input(bundle),
        additional_data={
            "task_key":     kwargs["task_key"],
            "window_start": bundle.window_start,
            "window_end":   bundle.window_end,
            "cycle_index":  kwargs.get("cycle_index", 0),
            "dry_run":      kwargs.get("dry_run", True),
            "bundle":       bundle.model_dump(),
        },
    )
    return _extract_outcome(response)


# ──────────────────────── Step executors ───────────────────────────────────────


def _step_collect(step_input, run_context=None):
    """Unpack the SessionBundle and stash cross-step context in session_state.

    The bundle was already assembled by `run_cycle()` before the workflow
    started; this step just hydrates it from `additional_data` and seeds
    session_state. Every window is processed regardless of activity level.
    """
    from agno.workflow.types import StepOutput

    data = step_input.additional_data or {}
    bundle = SessionBundle.model_validate(data["bundle"])

    if run_context is not None and run_context.session_state is not None:
        run_context.session_state["task_key"] = bundle.task_key
        run_context.session_state["earlier_today_summaries"] = list(
            bundle.earlier_today_summaries
        )

    log.info(
        "collect: task=%s sessions=%d real_s=%d heavy=%s",
        bundle.task_key, len(bundle.sessions), bundle.real_seconds, bundle.is_heavy,
    )
    return StepOutput(content=bundle.model_dump_json())



def _step_ground(step_input, run_context=None):
    """Drop un-grounded bullets, compute coverage, attach risk flags."""
    from agno.workflow.types import StepOutput

    update = _extract_jira_update(step_input)
    if update is None:
        return StepOutput(
            content=RouteOutcome(
                state=UpdateState.FAILED,
                reason="ground: no JiraUpdate produced upstream",
            ).model_dump_json(),
            stop=True,
        )

    bundle = SessionBundle.model_validate((step_input.additional_data or {})["bundle"])

    # Override time_spent_seconds with the Python-computed value — the LLM
    # is not responsible for calculating time; real_seconds is authoritative.
    update = update.model_copy(update={"time_spent_seconds": bundle.real_seconds})

    grounded = build_grounded_narrative(update)
    risk_flagger(grounded, bundle=bundle)
    log.info(
        "ground: coverage=%.2f dropped=%d task=%s time_spent=%ds",
        grounded.coverage, len(grounded.dropped_bullets), update.task_key,
        bundle.real_seconds,
    )
    return StepOutput(content=grounded.model_dump_json())


def _step_route(step_input, run_context=None):
    """Final gate: persist the row, optionally post the Jira worklog.

    Posting policy (phase 1 — worklog only):
      * dry_run=True  → always DRAFTED, no Jira call
      * dry_run=False AND bullets/ticket sane AND real_seconds >= 60
          → call jira_poster.post_worklog, stamp the row POSTED
      * any Jira failure → leave the row DRAFTED with a reason; the
        next cycle will retry the same window because of the
        uq_pm_worklogs_worklog_window index (only "posted" rows are
        unique-constrained, so DRAFTED rows can be replaced).
    """
    from datetime import datetime as _dt

    from agno.workflow.types import StepOutput

    grounded = _extract_grounded(step_input)
    if grounded is None:
        return StepOutput(
            content=RouteOutcome(state=UpdateState.FAILED, reason="route: no grounded").model_dump_json(),
            stop=True,
        )

    extras = step_input.additional_data or {}
    bundle = SessionBundle.model_validate(extras["bundle"])
    update = grounded.update
    dry_run = extras.get("dry_run", True)

    # First-pass decision matrix (might still flip below after a real post).
    state = _decide_state(update, grounded.coverage, bundle)
    pm_worklog_id = db.upsert_pm_worklog(
        update,
        state=state,
        coverage=grounded.coverage,
        session_id_min=min((s.id for s in bundle.sessions), default=None),
        session_id_max=max((s.id for s in bundle.sessions), default=None),
    )
    reason = _state_reason(state, update, grounded.coverage)
    posted_worklog_id: Optional[str] = None

    if dry_run:
        log.info("route: dry_run=True — skipping Jira post")
    else:
        post_decision = _evaluate_post_eligibility(state, bundle, update)
        if post_decision is None:
            log.info("route: post eligible — attempting jira worklog")
            try:
                posted_worklog_id = _post_worklog(bundle, update)
            except Exception as exc:                    # noqa: BLE001 — log + keep DRAFTED
                log.exception("route: jira post failed; row remains %s", state.value)
                reason = f"{reason}; jira post failed: {exc}"
            else:
                db.mark_worklog_posted(pm_worklog_id, posted_worklog_id)
                state = UpdateState.POSTED
                reason = f"worklog {posted_worklog_id} posted"
        else:
            log.info("route: post NOT eligible — %s", post_decision)
            reason = f"{reason}; post skipped: {post_decision}"

    outcome = RouteOutcome(
        state=state,
        pm_worklog_id=pm_worklog_id,
        posted_comment_id=posted_worklog_id,            # piggyback on field for now
        reason=reason,
    )
    log.info(
        "route: state=%s pm_worklog_id=%d worklog_id=%s reason=%s",
        state.value, pm_worklog_id, posted_worklog_id or "-", reason,
    )
    return StepOutput(content=outcome.model_dump_json())


# ──────────────────────── Posting helpers ──────────────────────────────────────


def _evaluate_post_eligibility(
    state: UpdateState, bundle: SessionBundle, update: JiraUpdate,
) -> Optional[str]:
    """Return None if eligible to post; otherwise a one-line reason.

    Ticket-closed is intentionally NOT a gate — we keep logging time
    against tickets even after they're marked done upstream.
    """
    if state == UpdateState.SKIPPED:
        return "row marked SKIPPED upstream"
    if state == UpdateState.FAILED:
        return "row marked FAILED upstream"
    if bundle.real_seconds < 60:
        return f"real_seconds={bundle.real_seconds} below Jira's 60s minimum"
    return None


def _post_worklog(bundle: SessionBundle, update: JiraUpdate) -> str:
    """Post (or recover) a Jira worklog for this (task, window).

    Idempotency: if `pm_updates` already has a row for the same
    (task, window) with a worklog id, we short-circuit and return that
    id without calling Jira again. This is what makes daemon restarts
    and backfill safe.
    """
    from agents.pm_worklog_update import jira_poster

    existing = db.find_existing_worklog(
        task_key=bundle.task_key,
        window_start=bundle.window_start,
        window_end=bundle.window_end,
    )
    if existing is not None:
        prior_id, worklog_id = existing
        log.info(
            "route: worklog already posted for %s [%s→%s] as %s (row %d) — skipping",
            bundle.task_key, bundle.window_start, bundle.window_end, worklog_id, prior_id,
        )
        return worklog_id

    started_utc = _parse_iso_utc(bundle.window_start)
    comment = (update.summary or "").strip() or None
    result = jira_poster.post_worklog(
        task_key=bundle.task_key,
        time_spent_seconds=bundle.real_seconds,
        started_utc=started_utc,
        comment=comment,
    )
    return result.worklog_id


def _parse_iso_utc(iso: str):
    """Parse our canonical 'YYYY-MM-DDTHH:MM:SSZ' back into an aware datetime."""
    from datetime import datetime as _dt
    return _dt.fromisoformat(iso.replace("Z", "+00:00"))


# ──────────────────────── Helpers ──────────────────────────────────────────────


def _render_workflow_input(bundle: SessionBundle) -> str:
    """Compose the user-message blob fed to the Synthesise agent."""
    lines: list[str] = []

    # Ticket context — model uses this to verify sessions belong here.
    lines.append(f"# TICKET: {bundle.task_key}")
    if bundle.pm_task_title:
        lines.append(f"title: {bundle.pm_task_title}")
    if bundle.pm_task_status:
        lines.append(f"status: {bundle.pm_task_status}")
    if bundle.assignee_name:
        lines.append(f"assignee: {bundle.assignee_name}")
    if bundle.pm_task_description:
        lines.append(f"description: {bundle.pm_task_description}")
    lines.append("")

    lines.append(f"# WINDOW: {bundle.window_start} → {bundle.window_end}")
    lines.append(f"real_seconds: {bundle.real_seconds}")
    lines.append("")

    if bundle.earlier_today_summaries:
        lines.append("# EARLIER TODAY (already logged — do not repeat)")
        for s in bundle.earlier_today_summaries:
            lines.append(f"- {s}")
        lines.append("")

    # Session summaries — model must verify each belongs to this ticket.
    lines.append(f"# SESSION SUMMARIES ({len(bundle.sessions)})")
    lines.append("Each summary was classified to this ticket by an upstream model.")
    lines.append("Verify each one actually relates to the ticket before including it.")
    lines.append("")
    for s in bundle.sessions:
        lines.append(f"## session {s.id} — {s.app_name} — {s.duration_s}s")
        if s.top_titles:
            lines.append(f"windows: {', '.join(s.top_titles)}")
        if s.excerpt:
            lines.append(s.excerpt)
        lines.append("")

    return "\n".join(lines)


def _extract_jira_update(step_input) -> JiraUpdate | None:
    """Pull the JiraUpdate produced by the Synthesise step out of step_input."""
    raw = step_input.get_step_content("synthesise") or step_input.previous_step_content
    return _coerce_jira(raw)


def _extract_grounded(step_input) -> GroundedNarrative | None:
    raw = step_input.get_step_content("ground") or step_input.previous_step_content
    if isinstance(raw, GroundedNarrative):
        return raw
    if isinstance(raw, str):
        try:
            return GroundedNarrative.model_validate_json(raw)
        except Exception:
            return None
    if isinstance(raw, dict):
        try:
            return GroundedNarrative.model_validate(raw)
        except Exception:
            return None
    return None


def _coerce_jira(raw) -> JiraUpdate | None:
    if isinstance(raw, JiraUpdate):
        return raw
    if isinstance(raw, str):
        try:
            return JiraUpdate.model_validate_json(raw)
        except Exception:
            return None
    if isinstance(raw, dict):
        try:
            return JiraUpdate.model_validate(raw)
        except Exception:
            return None
    return None


def _extract_outcome(workflow_response) -> RouteOutcome:
    """Pull the RouteOutcome from the workflow's final response."""
    raw = getattr(workflow_response, "content", workflow_response)
    if isinstance(raw, RouteOutcome):
        return raw
    if isinstance(raw, str):
        try:
            return RouteOutcome.model_validate_json(raw)
        except Exception:
            pass
    if isinstance(raw, dict):
        try:
            return RouteOutcome.model_validate(raw)
        except Exception:
            pass
    return RouteOutcome(state=UpdateState.FAILED, reason="could not parse workflow output")


def _decide_state(update: JiraUpdate, coverage: float, bundle: SessionBundle) -> UpdateState:
    """Apply the routing matrix that picks the final UpdateState."""
    if not update.summary.strip():
        return UpdateState.SKIPPED
    if update.confidence < config.PM_WORKLOG_MIN_CONFIDENCE:
        return UpdateState.DRAFTED
    return UpdateState.DRAFTED


def _state_reason(state: UpdateState, update: JiraUpdate, coverage: float) -> str:
    if state == UpdateState.SKIPPED:
        return "no worklog summary produced"
    if state == UpdateState.DRAFTED:
        return f"drafted (confidence={update.confidence:.2f})"
    return state.value


def _open_run_log(task_key: str) -> "tuple[Path, logging.FileHandler]":
    """Attach a FileHandler to every agno logger and the root logger.

    agno uses RichHandler (rich library) with propagate=False, so patching
    sys.stderr or root-logger handlers alone misses it. The fix is to add a
    plain FileHandler directly to each agno logger by name — they'll then
    write every DEBUG/INFO line to both terminal (via RichHandler) and the
    file (via our FileHandler) simultaneously.

    Returns (log_path, file_handler). Caller removes the handler on teardown.
    """
    log_dir = Path(__file__).parent / "logs"
    log_dir.mkdir(parents=True, exist_ok=True)
    ts = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S")
    log_path = log_dir / f"run_{ts}_{task_key}.log"

    fh = logging.FileHandler(log_path, encoding="utf-8")
    fh.setLevel(logging.DEBUG)
    fh.setFormatter(logging.Formatter("%(levelname)s %(name)s — %(message)s"))

    # Root logger catches our structured JSON logs (they propagate up).
    logging.getLogger().addHandler(fh)

    # agno loggers have propagate=False — add directly.
    for name in ("agno", "agno-workflow", "agno-team", "agno-tools"):
        logging.getLogger(name).addHandler(fh)

    return log_path, fh


def _append_run_summary(fh: logging.FileHandler, *, response: object, outcome: "RouteOutcome") -> None:
    """Append final outcome JSON to the log file."""
    try:
        events_raw: list = []
        for ev in (getattr(response, "events", None) or []):
            try:
                events_raw.append(ev.model_dump() if hasattr(ev, "model_dump") else str(ev))
            except Exception:
                events_raw.append(str(ev))
        summary = {
            "outcome_state":  outcome.state.value,
            "outcome_reason": outcome.reason,
            "events":         events_raw,
        }
        fh.stream.write(f"\n--- OUTCOME ---\n{json.dumps(summary, default=str, indent=2)}\n")
        fh.stream.flush()
    except Exception as exc:
        log.warning("pm_worklog: failed to write run summary: %s", exc)
