"""Stage 2 — embedding-based session→task matcher.

Runs after Stage 1 has written the dimensions and only when Stage 1's regex
deferred. Combines three signals into one score:

    score(task) = 0.55 * cosine
                + 0.30 * dim_overlap
                + 0.15 * past_vote

cosine        — embedding similarity between session text and task text
dim_overlap   — agreement between Stage-1 dimensions and the task's
                expected_dims (activity / topic / tool weighted blend)
past_vote     — softmax-weighted vote from the K nearest past sessions
                whose ticket_links.task_key was set to a real task

Cold-start renormalisation:
    no past_vote (no history yet)  →  0.65 * cosine + 0.35 * dim_overlap
    no dim_overlap (empty session) →  0.75 * cosine + 0.25 * past_vote

Routing decision:
    auto   — score_top1 ≥ 0.62  AND  score_top1 - score_top2 ≥ 0.08
    queue  — score_top1 ≥ 0.40  (otherwise unsafe to auto-dispatch but
             worth a human glance)
    skip   — anything below
"""
from __future__ import annotations

import json
import logging
import math
import sqlite3
from dataclasses import dataclass, field
from typing import Any

import numpy as np

from agents import db, embeddings as emb, text_for_embedding as tfe

log = logging.getLogger("agents.stage2")


# ────────────────────────── Constants / defaults ──────────────────────────────
SCORE_W_COSINE     = 0.55
SCORE_W_DIM        = 0.30
SCORE_W_PAST       = 0.15

# Renormalised weights when one of the components has no signal.
SCORE_W_COSINE_NO_PAST = 0.65
SCORE_W_DIM_NO_PAST    = 0.35
SCORE_W_COSINE_NO_DIM  = 0.75
SCORE_W_PAST_NO_DIM    = 0.25
SCORE_W_COSINE_ONLY    = 1.0  # nothing else available

AUTO_THRESHOLD     = 0.62
AUTO_GAP           = 0.08
QUEUE_THRESHOLD    = 0.40

# Dimension-overlap blend weights.
DIM_W_ACTIVITY = 0.40
DIM_W_TOPIC    = 0.35
DIM_W_TOOL     = 0.25


# ────────────────────────── Result types ──────────────────────────────────────
@dataclass
class CandidateBreakdown:
    task_key: str
    cosine: float           # rescaled to [0,1] via (cos+1)/2 — max over session samples
    dim_overlap: float      # [0,1]
    past_vote: float        # [0,1]
    score: float            # weighted blend
    best_sample_label: str = ""  # which session sample contributed the max cosine
    raw_cosine: float = 0.0      # unrescaled max cosine (signed, [-1, 1])
    overlap_detail: dict = field(default_factory=dict)


@dataclass
class Stage2Result:
    session_id: int
    top_candidates: list[CandidateBreakdown]
    chosen_task_key: str | None
    confidence: float
    routing: str            # 'auto' | 'queue' | 'skip'
    method: str             # 'stage2_embed' | 'stage2_no_pm_tasks' | 'stage2_unavailable'
    debug: dict = field(default_factory=dict)


# ────────────────────────── Expected-dims derivation ──────────────────────────
_ISSUE_TYPE_TO_ACTIVITY: dict[str, list[str]] = {
    "bug":        ["debugging"],
    "defect":     ["debugging"],
    "story":      ["coding", "planning"],
    "task":       ["coding"],
    "spike":      ["research"],
    "epic":       ["planning"],
    "documentation": ["documentation"],
    "doc":        ["documentation"],
}


def derive_expected_dims(task: dict) -> dict:
    """Heuristic mapping pm_task → expected Stage-1 dimensions.

    Used to compute dim_overlap. Stored on `pm_task_embeddings.expected_dims`
    when a task is embedded, and refreshed on each re-embed.

    Returns a dict like:
        {
          "activity": ["coding", "planning"],
          "topic":    ["meridian", "sqlite", "rust"],
          "tool":     ["vscode"],
        }

    The lists are unweighted — the matcher treats any match as a hit.
    """
    from agents.rules import session_text  # lazy import; module is small
    from agents.rules.tool import URL_HOSTS, APP_TO_TOOL, CLI_TOOL_RE
    from agents.rules.topic import TOPIC_PATTERNS

    issue_type = (task.get("issue_type") or "").strip().lower()
    activities = list(_ISSUE_TYPE_TO_ACTIVITY.get(issue_type, ["coding"]))

    # Run topic regexes against the task title + description.
    text_blob = " ".join([
        task.get("title") or "",
        task.get("description_text") or "",
        task.get("project_key") or "",
    ])
    topics: list[str] = []
    for slug, pattern in TOPIC_PATTERNS:
        if pattern.search(text_blob):
            topics.append(slug)

    tools: list[str] = []
    for pattern, slug in URL_HOSTS:
        if pattern.search(text_blob):
            if slug not in tools:
                tools.append(slug)
    # CLI tool mentions
    for m in CLI_TOOL_RE.finditer(text_blob):
        slug = m.group(1).lower()
        if slug not in tools:
            tools.append(slug)

    return {"activity": activities, "topic": topics, "tool": tools}


# ────────────────────────── Score components ──────────────────────────────────
def _cos_to_unit(cos: float) -> float:
    """Rescale cosine [-1,1] to [0,1] so it composes cleanly with the other terms."""
    return float(max(0.0, min(1.0, (cos + 1.0) * 0.5)))


def _jaccard(a: set[str], b: set[str]) -> float:
    if not a and not b:
        return 0.0
    inter = a & b
    union = a | b
    return len(inter) / len(union) if union else 0.0


def _activity_match(session_dims: dict[str, set[str]], expected: dict | None) -> float:
    if not expected:
        return 0.0
    sess_act = session_dims.get("activity", set())
    exp_act = set(expected.get("activity") or [])
    if not sess_act or not exp_act:
        return 0.0
    return 1.0 if sess_act & exp_act else 0.0


def _dim_overlap_score(
    session_dims: dict[str, set[str]],
    expected: dict | None,
) -> tuple[float, dict]:
    """Returns (score, detail). `detail` is for the inspector view."""
    if not expected:
        return 0.0, {}
    activity_score = _activity_match(session_dims, expected)
    sess_topics = session_dims.get("topic", set())
    exp_topics  = set(expected.get("topic") or [])
    sess_tools  = session_dims.get("tool", set())
    exp_tools   = set(expected.get("tool") or [])
    topic_j = _jaccard(sess_topics, exp_topics)
    tool_j  = _jaccard(sess_tools, exp_tools)
    score = (
        DIM_W_ACTIVITY * activity_score
        + DIM_W_TOPIC * topic_j
        + DIM_W_TOOL  * tool_j
    )
    detail = {
        "activity_match":   bool(activity_score),
        "session_topics":   sorted(sess_topics),
        "expected_topics":  sorted(exp_topics),
        "topic_overlap":    sorted(sess_topics & exp_topics),
        "topic_jaccard":    round(topic_j, 3),
        "session_tools":    sorted(sess_tools),
        "expected_tools":   sorted(exp_tools),
        "tool_overlap":     sorted(sess_tools & exp_tools),
        "tool_jaccard":     round(tool_j, 3),
    }
    return float(score), detail


def _past_vote(
    conn: sqlite3.Connection,
    session_matrix: np.ndarray,
    *,
    session_id: int,
    k: int = 10,
) -> tuple[dict[str, float], list[tuple[int, str, float]]]:
    """Return (votes_per_task_key, debug_neighbors).

    With multi-vec sessions, neighbour similarity is the MaxSim over
    (query_sample, past_sample) pairs. We group by past session_id and keep
    the max — so a session that has *one* sample matching the query strongly
    counts as a strong neighbour even if its other samples are unrelated.

    To avoid the documented self-reinforcing failure mode, we exclude
    ticket_links rows whose `method` was written by Stage 2 itself — only
    Stage-1 regex matches and (eventually) human-confirmed tags vote.
    """
    neighbors = emb.fetch_top_k_similar_sessions(
        conn, session_matrix, k=k * 3, exclude_session_id=session_id
    )
    if not neighbors:
        return {}, []

    ids = [sid for sid, _, _ in neighbors]
    placeholders = ",".join(["?"] * len(ids))
    rows = conn.execute(
        f"""
        SELECT session_id, task_key, method
          FROM ticket_links
         WHERE session_id IN ({placeholders})
           AND task_key IS NOT NULL
           AND (method IS NULL OR method NOT LIKE 'stage2%')
        """,
        ids,
    ).fetchall()
    tagged = {int(r["session_id"]): r["task_key"] for r in rows}

    debug: list[tuple[int, str, float]] = []
    weighted: dict[str, float] = {}
    total = 0.0
    for sid, sim, _ in neighbors:
        if sid not in tagged:
            continue
        if sim <= 0.0:
            continue
        tk = tagged[sid]
        weighted[tk] = weighted.get(tk, 0.0) + sim
        total += sim
        debug.append((sid, tk, sim))
        if len(debug) >= k:
            break
    if total <= 0.0:
        return {}, debug
    return {k: v / total for k, v in weighted.items()}, debug


def _blend_score(
    cosine_unit: float,
    dim_score: float,
    past_score: float,
    *,
    has_dim: bool,
    has_past: bool,
) -> float:
    if has_dim and has_past:
        return SCORE_W_COSINE * cosine_unit + SCORE_W_DIM * dim_score + SCORE_W_PAST * past_score
    if has_dim and not has_past:
        return SCORE_W_COSINE_NO_PAST * cosine_unit + SCORE_W_DIM_NO_PAST * dim_score
    if has_past and not has_dim:
        return SCORE_W_COSINE_NO_DIM * cosine_unit + SCORE_W_PAST_NO_DIM * past_score
    return SCORE_W_COSINE_ONLY * cosine_unit


def _routing_decision(top1: float, top2: float) -> str:
    if top1 >= AUTO_THRESHOLD and (top1 - top2) >= AUTO_GAP:
        return "auto"
    if top1 >= QUEUE_THRESHOLD:
        return "queue"
    return "skip"


# ────────────────────────── Session-dimension reader ──────────────────────────
def _session_dims_grouped(
    conn: sqlite3.Connection, session_id: int
) -> dict[str, set[str]]:
    """Read session_dimensions and return {dim: {value, ...}}."""
    rows = db.fetch_session_dimensions(conn, session_id)
    out: dict[str, set[str]] = {}
    for r in rows:
        out.setdefault(r["dimension"], set()).add(r["value"])
    return out


# ────────────────────────── Main entry ────────────────────────────────────────
def stage2_match(
    conn: sqlite3.Connection,
    session: dict,
    pm_tasks: list[dict],
    *,
    k_top: int = 5,
    k_neighbors: int = 10,
) -> Stage2Result:
    """Embed `session` as a multi-vec matrix, score against `pm_tasks` via
    max-pool cosine, and return a decision."""
    sid = int(session["id"])

    if not pm_tasks:
        return Stage2Result(
            session_id=sid, top_candidates=[], chosen_task_key=None,
            confidence=0.0, routing="skip", method="stage2_no_pm_tasks",
        )

    # 1. Make sure all pm_tasks have an up-to-date embedding (and expected_dims).
    n_embedded = 0
    for t in pm_tasks:
        expected = derive_expected_dims(t)
        _, did = emb.upsert_pm_task_embedding(conn, t, expected_dims=expected)
        if did:
            n_embedded += 1
    log.debug("stage2: re-embedded %d/%d pm_tasks", n_embedded, len(pm_tasks))

    # 2. Embed the session as a multi-vector matrix (one row per OCR sample
    #    + titles + audio).
    session_matrix, sample_labels, _ = emb.upsert_session_embeddings(conn, session)
    if session_matrix.size == 0:
        return Stage2Result(
            session_id=sid, top_candidates=[], chosen_task_key=None,
            confidence=0.0, routing="skip", method="stage2_no_session_text",
        )

    # 3. Cosine against every pm_task — max over session samples per task.
    keys, task_matrix, expected_dims_list = emb.fetch_all_pm_task_embeddings(conn)
    if not keys:
        return Stage2Result(
            session_id=sid, top_candidates=[], chosen_task_key=None,
            confidence=0.0, routing="skip", method="stage2_no_pm_tasks",
        )

    sim_matrix = session_matrix @ task_matrix.T          # (M_samples, N_tasks)
    raw_max_cos = sim_matrix.max(axis=0)                 # (N_tasks,)
    best_sample_idx = sim_matrix.argmax(axis=0)          # (N_tasks,)

    active_keys = {t["task_key"] for t in pm_tasks}
    candidates_idx = [i for i, k in enumerate(keys) if k in active_keys]
    if not candidates_idx:
        return Stage2Result(
            session_id=sid, top_candidates=[], chosen_task_key=None,
            confidence=0.0, routing="skip", method="stage2_no_pm_tasks",
        )

    # 4. Read session dimensions once, group by dim for cheap lookup.
    sess_dims = _session_dims_grouped(conn, sid)
    has_dim = bool(sess_dims.get("topic") or sess_dims.get("tool") or sess_dims.get("activity"))

    # 5. Past-similar-session vote (multi-vec MaxSim, anti-self-reinforce filter inside).
    past_votes, past_debug = _past_vote(conn, session_matrix, session_id=sid, k=k_neighbors)
    has_past = bool(past_votes)

    # 6. Blend per candidate.
    candidates: list[CandidateBreakdown] = []
    for i in candidates_idx:
        key = keys[i]
        raw = float(raw_max_cos[i])
        cos_unit = _cos_to_unit(raw)
        expected = expected_dims_list[i]
        dim_score, dim_detail = _dim_overlap_score(sess_dims, expected)
        past_score = past_votes.get(key, 0.0)
        score = _blend_score(
            cos_unit, dim_score, past_score,
            has_dim=has_dim, has_past=has_past,
        )
        best_label = sample_labels[int(best_sample_idx[i])] if sample_labels else ""
        candidates.append(CandidateBreakdown(
            task_key=key,
            cosine=cos_unit,
            dim_overlap=dim_score,
            past_vote=past_score,
            score=score,
            best_sample_label=best_label,
            raw_cosine=raw,
            overlap_detail=dim_detail,
        ))

    candidates.sort(key=lambda c: -c.score)
    top = candidates[:k_top]
    if not top:
        return Stage2Result(
            session_id=sid, top_candidates=[], chosen_task_key=None,
            confidence=0.0, routing="skip", method="stage2_no_pm_tasks",
        )

    top1 = top[0].score
    top2 = top[1].score if len(top) > 1 else 0.0
    routing = _routing_decision(top1, top2)
    chosen = top[0].task_key if routing != "skip" else None

    debug = {
        "method":       "stage2_embed",
        "n_pm_tasks":   len(active_keys),
        "n_embedded":   n_embedded,
        "n_samples":    int(session_matrix.shape[0]),
        "sample_labels": sample_labels,
        "has_dim":      has_dim,
        "has_past":     has_past,
        "score_top1":   round(top1, 4),
        "score_top2":   round(top2, 4),
        "score_gap":    round(top1 - top2, 4),
        "auto_threshold": AUTO_THRESHOLD,
        "auto_gap":     AUTO_GAP,
        "past_neighbors": [
            {"session_id": s, "task_key": t, "sim": round(sim, 3)}
            for (s, t, sim) in past_debug
        ],
    }

    return Stage2Result(
        session_id=sid,
        top_candidates=top,
        chosen_task_key=chosen,
        confidence=top1,
        routing=routing,
        method="stage2_embed",
        debug=debug,
    )


__all__ = [
    "Stage2Result", "CandidateBreakdown",
    "stage2_match", "derive_expected_dims",
]
