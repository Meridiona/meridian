"""Tagger — 3-stage session classification pipeline.

Reads recently-closed sessions from `meridian.db` and drives them through:

  Stage 1 — Rule Classifier: fast deterministic rules + ticket-key regex
             (no LLM). Writes session_dimensions and may write ticket_links.
  Stage 2 — Semantic Matcher: bge-small embeddings, cosine + dim_overlap +
             past_vote score → top-K candidates. May finalise ticket_links.
  Stage 3 — Agent Tiebreaker: hermes AIAgent single-shot call, fires only
             when Stage 2 returns routing=queue.

Results are written into:

  * `ticket_links`        — best-effort Jira task assignment
  * `session_dimensions`  — multi-label tags (activity, intent, engagement,
                            tool, topic, practice, collaboration)
  * `agent_runs`          — audit row per cycle

Everything is observable from logs: every rule fire, every dimension
assigned, every DB write is logged at INFO; full session bundles + raw
stage output are at DEBUG.

CLI entry:
    python -m agents.tagger --once

Useful env:
    LOG_LEVEL              INFO | DEBUG (default INFO)
    SESSION_BATCH_LIMIT    int (default 50)
    ONLY_TODAY             "1" / "0" — limit to sessions started today
    MIN_LLM_DURATION_S     unused at stage 1 but the pre-filter still uses it
"""
from __future__ import annotations

import argparse
import json
import logging
import os
import sqlite3
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

from opentelemetry import trace

from agents import db                                     # noqa: E402
from agents import observability                          # noqa: E402

# Claim the process's service.name BEFORE importing semantic_matcher/agent_tiebreaker
# — each of those modules also calls observability.setup at import time.
# First-call-wins on the global TracerProvider, so without this the process
# would end up labelled `meridian-semantic-matcher` in OpenObserve.
tracer = observability.setup("meridian-tagger")

from agents import rules as rules_mod                     # noqa: E402
from agents.config import (                               # noqa: E402
    SESSION_BATCH_LIMIT, MIN_LLM_DURATION_S, ONLY_TODAY,
    LOG_DIR, today_start_utc_iso, default_stages, current_stages,
    stages_from_file, write_stages_override, clear_stages_override,
    STAGE1_ENABLED, STAGE2_ENABLED, STAGE3_ENABLED,
    TAGGER_CONFIG_FILE,
)
from agents.rules import (                                # noqa: E402
    RuleHit, discover_rules, run_rules, resolve_hits,
    extract_tickets,
)
from agents.taxonomy import SINGLE_VALUE_DIMENSIONS       # noqa: E402
from agents.semantic_matcher import (                      # noqa: E402
    semantic_match, SemanticMatchResult, CandidateBreakdown,
)
from agents.agent_tiebreaker import (                     # noqa: E402
    agent_tiebreak, AgentDecision,
    MODE_TIEBREAK, MODE_NO_DIMS, MODE_STANDALONE,
)

log = logging.getLogger("tagger")


# ──────────────────────── Logging setup ───────────────────────────────────────
def _configure_logging() -> Path:
    """Idempotent — wires OTel + JSON logging via observability.setup.

    Kept under the old name (and returning a Path) so the CLI / inspector
    entry points don't have to change. The returned path is the JSONL log
    file under ~/.meridian/logs/.
    """
    global tracer
    tracer = observability.setup("meridian-tagger")
    return LOG_DIR / "meridian-tagger.jsonl"


# ──────────────────────── Pre-filter ───────────────────────────────────────────
def _is_trivial_overhead(s: dict) -> bool:
    duration = int(s.get("duration_s") or 0)
    titles   = s.get("window_titles") or []
    ocr      = s.get("ocr_samples")    or []
    audio    = s.get("audio_snippets") or []
    if duration < MIN_LLM_DURATION_S:
        return True
    if not titles and not ocr and not audio:
        return True
    return False


# ──────────────────────── Session digest helpers ─────────────────────────────
def _truncate(s: object, n: int = 60) -> str:
    text = str(s or "").replace("\n", " ")
    return text if len(text) <= n else text[: n - 1] + "…"


def _session_header(s: dict) -> str:
    titles = s.get("window_titles") or []
    top = ""
    if titles:
        first = titles[0]
        if isinstance(first, dict):
            top = str(first.get("title") or first.get("window_name") or "")
        elif isinstance(first, (list, tuple)) and first:
            top = str(first[0])
        else:
            top = str(first)
    return (
        f"id={s['id']:<5} app={_truncate(s.get('app_name'), 22):<22} "
        f"dur={s.get('duration_s', 0):>4}s "
        f"titles={len(titles):<2} ocr={len(s.get('ocr_samples') or []):<2} "
        f"audio={len(s.get('audio_snippets') or []):<2} "
        f"cat={s.get('category')}/{round(s.get('confidence') or 0.0, 2):<4}"
        f' top="{_truncate(top, 50)}"'
    )


def _format_resolved(hits: list[RuleHit]) -> str:
    """Compact one-line summary of resolved tags grouped by dimension."""
    by_dim: dict[str, list[RuleHit]] = {}
    for h in hits:
        by_dim.setdefault(h.dimension, []).append(h)
    parts: list[str] = []
    for dim in ("activity", "intent", "engagement", "collaboration",
                "tool", "topic", "practice"):
        items = by_dim.get(dim, [])
        if not items:
            continue
        items.sort(key=lambda h: -h.confidence)
        rendered = ",".join(f"{h.value}({h.confidence:.2f})" for h in items[:8])
        parts.append(f"{dim}={rendered}")
    return " | ".join(parts) or "(no hits)"


# ──────────────────────── Ticket decision ─────────────────────────────────────
def _pick_ticket_link(
    session: dict,
    valid_task_keys: set[str],
) -> tuple[str | None, float, str, str]:
    """Decide what to write to ticket_links for this session.

    Returns (task_key, confidence, session_type, routing).
    Stage-1 logic:
      * If a ticket key from pm_tasks appears verbatim in titles/OCR/audio
        → task / auto / 0.95.
      * If something *that looks like a ticket* (regex hit) but isn't in
        pm_tasks → task / skip / 0.0  (logged as "ticket-shaped, no match").
      * Otherwise → no decision yet (the tagger leaves ticket_links untouched
        for this session — Stage 2/3 may fill it in later).
    Returns (None, 0, "", "") to mean "leave it".
    """
    candidates = extract_tickets(session)
    matched = [c for c in candidates if c in valid_task_keys]
    if matched:
        # Take the most-recently-mentioned ticket if multiple match. Order in
        # `extract_tickets` is preserved but de-duped, so first wins.
        return matched[0], 0.95, "task", "auto"
    if candidates:
        # Found something ticket-shaped but unknown — record as task/skip so
        # downstream can see we noticed but couldn't act.
        return None, 0.0, "task", "skip"
    return None, 0.0, "", ""


# ──────────────────────── Per-session pipeline ────────────────────────────────
def _tag_session(
    conn: sqlite3.Connection,
    *,
    run_id: int,
    session: dict,
    pm_tasks: list[dict],
    stages: set[int],
) -> dict:
    """Run the configured stages on one session and persist results.

    `stages` is a subset of {1, 2, 3}. Stage 1 is the Rule Classifier (rules + regex).
    Stage 2 (Semantic Matcher) runs only when Stage 1 deferred (or called in isolation).
    Stage 3 (Agent Tiebreaker) runs only when Stage 2 routes to queue.
    """
    sid = int(session["id"])
    valid_task_keys: set[str] = {t["task_key"] for t in pm_tasks}
    log.info("─" * 76)
    log.info("Session %s", _session_header(session))

    # Continue the Rust ETL trace if the session row carries a traceparent.
    # When it doesn't (older rows, or daemon ran without observability) we
    # fall through to a root span — still useful, just not linked upstream.
    parent_ctx = observability.extract_parent_context(session.get("traceparent"))
    session_span = tracer.start_as_current_span("tagger.session", context=parent_ctx)
    with session_span as span:
        span.set_attribute("session_id", sid)
        if session.get("app_name"):
            span.set_attribute("app_name", str(session["app_name"]))
        if session.get("duration_s") is not None:
            span.set_attribute("duration_s", int(session.get("duration_s") or 0))

        return _tag_session_inner(
            conn,
            run_id=run_id,
            session=session,
            pm_tasks=pm_tasks,
            stages=stages,
            valid_task_keys=valid_task_keys,
        )


def _tag_session_inner(
    conn: sqlite3.Connection,
    *,
    run_id: int,
    session: dict,
    pm_tasks: list[dict],
    stages: set[int],
    valid_task_keys: set[str],
) -> dict:
    """Body of `_tag_session`, split out so the per-session span wraps cleanly."""
    sid = int(session["id"])
    written = 0
    rule_decision: tuple[str | None, float, str, str] = (None, 0.0, "", "")
    semantic_result: SemanticMatchResult | None = None

    # ── Stage 1 ──
    if 1 in stages:
        with tracer.start_as_current_span("tagger.rules.fire") as rules_span:
            raw_hits = run_rules(session)
            rules_span.set_attribute("rule_hits", len(raw_hits))
        if not raw_hits:
            log.info("  no rule hits")
        else:
            log.debug("  raw hits: %s", json.dumps([h.__dict__ for h in raw_hits], default=str))

        resolved = resolve_hits(raw_hits)
        log.info("  resolved → %s", _format_resolved(resolved))

        for h in resolved:
            db.upsert_session_dimension(
                conn,
                session_id=sid,
                dimension=h.dimension,
                value=h.value,
                confidence=h.confidence,
                source=h.source,
            )
            written += 1

        task_key, conf, stype, routing = _pick_ticket_link(session, valid_task_keys)
        rule_decision = (task_key, conf, stype, routing)

        if stype and (task_key or routing == "skip"):
            # Stage 1 made a real decision (matched ticket, or "ticket-shaped
            # but unknown" → skip). Persist it; Stage 2 only kicks in when
            # Stage 1 didn't decide at all.
            db.write_ticket_link(
                conn,
                session_id=sid,
                task_key=task_key,
                confidence=conf,
                session_type=stype,
                routing=routing,
                method="rule_regex",
            )
            if task_key and routing in ("auto", "queue"):
                db.enqueue_dispatch(
                    conn, session_id=sid, agent_run_id=run_id, task_key=task_key,
                    provider="jira",
                    payload={"routing": routing, "session_type": stype,
                             "confidence": conf, "stage": "rule_regex"},
                )
                log.info("  rule ticket → %s / %s / %.2f  (queued for dispatch)",
                         task_key, routing, conf)
            else:
                log.info("  rule ticket → %s / %s / %.2f", task_key or "∅", routing, conf)
        elif not stype:
            log.info("  rule ticket → deferred (no candidate keys visible)")

    # ── Stage 2 ──
    # Only run if stage 1 deferred (no decision at all). If stage 1 already
    # wrote `task/skip` because it saw a ticket-shaped string but couldn't
    # match, that's a final decision — Stage 2 doesn't override.
    stage1_deferred = rule_decision[2] == ""
    stage2_attempted = 2 in stages and stage1_deferred and bool(pm_tasks)
    if stage2_attempted:
        try:
            semantic_result = semantic_match(conn, session, pm_tasks)
        except ImportError as exc:
            log.warning("semantic_matcher unavailable: %s", exc)
            semantic_result = SemanticMatchResult(
                session_id=sid, top_candidates=[], chosen_task_key=None,
                confidence=0.0, routing="skip", method="semantic_unavailable",
            )
        except Exception as exc:
            log.exception("semantic_matcher failed for session %d: %s", sid, exc)
            semantic_result = None

        if semantic_result and semantic_result.method == "semantic_embed":
            top = semantic_result.top_candidates
            log.info("  semantic top-3: %s",
                     ", ".join(f"{c.task_key}={c.score:.2f}" for c in top[:3]))
            log.info("  semantic → %s / %s / %.2f  (gap=%.2f)",
                     semantic_result.chosen_task_key or "∅",
                     semantic_result.routing,
                     semantic_result.confidence,
                     semantic_result.debug.get("score_gap", 0.0))

            # ── Stage 3 (only when Stage 2 wants queue) ──
            agent_result: AgentDecision | None = None
            if 3 in stages and semantic_result.routing == "queue" and top:
                pm_lookup = {t["task_key"]: t for t in pm_tasks}
                from agents.semantic_matcher import _session_dims_grouped
                dims_grouped = _session_dims_grouped(conn, sid)
                # Use MODE_NO_DIMS when Stage 1 was disabled — no rule-derived dims.
                mode = MODE_TIEBREAK if 1 in stages else MODE_NO_DIMS
                agent_result = agent_tiebreak(session, dims_grouped, top, pm_lookup, mode=mode)
                log.info("  agent → %s / %s / %.2f  (%.1fs mode=%s)",
                         agent_result.chosen_task_key or "∅",
                         agent_result.routing,
                         agent_result.confidence,
                         agent_result.elapsed_s,
                         mode)

            # Decide what to persist.
            # Stage 3 verdict wins when the method is "agent_tiebreak" — including
            # when it says null/skip (that means "none of these candidates fit").
            # Only fall back to Stage 2 when Stage 3 was unavailable or returned
            # an invalid/unparseable response.
            if agent_result and agent_result.method == "agent_tiebreak":
                final_task   = agent_result.chosen_task_key
                final_conf   = agent_result.confidence
                final_route  = agent_result.routing
                final_method = "agent_tiebreak"
                final_top = top
            else:
                final_task   = semantic_result.chosen_task_key
                final_conf   = semantic_result.confidence
                final_route  = semantic_result.routing
                final_method = "semantic_embed"
                final_top = top

            db.write_ticket_link(
                conn,
                session_id=sid,
                task_key=final_task,
                confidence=final_conf,
                session_type="task",
                routing=final_route,
                method=final_method,
            )
            if final_task and final_route in ("auto", "queue"):
                db.enqueue_dispatch(
                    conn, session_id=sid, agent_run_id=run_id,
                    task_key=final_task, provider="jira",
                    payload={
                        "routing":      final_route,
                        "session_type": "task",
                        "confidence":   final_conf,
                        "stage":        final_method,
                        "semantic_top": [
                            {"task_key": c.task_key, "score": round(c.score, 4)}
                            for c in final_top[:3]
                        ],
                        "agent_reasoning": (agent_result.reasoning if agent_result else ""),
                    },
                )

    # ── Stage 3 standalone (Stage 1+2 both not attempted) ──
    # Fires when Stage 2 was never attempted (disabled or no tasks) and Stage 1
    # also deferred — i.e. no prior stage produced a decision.
    elif (3 in stages and stage1_deferred and pm_tasks
          and db.fetch_ticket_link(conn, sid) is None):
        pm_lookup = {t["task_key"]: t for t in pm_tasks}
        agent_result = agent_tiebreak(
            session, {}, [], pm_lookup,
            mode=MODE_STANDALONE,
            all_pm_tasks=pm_tasks,
        )
        log.info("  agent standalone → %s / %s / %.2f  (%.1fs)",
                 agent_result.chosen_task_key or "∅",
                 agent_result.routing,
                 agent_result.confidence,
                 agent_result.elapsed_s)
        if agent_result.method == "agent_tiebreak":
            db.write_ticket_link(
                conn,
                session_id=sid,
                task_key=agent_result.chosen_task_key,
                confidence=agent_result.confidence,
                session_type="task",
                routing=agent_result.routing,
                method="agent_standalone",
            )
            for dim, vals in agent_result.dimensions.items():
                for val in vals:
                    db.upsert_session_dimension(
                        conn,
                        session_id=sid,
                        dimension=dim,
                        value=val,
                        confidence=0.75,
                        source="agent_standalone",
                    )
                    written += 1
            if agent_result.chosen_task_key and agent_result.routing in ("auto", "queue"):
                db.enqueue_dispatch(
                    conn, session_id=sid, agent_run_id=run_id,
                    task_key=agent_result.chosen_task_key, provider="jira",
                    payload={
                        "routing":         agent_result.routing,
                        "session_type":    "task",
                        "confidence":      agent_result.confidence,
                        "stage":           "agent_standalone",
                        "agent_reasoning": agent_result.reasoning,
                    },
                )

    # ── Report ──
    final_link = db.fetch_ticket_link(conn, sid)
    return {
        "session_id":         sid,
        "dimensions_written": written,
        "ticket_decided":     final_link is not None,
        "task_key":           (final_link or {}).get("task_key"),
        "session_type":       (final_link or {}).get("session_type", ""),
        "routing":            (final_link or {}).get("routing", ""),
        "semantic_method":    semantic_result.method if semantic_result else None,
    }


# ──────────────────────── Cycle entry ─────────────────────────────────────────
def run_once(*, since_iso: str | None = None, stages: set[int] | None = None) -> dict:
    """Pull a batch of unprocessed sessions and tag them with the chosen stages.

    `stages` defaults to whatever the STAGE{1,2,3}_ENABLED env flags resolve
    to via `config.default_stages()` — typically all three.
    """
    if stages is None:
        stages = default_stages()
        if not stages:
            log.warning("All stages disabled via STAGE{1,2,3}_ENABLED — nothing to do")
            return {"sessions_processed": 0, "elapsed_s": 0.0, "skipped_all_stages_disabled": True}
    log.info("=" * 76)
    log.info("Tagger cycle (stages=%s) — %s",
             sorted(stages), datetime.now(timezone.utc).isoformat())

    discover_rules()
    log.info("Loaded %d rules", len(rules_mod.RULE_REGISTRY))
    for name, dim, _fn in rules_mod.RULE_REGISTRY:
        log.debug("  registered rule: %-30s dim=%s", name, dim)

    # Single root span for the whole cycle. Per-session spans (created in
    # `_tag_session`) link back to the Rust ETL trace via the session row's
    # `traceparent` column; they do NOT inherit this run_once span as their
    # parent — keeping the agent cycle separate from cross-process traces
    # avoids fanning every ETL trace into one giant tagger run tree.
    with tracer.start_as_current_span("tagger.run_once") as cycle_span:
        cycle_span.set_attribute("stages", sorted(stages))
        with db.connection() as conn:
            run_id = db.start_agent_run(conn)

            sessions = db.fetch_unprocessed_sessions(
                conn, SESSION_BATCH_LIMIT, since_iso=since_iso,
            )
            pm_tasks = db.fetch_pm_tasks(conn)
            valid_keys: set[str] = {t["task_key"] for t in pm_tasks}

            cycle_span.set_attribute("sessions_batch_count", len(sessions))
            cycle_span.set_attribute("pm_tasks_count", len(pm_tasks))

            log.info(
                "%d session(s) | %d pm_tasks | filter since=%s | batch_limit=%d",
                len(sessions), len(pm_tasks), since_iso or "∅", SESSION_BATCH_LIMIT,
            )

            # Single linear pass through sessions in id-ascending order, with
            # cursor advance after EACH session. A SIGTERM mid-batch loses at
            # most the in-flight session; everything completed before is durable.
            # The two paths (trivial-overhead prefilter / full rule+stage pass)
            # both write before they advance the cursor.
            skipped = 0
            kept_count = 0
            reports: list[dict] = []
            t0 = time.time()
            for idx, s in enumerate(sessions, start=1):
                sid = int(s["id"])
                if _is_trivial_overhead(s):
                    db.write_ticket_link(
                        conn,
                        session_id=sid,
                        task_key=None,
                        confidence=0.0,
                        session_type="overhead",
                        routing="skip",
                        method="rule_prefilter",
                    )
                    db.upsert_session_dimension(
                        conn,
                        session_id=sid,
                        dimension="engagement",
                        value="idle",
                        confidence=0.9,
                        source="rule:prefilter_trivial",
                    )
                    skipped += 1
                else:
                    kept_count += 1
                    log.info("[%d/%d kept]", kept_count, len(sessions) - skipped)
                    reports.append(_tag_session(
                        conn, run_id=run_id, session=s,
                        pm_tasks=pm_tasks, stages=stages,
                    ))
                db.advance_cursor(conn, sid)
            elapsed = time.time() - t0
            log.info("Single-pass done: %d trivial-overhead, %d ran rules+stages",
                     skipped, kept_count)

            # Counters.
            dims_total       = sum(r["dimensions_written"] for r in reports)
            tickets_decided  = sum(1 for r in reports if r["ticket_decided"])
            auto_tickets     = sum(1 for r in reports if r["routing"] == "auto" and r["task_key"])
            skip_decisions   = sum(1 for r in reports if r["routing"] == "skip")
            rule_hits        = sum(1 for r in reports if r.get("semantic_method") is None)
            semantic_cands   = sum(1 for r in reports if r.get("semantic_method") == "semantic_embed")
            agent_resolutions = sum(
                1 for r in reports
                if r.get("routing") in ("auto", "queue") and r.get("task_key")
            )

            cycle_span.set_attribute("rule_classifier_hits", rule_hits)
            cycle_span.set_attribute("semantic_matcher_candidates", semantic_cands)
            cycle_span.set_attribute("agent_tiebreaker_resolutions", agent_resolutions)
            cycle_span.set_attribute("tickets_decided", tickets_decided)
            cycle_span.set_attribute("auto_tickets", auto_tickets)
            cycle_span.set_attribute("elapsed_s", elapsed)

            db.complete_agent_run(
                conn, run_id, "success",
                sessions_processed=len(sessions),
                summaries_written=0,        # stage 1 doesn't generate narrative summaries
                links_written=skipped + tickets_decided,
                dispatches_queued=auto_tickets,
            )

            log.info("=" * 76)
            log.info("Rule Classifier cycle complete: run_id=%d", run_id)
            log.info("  sessions seen          : %d", len(sessions))
            log.info("  pre-filter overhead    : %d", skipped)
            log.info("  rule-pass sessions     : %d", kept_count)
            log.info("  dimensions written     : %d", dims_total)
            log.info("  ticket_links decided   : %d  (auto=%d skip=%d)",
                     tickets_decided, auto_tickets, skip_decisions)
            log.info("  elapsed                : %.2fs", elapsed)

            return {
                "run_id":          run_id,
                "sessions":        len(sessions),
                "kept":            kept_count,
                "skipped":         skipped,
                "dimensions":      dims_total,
                "tickets_decided": tickets_decided,
                "auto_tickets":    auto_tickets,
                "elapsed_s":       elapsed,
            }


# ──────────────────────── Single-session inspection ──────────────────────────
def _print_block(title: str) -> None:
    bar = "═" * 80
    print(f"\n{bar}\n{title}\n{bar}")


def _print_session_input(session: dict, pm_tasks: list[dict]) -> None:
    _print_block(f"INPUT  session_id={session['id']}")
    print(f"  app_name        : {session.get('app_name')}")
    print(f"  duration_s      : {session.get('duration_s')}")
    print(f"  started_at      : {session.get('started_at')}")
    print(f"  ended_at        : {session.get('ended_at')}")
    print(f"  category        : {session.get('category')} (conf {session.get('confidence', 0):.2f})")

    titles = session.get("window_titles") or []
    print(f"  window_titles ({len(titles)}):")
    for t in titles[:8]:
        if isinstance(t, dict):
            print(f"    • {t.get('window_name') or t.get('title') or ''}  ×{t.get('count', '?')}")
        elif isinstance(t, (list, tuple)) and t:
            print(f"    • {t[0]}  ×{t[1] if len(t) > 1 else '?'}")
    if len(titles) > 8:
        print(f"    … and {len(titles) - 8} more")

    ocr = session.get("ocr_samples") or []
    print(f"  ocr_samples ({len(ocr)}):")
    for i, s in enumerate(ocr[:5]):
        text = s.get("text", "") if isinstance(s, dict) else str(s)
        print(f"    [{i}] {_truncate(text, 200)}")
    if len(ocr) > 5:
        print(f"    … and {len(ocr) - 5} more")

    audio = session.get("audio_snippets") or []
    print(f"  audio_snippets ({len(audio)}):")
    for s in audio[:3]:
        text = s.get("text", "") if isinstance(s, dict) else str(s)
        print(f"    • {_truncate(text, 200)}")

    keys = ", ".join(t["task_key"] for t in pm_tasks[:8])
    if len(pm_tasks) > 8:
        keys += f", … (+{len(pm_tasks) - 8} more)"
    print(f"\n  pm_tasks ({len(pm_tasks)} candidates): {keys}")


def _print_db_state(conn: sqlite3.Connection, session_id: int) -> None:
    _print_block(f"DB STATE  session_id={session_id}")

    link = db.fetch_ticket_link(conn, session_id)
    if link:
        print(f"  ticket_links: task={link['task_key'] or '∅'}  "
              f"type={link['session_type']}  route={link['routing']}  "
              f"conf={link['confidence']:.2f}  method={link['method']}")
    else:
        print("  ticket_links: (no row)")

    dims = db.fetch_session_dimensions(conn, session_id)
    if not dims:
        print("  session_dimensions: (no rows)")
        return

    by_dim: dict[str, list[dict]] = {}
    for d in dims:
        by_dim.setdefault(d["dimension"], []).append(d)
    print(f"  session_dimensions ({len(dims)} rows):")
    for dim in sorted(by_dim):
        rows = sorted(by_dim[dim], key=lambda r: -r["confidence"])
        for r in rows:
            print(f"    {dim:14s} = {r['value']:24s}  conf={r['confidence']:.2f}  "
                  f"src={r['source']}")


def _print_agent_block(result: AgentDecision) -> None:
    _print_block("AGENT TIEBREAKER")
    print(f"  method   = {result.method}")
    print(f"  model    = {result.debug.get('model')}")
    print(f"  endpoint = {result.debug.get('base_url')}")
    print(f"  elapsed  = {result.elapsed_s:.2f}s")
    if result.method != "agent_tiebreak":
        print(f"  ✗ {result.reasoning}")
        if result.raw_response:
            print(f"  raw response (truncated):\n    {result.raw_response[:500]}")
        return
    print(f"  decision : {result.chosen_task_key or '∅'} / {result.routing} / "
          f"{result.confidence:.2f}")
    print(f"  reasoning: {result.reasoning}")
    auto_floor  = result.debug.get("auto_floor")
    queue_floor = result.debug.get("queue_floor")
    print(f"  thresholds: auto ≥ {auto_floor}, queue ≥ {queue_floor}")


def _print_semantic_block(result: SemanticMatchResult) -> None:
    _print_block("SEMANTIC MATCHER")
    if result.method != "semantic_embed":
        print(f"  method = {result.method}  (no candidates scored)")
        return
    n_samples = result.debug.get("n_samples", 0)
    sample_labels = result.debug.get("sample_labels") or []
    print(f"  candidates scored : {result.debug.get('n_pm_tasks')}  "
          f"(re-embedded {result.debug.get('n_embedded', 0)})")
    print(f"  session samples   : {n_samples} ({', '.join(sample_labels[:8])}"
          + (f", … +{len(sample_labels) - 8}" if len(sample_labels) > 8 else "") + ")")
    print(f"  has_dim={result.debug.get('has_dim')}  has_past={result.debug.get('has_past')}")
    print(f"  score_top1 = {result.debug.get('score_top1')}  "
          f"score_top2 = {result.debug.get('score_top2')}  "
          f"gap = {result.debug.get('score_gap')}")
    auto_t = result.debug.get('auto_threshold', 0.62)
    auto_g = result.debug.get('auto_gap', 0.08)
    print(f"  auto needs    : top1 ≥ {auto_t}  AND  gap ≥ {auto_g}")

    print("\n  rank  task        score   cosine  raw_cos  dim_ovl  past   best_sample   topic_overlap")
    print("  ----  ----------  ------  ------  -------  -------  ----   -----------   ----------------------")
    for i, c in enumerate(result.top_candidates, start=1):
        topics = c.overlap_detail.get("topic_overlap") or []
        topic_s = ",".join(topics[:6])
        print(f"  {i:>4}  {c.task_key:<10}  {c.score:.3f}   {c.cosine:.3f}   "
              f"{c.raw_cosine:+.3f}   {c.dim_overlap:.3f}    {c.past_vote:.2f}   "
              f"{c.best_sample_label:<11}   {topic_s}")

    print(f"\n  decision : {result.chosen_task_key or '∅'} / {result.routing} / "
          f"{result.confidence:.3f}")
    nbrs = result.debug.get("past_neighbors") or []
    if nbrs:
        print(f"  past_session_vote (top {len(nbrs)} similar tagged sessions, "
              f"stage2-tagged excluded):")
        for n in nbrs[:5]:
            print(f"    • session {n['session_id']:>5} → {n['task_key']:<10} sim={n['sim']}")


def inspect_one(
    session_id: int,
    *,
    dry_run: bool = False,
    reset: bool = True,
    stages: set[int] | None = None,
) -> None:
    """Tag exactly one session and dump every step (input → rules → stage 2 → DB)."""
    if stages is None:
        stages = default_stages() or {1}
    _configure_logging()
    discover_rules()
    log.info("Single-session inspection: id=%d  stages=%s  dry_run=%s  reset=%s",
             session_id, sorted(stages), dry_run, reset)

    with db.connection() as conn:
        session = db.fetch_session(conn, session_id)
        if session is None:
            print(f"\n✗ session id={session_id} not found in app_sessions")
            sys.exit(2)
        pm_tasks = db.fetch_pm_tasks(conn)
        valid_keys: set[str] = {t["task_key"] for t in pm_tasks}

        _print_session_input(session, pm_tasks)

        if reset and not dry_run:
            removed_d = db.clear_session_dimensions(conn, session_id)
            removed_t = db.clear_ticket_link(conn, session_id)
            print(f"\n  reset: cleared {removed_d} dimensions, {removed_t} ticket_links")

        resolved: list[RuleHit] = []
        rule_decision: tuple[str | None, float, str, str] = (None, 0.0, "", "")

        # ── Stage 1 ──
        if 1 in stages:
            _print_block(f"RULE CLASSIFIER ({len(rules_mod.RULE_REGISTRY)} registered)")
            raw_hits = run_rules(session)
            if not raw_hits:
                print("  (no rules fired)")
            resolved = resolve_hits(raw_hits)

            _print_block("RULE CLASSIFIER — RESOLVED")
            if not resolved:
                print("  (nothing resolved)")
            else:
                for h in sorted(resolved, key=lambda h: (h.dimension, -h.confidence)):
                    marker = "★" if h.dimension in SINGLE_VALUE_DIMENSIONS else " "
                    print(f"  {marker} {h.dimension:14s} = {h.value:24s}  conf={h.confidence:.2f}  "
                          f"src={h.source}"
                          + (f"   ({h.explanation})" if h.explanation else ""))

            _print_block("RULE CLASSIFIER — TICKET DECISION (regex)")
            candidates = sorted(set(re_extract_tickets(session)))
            matched = [c for c in candidates if c in valid_keys]
            unmatched = [c for c in candidates if c not in valid_keys]
            print(f"  ticket-key candidates seen   : {candidates or '(none)'}")
            print(f"    in pm_tasks  (would match) : {matched or '(none)'}")
            print(f"    not in pm_tasks            : {unmatched or '(none)'}")
            rule_decision = _pick_ticket_link(session, valid_keys)
            task_key, conf, stype, routing = rule_decision
            if stype:
                print(f"  → ticket_links: task={task_key or '∅'}  type={stype}  "
                      f"route={routing}  conf={conf:.2f}")
            else:
                print("  → ticket_links: deferred (no candidate keys, escalating to Stage 2)")

        # ── Stage 2 ──
        stage1_deferred = rule_decision[2] == ""
        semantic_result: SemanticMatchResult | None = None
        agent_result: AgentDecision | None = None
        if 2 in stages and (stage1_deferred or 1 not in stages) and pm_tasks:
            # Persist Stage 1 dimensions FIRST so Stage 2 can read them when
            # computing dim_overlap.
            if not dry_run and resolved:
                for h in resolved:
                    db.upsert_session_dimension(
                        conn,
                        session_id=session_id,
                        dimension=h.dimension,
                        value=h.value,
                        confidence=h.confidence,
                        source=h.source,
                    )
            try:
                semantic_result = semantic_match(conn, session, pm_tasks)
            except ImportError as exc:
                print(f"\n  Semantic Matcher unavailable: {exc}")
                semantic_result = None
            if semantic_result:
                _print_semantic_block(semantic_result)

            # ── Stage 3 — only when Stage 2 wants queue ──
            if (3 in stages
                and semantic_result is not None
                and semantic_result.method == "semantic_embed"
                and semantic_result.routing == "queue"
                and semantic_result.top_candidates):
                pm_lookup = {t["task_key"]: t for t in pm_tasks}
                from agents.semantic_matcher import _session_dims_grouped
                dims_grouped = _session_dims_grouped(conn, session_id)
                mode = MODE_TIEBREAK if 1 in stages else MODE_NO_DIMS
                agent_result = agent_tiebreak(
                    session, dims_grouped, semantic_result.top_candidates, pm_lookup, mode=mode,
                )
                _print_agent_block(agent_result)

        if dry_run:
            _print_block("DRY-RUN — no DB writes")
            print("  No changes persisted. Re-run without --dry-run to write.")
            return

        # ── Persist (Stage 1 dimensions already written above when stage 2 ran). ──
        run_id = db.start_agent_run(conn)
        _print_block(f"WRITING TO DB  agent_run_id={run_id}")
        if not (2 in stages and semantic_result):
            # Stage 1-only path — write dims now.
            for h in resolved:
                db.upsert_session_dimension(
                    conn,
                    session_id=session_id,
                    dimension=h.dimension,
                    value=h.value,
                    confidence=h.confidence,
                    source=h.source,
                )
        print(f"  wrote {len(resolved)} session_dimensions row(s)")

        ticket_written = False
        # Stage 1 decision wins if non-deferred.
        if rule_decision[2]:
            task_key, conf, stype, routing = rule_decision
            db.write_ticket_link(
                conn,
                session_id=session_id,
                task_key=task_key,
                confidence=conf,
                session_type=stype,
                routing=routing,
                method="rule_regex_inspect",
            )
            ticket_written = True
            print(f"  wrote ticket_links (rule) → {task_key or '∅'} / {stype} / {routing} / {conf:.2f}")
            if task_key and routing in ("auto", "queue"):
                db.enqueue_dispatch(
                    conn, session_id=session_id, agent_run_id=run_id,
                    task_key=task_key, provider="jira",
                    payload={"routing": routing, "session_type": stype,
                             "confidence": conf, "stage": "rule_inspect"},
                )
        elif semantic_result and semantic_result.method == "semantic_embed":
            # Stage 3 verdict wins when method is "agent_tiebreak" — including null/skip.
            # Fall back to Stage 2 only when Stage 3 was unavailable or returned
            # an invalid/unparseable response.
            if agent_result and agent_result.method == "agent_tiebreak":
                final_task   = agent_result.chosen_task_key
                final_conf   = agent_result.confidence
                final_route  = agent_result.routing
                final_method = "agent_tiebreak_inspect"
                final_label  = "agent"
            else:
                final_task   = semantic_result.chosen_task_key
                final_conf   = semantic_result.confidence
                final_route  = semantic_result.routing
                final_method = "semantic_embed_inspect"
                final_label  = "semantic"

            db.write_ticket_link(
                conn,
                session_id=session_id,
                task_key=final_task,
                confidence=final_conf,
                session_type="task",
                routing=final_route,
                method=final_method,
            )
            ticket_written = True
            print(f"  wrote ticket_links ({final_label}) → "
                  f"{final_task or '∅'} / {final_route} / {final_conf:.3f}")
            if final_task and final_route in ("auto", "queue"):
                db.enqueue_dispatch(
                    conn, session_id=session_id, agent_run_id=run_id,
                    task_key=final_task, provider="jira",
                    payload={
                        "routing":      final_route,
                        "session_type": "task",
                        "confidence":   final_conf,
                        "stage":        final_method,
                        "stage2_top": [
                            {"task_key": c.task_key, "score": round(c.score, 4)}
                            for c in semantic_result.top_candidates[:3]
                        ],
                        "stage3_reasoning": (agent_result.reasoning if agent_result else ""),
                    },
                )
        elif (3 in stages and stage1_deferred and pm_tasks
              and 2 not in stages and not ticket_written):
            # ── Stage 3 standalone (inspect path) ──
            _print_block("AGENT TIEBREAKER — STANDALONE")
            pm_lookup = {t["task_key"]: t for t in pm_tasks}
            standalone_result = agent_tiebreak(
                session, {}, [], pm_lookup,
                mode=MODE_STANDALONE,
                all_pm_tasks=pm_tasks,
            )
            _print_agent_block(standalone_result)
            if standalone_result.method == "agent_tiebreak":
                db.write_ticket_link(
                    conn,
                    session_id=session_id,
                    task_key=standalone_result.chosen_task_key,
                    confidence=standalone_result.confidence,
                    session_type="task",
                    routing=standalone_result.routing,
                    method="agent_standalone_inspect",
                )
                ticket_written = True
                print(f"  wrote ticket_links (agent standalone) → "
                      f"{standalone_result.chosen_task_key or '∅'} / "
                      f"{standalone_result.routing} / {standalone_result.confidence:.3f}")
                for dim, vals in standalone_result.dimensions.items():
                    for val in vals:
                        db.upsert_session_dimension(
                            conn,
                            session_id=session_id,
                            dimension=dim,
                            value=val,
                            confidence=0.75,
                            source="agent_standalone",
                        )
                print(f"  wrote {sum(len(v) for v in standalone_result.dimensions.values())} "
                      f"standalone dimension(s)")
        else:
            print("  ticket_links: not written (deferred from all stages)")

        db.complete_agent_run(
            conn, run_id, "success",
            sessions_processed=1, summaries_written=0,
            links_written=1 if ticket_written else 0,
            dispatches_queued=0,
        )

        _print_db_state(conn, session_id)


def show_one(session_id: int) -> None:
    """Read-only — print whatever's currently stored for this session."""
    _configure_logging()
    with db.connection() as conn:
        session = db.fetch_session(conn, session_id)
        if session is None:
            print(f"\n✗ session id={session_id} not found in app_sessions")
            sys.exit(2)
        pm_tasks = db.fetch_pm_tasks(conn)
        _print_session_input(session, pm_tasks)
        _print_db_state(conn, session_id)


def list_recent(n: int, *, since_iso: str | None = None) -> None:
    """Print a compact table of the last N sessions and any tags they carry."""
    _configure_logging()
    with db.connection() as conn:
        sessions = db.fetch_recent_sessions(conn, n, since_iso=since_iso)
        if not sessions:
            print("  (no sessions)")
            return
        print(f"\n{'id':>5}  {'app':22s}  {'dur':>5s}  {'titles':>6s}  {'ocr':>4s}  "
              f"{'cat':16s}  has_tag  preview")
        print("-" * 110)
        for s in sessions:
            link = db.fetch_ticket_link(conn, s["id"])
            tag = "—"
            if link:
                tag = f"{link['session_type']}/{link['routing']}"
                if link["task_key"]:
                    tag = f"{link['task_key']}/{link['routing']}"
            top = ""
            titles = s.get("window_titles") or []
            if titles:
                first = titles[0]
                top = (
                    str(first.get("window_name") or first.get("title") or "")
                    if isinstance(first, dict)
                    else str(first[0]) if isinstance(first, (list, tuple)) and first
                    else str(first)
                )
            print(
                f"{s['id']:>5}  {(_truncate(s.get('app_name'), 22)):22s}  "
                f"{s.get('duration_s', 0):>4}s  "
                f"{len(titles):>6d}  {len(s.get('ocr_samples') or []):>4d}  "
                f"{(s.get('category') or '')[:16]:16s}  "
                f"{tag:8s}  {_truncate(top, 60)}"
            )


# `re_extract_tickets` is named to avoid colliding with the imported helper
# (we re-export it for the inspector's "candidates seen" line).
def re_extract_tickets(session: dict) -> list[str]:
    from agents.rules import extract_tickets as _e
    return _e(session)


# ──────────────────────── CLI ─────────────────────────────────────────────────
def _parse_stages(spec: str) -> set[int]:
    """Parse a `--stage` CLI value (e.g. '1,2,3' or '1' or '2,3').

    Empty / unset falls back to STAGE{1,2,3}_ENABLED env flags via
    config.default_stages(). The 'auto' literal also routes there
    explicitly.
    """
    if spec is None or spec.strip() in ("", "auto"):
        return default_stages() or {1}
    out: set[int] = set()
    for piece in spec.split(","):
        piece = piece.strip()
        if not piece:
            continue
        if piece not in ("1", "2", "3"):
            raise ValueError(f"unknown stage {piece!r} (valid: 1, 2, 3)")
        out.add(int(piece))
    return out or default_stages() or {1}


def _print_stages_status() -> None:
    """Show env defaults, override file contents, and the resolved live set."""
    env_set = sorted(default_stages())
    override = stages_from_file()
    resolved = sorted(current_stages())
    print(f"env defaults  (STAGE1_ENABLED={STAGE1_ENABLED}, "
          f"STAGE2_ENABLED={STAGE2_ENABLED}, STAGE3_ENABLED={STAGE3_ENABLED}) "
          f"→ {env_set}")
    if override is None:
        print(f"override file ({TAGGER_CONFIG_FILE}) absent — env defaults win")
    else:
        print(f"override file ({TAGGER_CONFIG_FILE}) → {sorted(override)}")
    print(f"\nresolved live stages = {resolved}"
          + ("   (a running daemon will use this on its next tick)" if resolved else ""))


def _toggle_stage_cli(*, enable: int | None, disable: int | None) -> None:
    """Write the override file based on current state + the requested toggle.

    If no override file exists yet, we seed it from `default_stages()` so
    the user's explicit toggle doesn't accidentally turn off the others.
    """
    base = stages_from_file()
    if base is None:
        base = default_stages()
    flags = {1: 1 in base, 2: 2 in base, 3: 3 in base}
    if enable is not None:
        flags[enable] = True
    if disable is not None:
        flags[disable] = False

    path = write_stages_override(
        stage1=flags[1], stage2=flags[2], stage3=flags[3],
    )
    print(f"✓ wrote {path}")
    _print_stages_status()


def warm_pm_task_embeddings() -> None:
    """One-shot: embed every active pm_task. Useful before the first run."""
    _configure_logging()
    from agents import embeddings as emb_mod
    from agents.semantic_matcher import derive_expected_dims
    with db.connection() as conn:
        tasks = db.fetch_pm_tasks(conn)
        if not tasks:
            print("No pm_tasks to embed.")
            return
        print(f"Embedding {len(tasks)} pm_tasks with {emb_mod.EMBED_MODEL_NAME}…")
        n_new = 0
        for t in tasks:
            expected = derive_expected_dims(t)
            _, did = emb_mod.upsert_pm_task_embedding(conn, t, expected_dims=expected)
            if did:
                n_new += 1
                print(f"  embedded {t['task_key']:<10} expected_dims={expected}")
            else:
                print(f"  cached   {t['task_key']:<10}")
        print(f"\n✓ embedded {n_new}, reused cache for {len(tasks) - n_new}")


def main() -> None:
    parser = argparse.ArgumentParser(description="Meridian session tagger — Stages 1 + 2")
    g = parser.add_mutually_exclusive_group()
    g.add_argument("--session", type=int, metavar="ID",
                   help="Tag exactly one session by id and dump every step.")
    g.add_argument("--show", type=int, metavar="ID",
                   help="Read-only: print the session's current DB state.")
    g.add_argument("--list-recent", type=int, metavar="N", nargs="?", const=20,
                   help="List the last N sessions and any tags they carry (default 20).")
    g.add_argument("--once", action="store_true",
                   help="Run a full pass over the next batch (default).")
    g.add_argument("--embed-tasks", action="store_true",
                   help="One-shot: embed every active pm_task (warm-up before first run).")
    g.add_argument("--enable-stage", type=int, metavar="N", choices=(1, 2, 3),
                   help="Live: write the override file so stage N is ENABLED. "
                        "A running daemon will pick this up on its next tick.")
    g.add_argument("--disable-stage", type=int, metavar="N", choices=(1, 2, 3),
                   help="Live: write the override file so stage N is DISABLED. "
                        "A running daemon will pick this up on its next tick.")
    g.add_argument("--clear-stages-override", action="store_true",
                   help="Delete the override file so the daemon falls back to "
                        "STAGE{1,2,3}_ENABLED env flags.")
    g.add_argument("--stages-status", action="store_true",
                   help="Print the current resolved stage set + override file state.")

    parser.add_argument("--stage", default="auto",
                        help="Comma list of stages to run (e.g. '1', '2', '1,2', '1,2,3'). "
                             "Default 'auto' uses STAGE{1,2,3}_ENABLED env flags from config "
                             "(all on by default). The Agent Tiebreaker still self-gates — "
                             "it only fires when the Semantic Matcher returns routing=queue.")
    parser.add_argument("--all-history", action="store_true",
                        help="Disable ONLY_TODAY filter for --once / --list-recent.")
    parser.add_argument("--dry-run", action="store_true",
                        help="With --session: don't persist anything, just show what would happen.")
    parser.add_argument("--no-reset", action="store_true",
                        help="With --session: keep existing dims/ticket_link instead of clearing first.")
    args = parser.parse_args()

    stages = _parse_stages(args.stage)

    if args.enable_stage is not None or args.disable_stage is not None:
        _toggle_stage_cli(
            enable=args.enable_stage,
            disable=args.disable_stage,
        )
        return
    if args.clear_stages_override:
        path = clear_stages_override()
        if path:
            print(f"✓ removed override file at {path}")
        else:
            print(f"(no override file present at {TAGGER_CONFIG_FILE})")
        _print_stages_status()
        return
    if args.stages_status:
        _print_stages_status()
        return
    if args.embed_tasks:
        warm_pm_task_embeddings()
        return
    if args.session is not None:
        inspect_one(args.session, dry_run=args.dry_run, reset=not args.no_reset, stages=stages)
        return
    if args.show is not None:
        show_one(args.show)
        return
    if args.list_recent is not None:
        since = None if args.all_history else (today_start_utc_iso() if ONLY_TODAY else None)
        list_recent(args.list_recent, since_iso=since)
        return

    # Default: full batch run.
    _configure_logging()
    since_iso = None if args.all_history else (today_start_utc_iso() if ONLY_TODAY else None)
    summary = run_once(since_iso=since_iso, stages=stages)
    print(json.dumps(summary, indent=2, default=str))


if __name__ == "__main__":
    main()
