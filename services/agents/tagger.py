"""Tagger — Stage 1 (rules-only).

Reads recently-closed sessions from `meridian.db`, runs a library of fast,
deterministic rules against each one, and writes the results into:

  * `ticket_links`        — best-effort Jira task assignment, regex-driven
  * `session_dimensions`  — multi-label tags (activity, intent, engagement,
                            tool, topic, practice, collaboration)
  * `agent_runs`          — audit row per cycle

No LLM is involved at this stage. Everything is observable from logs:
every rule fire, every dimension assigned, every DB write is logged at
INFO; full session bundles + raw rule output are at DEBUG.

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

sys.path.insert(0, str(Path(__file__).parent.parent))
from agents import db                                     # noqa: E402
from agents import rules as rules_mod                     # noqa: E402
from agents.config import (                               # noqa: E402
    SESSION_BATCH_LIMIT, MIN_LLM_DURATION_S, ONLY_TODAY,
    LOG_DIR, today_start_utc_iso,
)
from agents.rules import (                                # noqa: E402
    RuleHit, discover_rules, run_rules, resolve_hits,
    extract_tickets,
)
from agents.taxonomy import SINGLE_VALUE_DIMENSIONS       # noqa: E402

log = logging.getLogger("tagger")


# ──────────────────────── Logging setup ───────────────────────────────────────
def _configure_logging() -> Path:
    LOG_DIR.mkdir(parents=True, exist_ok=True)
    log_path = LOG_DIR / "tagger.log"
    level = getattr(logging, os.environ.get("LOG_LEVEL", "INFO").upper(), logging.INFO)
    logging.basicConfig(
        level=level,
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
        handlers=[logging.FileHandler(log_path), logging.StreamHandler(sys.stdout)],
    )
    for noisy in ("httpx", "httpcore"):
        logging.getLogger(noisy).setLevel(logging.WARNING)
    log.info("LOG_LEVEL=%s (file=%s)", logging.getLevelName(level), log_path)
    return log_path


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
    valid_task_keys: set[str],
) -> dict:
    """Apply all stage-1 rules to one session and persist results.

    Returns a small report dict for logging / aggregation.
    """
    sid = int(session["id"])
    log.info("─" * 76)
    log.info("Session %s", _session_header(session))

    # 1. Run all rules.
    raw_hits = run_rules(session)
    if not raw_hits:
        log.info("  no rule hits")
    else:
        log.debug("  raw hits: %s", json.dumps([h.__dict__ for h in raw_hits], default=str))

    # 2. Resolve single-value dimension conflicts.
    resolved = resolve_hits(raw_hits)
    log.info("  resolved → %s", _format_resolved(resolved))

    # 3. Persist dimensions.
    written = 0
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

    # 4. Decide ticket link.
    task_key, conf, stype, routing = _pick_ticket_link(session, valid_task_keys)
    if stype:
        db.write_ticket_link(
            conn,
            session_id=sid,
            task_key=task_key,
            confidence=conf,
            session_type=stype,
            routing=routing,
            method="stage1_regex",
        )
        if task_key and routing in ("auto", "queue"):
            db.enqueue_dispatch(
                conn,
                session_id=sid,
                agent_run_id=run_id,
                task_key=task_key,
                provider="jira",
                payload={
                    "routing":      routing,
                    "session_type": stype,
                    "confidence":   conf,
                    "stage":        "stage1_regex",
                },
            )
            log.info("  ticket_links → %s / %s / %.2f  (queued for dispatch)", task_key, routing, conf)
        else:
            log.info("  ticket_links → %s / %s / %.2f", task_key or "∅", routing, conf)
    else:
        log.info("  ticket_links → not decided at stage 1 (deferred)")

    return {
        "session_id":        sid,
        "dimensions_written": written,
        "ticket_decided":    bool(stype),
        "task_key":          task_key,
        "session_type":      stype,
        "routing":           routing,
    }


# ──────────────────────── Cycle entry ─────────────────────────────────────────
def run_once(*, since_iso: str | None = None) -> dict:
    """Pull a batch of unprocessed sessions and tag them with stage-1 rules."""
    log.info("=" * 76)
    log.info("Tagger Stage-1 cycle — %s", datetime.now(timezone.utc).isoformat())

    discover_rules()
    log.info("Loaded %d rules", len(rules_mod.RULE_REGISTRY))
    for name, dim, _fn in rules_mod.RULE_REGISTRY:
        log.debug("  registered rule: %-30s dim=%s", name, dim)

    with db.connection() as conn:
        run_id = db.start_agent_run(conn)

        sessions = db.fetch_unprocessed_sessions(
            conn, SESSION_BATCH_LIMIT, since_iso=since_iso,
        )
        pm_tasks = db.fetch_pm_tasks(conn)
        valid_keys: set[str] = {t["task_key"] for t in pm_tasks}

        log.info(
            "%d session(s) | %d pm_tasks | filter since=%s | batch_limit=%d",
            len(sessions), len(pm_tasks), since_iso or "∅", SESSION_BATCH_LIMIT,
        )

        # Pre-filter trivial overhead: write ticket_links overhead/skip and
        # one engagement=idle dim, no rule run.
        kept: list[dict] = []
        skipped = 0
        for s in sessions:
            if _is_trivial_overhead(s):
                sid = int(s["id"])
                db.write_ticket_link(
                    conn,
                    session_id=sid,
                    task_key=None,
                    confidence=0.0,
                    session_type="overhead",
                    routing="skip",
                    method="stage1_prefilter",
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
                kept.append(s)
        log.info("Pre-filter: %d trivial → overhead/skip; %d kept for rule pass",
                 skipped, len(kept))

        t0 = time.time()
        reports: list[dict] = []
        for idx, s in enumerate(kept, start=1):
            log.info("[%d/%d]", idx, len(kept))
            reports.append(_tag_session(conn, run_id=run_id, session=s, valid_task_keys=valid_keys))
        elapsed = time.time() - t0

        if sessions:
            db.advance_cursor(conn, db.session_id_max(sessions))

        # Counters.
        dims_total       = sum(r["dimensions_written"] for r in reports)
        tickets_decided  = sum(1 for r in reports if r["ticket_decided"])
        auto_tickets     = sum(1 for r in reports if r["routing"] == "auto" and r["task_key"])
        skip_decisions   = sum(1 for r in reports if r["routing"] == "skip")

        db.complete_agent_run(
            conn, run_id, "success",
            sessions_processed=len(sessions),
            summaries_written=0,        # stage 1 doesn't generate narrative summaries
            links_written=skipped + tickets_decided,
            dispatches_queued=auto_tickets,
        )

        log.info("=" * 76)
        log.info("Stage-1 cycle complete: run_id=%d", run_id)
        log.info("  sessions seen          : %d", len(sessions))
        log.info("  pre-filter overhead    : %d", skipped)
        log.info("  rule-pass sessions     : %d", len(kept))
        log.info("  dimensions written     : %d", dims_total)
        log.info("  ticket_links decided   : %d  (auto=%d skip=%d)",
                 tickets_decided, auto_tickets, skip_decisions)
        log.info("  elapsed                : %.2fs", elapsed)

        return {
            "run_id":          run_id,
            "sessions":        len(sessions),
            "kept":            len(kept),
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


def inspect_one(session_id: int, *, dry_run: bool = False, reset: bool = True) -> None:
    """Tag exactly one session and dump every step (input → rules → DB)."""
    _configure_logging()
    discover_rules()
    log.info("Single-session inspection: id=%d  dry_run=%s  reset=%s",
             session_id, dry_run, reset)

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

        # Run rules and print every fire (handler logs at INFO already).
        _print_block(f"RULES — {len(rules_mod.RULE_REGISTRY)} registered, firing")
        raw_hits = run_rules(session)
        if not raw_hits:
            print("  (no rules fired)")
        resolved = resolve_hits(raw_hits)

        _print_block("RESOLVED")
        if not resolved:
            print("  (nothing resolved)")
        else:
            for h in sorted(resolved, key=lambda h: (h.dimension, -h.confidence)):
                marker = "★" if h.dimension in SINGLE_VALUE_DIMENSIONS else " "
                print(f"  {marker} {h.dimension:14s} = {h.value:24s}  conf={h.confidence:.2f}  "
                      f"src={h.source}"
                      + (f"   ({h.explanation})" if h.explanation else ""))

        _print_block("TICKET DECISION")
        candidates = sorted(set(re_extract_tickets(session)))
        matched = [c for c in candidates if c in valid_keys]
        unmatched = [c for c in candidates if c not in valid_keys]
        print(f"  ticket-key candidates seen   : {candidates or '(none)'}")
        print(f"    in pm_tasks  (would match) : {matched or '(none)'}")
        print(f"    not in pm_tasks            : {unmatched or '(none)'}")
        task_key, conf, stype, routing = _pick_ticket_link(session, valid_keys)
        if stype:
            print(f"  → ticket_links: task={task_key or '∅'}  type={stype}  route={routing}  conf={conf:.2f}")
        else:
            print("  → ticket_links: deferred (no decision at stage 1)")

        if dry_run:
            _print_block("DRY-RUN — no DB writes")
            print("  No changes persisted. Re-run without --dry-run to write.")
            return

        # Persist. Use a fresh agent_run for traceability.
        run_id = db.start_agent_run(conn)
        _print_block(f"WRITING TO DB  agent_run_id={run_id}")
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
        if stype:
            db.write_ticket_link(
                conn,
                session_id=session_id,
                task_key=task_key,
                confidence=conf,
                session_type=stype,
                routing=routing,
                method="stage1_regex_inspect",
            )
            print(f"  wrote ticket_links → {task_key or '∅'} / {stype} / {routing} / {conf:.2f}")
            if task_key and routing in ("auto", "queue"):
                db.enqueue_dispatch(
                    conn,
                    session_id=session_id,
                    agent_run_id=run_id,
                    task_key=task_key,
                    provider="jira",
                    payload={
                        "routing":      routing,
                        "session_type": stype,
                        "confidence":   conf,
                        "stage":        "stage1_inspect",
                    },
                )
                print(f"  enqueued dispatch_queue row for {task_key}")
        else:
            print("  ticket_links: not written (deferred)")
        db.complete_agent_run(
            conn, run_id, "success",
            sessions_processed=1,
            summaries_written=0,
            links_written=1 if stype else 0,
            dispatches_queued=1 if (task_key and routing in ("auto", "queue")) else 0,
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
def main() -> None:
    parser = argparse.ArgumentParser(description="Meridian session tagger — Stage 1 (rules)")
    g = parser.add_mutually_exclusive_group()
    g.add_argument("--session", type=int, metavar="ID",
                   help="Tag exactly one session by id and dump every step.")
    g.add_argument("--show", type=int, metavar="ID",
                   help="Read-only: print the session's current DB state.")
    g.add_argument("--list-recent", type=int, metavar="N", nargs="?", const=20,
                   help="List the last N sessions and any tags they carry (default 20).")
    g.add_argument("--once", action="store_true",
                   help="Run a full stage-1 pass over the next batch (default).")

    parser.add_argument("--all-history", action="store_true",
                        help="Disable ONLY_TODAY filter for --once / --list-recent.")
    parser.add_argument("--dry-run", action="store_true",
                        help="With --session: don't persist anything, just show what would happen.")
    parser.add_argument("--no-reset", action="store_true",
                        help="With --session: keep existing dims/ticket_link instead of clearing first.")
    args = parser.parse_args()

    if args.session is not None:
        inspect_one(args.session, dry_run=args.dry_run, reset=not args.no_reset)
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
    summary = run_once(since_iso=since_iso)
    print(json.dumps(summary, indent=2, default=str))


if __name__ == "__main__":
    main()
