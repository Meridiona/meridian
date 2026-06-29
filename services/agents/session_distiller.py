"""
Session Distiller — compress app_sessions into a structured, noise-reduced
activity excerpt for PM worklog synthesis.

85–92% text reduction while preserving named facts (ticket keys, PR numbers,
file paths). 11-stage pipeline: segment → junk gate → prose gate → DF cut →
lexical dedup → SemDeDup → facility-location → entity rescue → format.

OTel span tree:
    distil.run                ← root span per distil_hour / distil_range call
        distil.embed          ← SemDeDup embedding stage
        distil.session        ← one per session (all metadata except session_text)

Public API:
    distil_hour(hour, db_path=None)          -> tuple[str, DistilStats]
    distil_range(start, end, db_path=None)   -> tuple[str, DistilStats]
    evict_embedder()                         -> None
"""
from __future__ import annotations

import gc
import json
import logging
import re
import sqlite3
import time
from collections import defaultdict
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Optional

import numpy as np
from opentelemetry.trace import StatusCode

from agents import model_registry, observability
from agents.time_utils import local_hour_utc_bounds, utc_to_local_hhmm  # noqa: F401 (re-exported)
from agents.config import (
    DISTILLER_DF_FRAC,
    DISTILLER_EXCLUDE_APPS,
    DISTILLER_MIN_SESSION_DUR,
    DISTILLER_SEM_DEDUP_THR,
    MERIDIAN_DB,
)

log    = logging.getLogger(__name__)
tracer = observability.setup("meridian-mlx-server")

# ── Regex patterns ─────────────────────────────────────────────────────────────

NAMED_ENTITY_RE = re.compile(
    r'\b[A-Z]{2,10}-\d+\b'
    r'|(?:PR\s*#?|#)\d{2,5}\b'
    r'|\b\w[\w./\\-]{3,}\.'
    r'(?:rs|py|ts|tsx|js|jsx|md|json|toml|sh|sql|yaml|yml)\b',
    re.I,
)

_JUNK_SUBSTRINGS = (
    "<local-command-caveat>", "local-command-stdout", "ctrl+o to expand",
    "ctrl+o to", "lines (ctrl", "auto mode classifier", "Allowed by auto mode",
    "for agents", "shift+tab to cycle", "+ s ifi", "Type to", "tokens · ",
)

_HOUR_RE           = re.compile(r'^\d{4}-\d{2}-\d{2}T\d{2}$')
_SPINNER_RE        = re.compile(r"Photosynthesizing|Thinking|tokens\)|esc to interrupt|[↓✶✳⠂⏵]")
_TIMESTAMP_ONLY_RE = re.compile(r"^\[\d\d:\d\d:\d\d\]\s*$")
_URL_PREFIX_RE     = re.compile(r"^\[url\]\s*", re.I)
_NONWORD_RE        = re.compile(r"[^a-z0-9]+")
_WORDTOKEN_RE      = re.compile(r"[A-Za-z][A-Za-z']+")
_CODEISH_RE        = re.compile(
    r"[/\\.]|::|->|\b(?:def|fn|let|const|import|cargo|git|npm|python|SELECT|FROM|WHERE)\b"
)
_EXT_RE = re.compile(
    r"\.(?:py|md|sh|json|toml|rs|tsx?|jsx?|lock|txt|ya?ml|cfg|ini|sql|db|env|rb|go|c|h)\b",
    re.I,
)
_SEG_SPLIT_RE = re.compile(r"[•·▸►▶‣⁃◦●○➤➔→⟶»«›‹❯❮|©®✓✦✱✶✳※❘┃│]+|\s{2,}")

_STOP = frozenset(
    "the a an and or but to of in on for with at by from is are was were be been "
    "this that these those it its as into out up down over we i you he she they "
    "have has had do does did not no so if then than when while there here can will "
    "would should could about which who what your my our their let me now".split()
)

_SEG_TRIGGER       = 140
_FLOOR             = 3
_CEIL              = 14
_ENTITY_RESCUE_CAP = 4

# ── Embedding singleton ────────────────────────────────────────────────────────
# MLX-native embedder (mlx_embeddings), not sentence-transformers/torch — keeps
# the runtime lean (no ~2.5 GB torch) and on the single MLX backend, and honours
# the same single-slot eviction discipline as the generative model. The model id
# is resolved from the registry (env-overridable via MERIDIAN_EMBEDDER_ID).
_embedder = None  # cached (model, tokenizer) tuple


def _get_embedder() -> tuple:
    global _embedder
    if _embedder is None:
        import mlx_embeddings
        mid = model_registry.embedder_id()
        log.debug("session_distiller: loading MLX embedder %s", mid)
        _embedder = mlx_embeddings.load(mid)
    return _embedder


def _embed(texts: list[str]) -> np.ndarray:
    """L2-normalized sentence embeddings for ``texts`` via the MLX embedder.

    Batched to bound peak Metal memory — the embedder is single-slot, and one
    forward pass over every span would spike unified memory. ``text_embeds`` come
    back already normalized, so downstream cosine similarity is a plain dot product.
    """
    if not texts:
        return np.zeros((0, 0), dtype=np.float32)
    import mlx.core as mx
    from mlx_embeddings import generate

    model, tok = _get_embedder()
    out: list[np.ndarray] = []
    batch = 16
    for i in range(0, len(texts), batch):
        emb = generate(model, tok, texts=texts[i:i + batch]).text_embeds
        out.append(np.array(emb).astype(np.float32))
        mx.clear_cache()
    return np.concatenate(out, axis=0) if out else np.zeros((0, 0), dtype=np.float32)


def evict_embedder() -> None:
    """Free the embedder's Metal memory. Model reloads lazily on next distil call."""
    global _embedder
    if _embedder is not None:
        _embedder = None
        gc.collect()
        try:
            import mlx.core as mx
            mx.clear_cache()
        except Exception:  # noqa: BLE001 — cache flush is best-effort; non-fatal if mlx absent
            pass
        log.info("session_distiller: embedding model evicted from memory")


@dataclass(frozen=True)
class DistilStats:
    hour:              str    # label — hour string or 'start..end' for ranges
    nsess:             int
    raw_chars:         int
    out_chars:         int
    reduction_pct:     float
    n_after_junk:      int
    n_after_df:        int
    n_after_lex:       int
    n_after_sem:       int
    n_selected:        int
    n_session_rescued: int
    n_entity_rescued:  int
    elapsed_s:         float


# ── Internal helpers ───────────────────────────────────────────────────────────

def _norm(line: str) -> str:
    return _NONWORD_RE.sub(" ", line.lower()).strip()


def _clean_window_title(raw: str) -> str:
    t = re.sub(r"\s+", " ", raw or "").strip()
    if "local-command-caveat" in t or t.lower().startswith("terminal"):
        return "Terminal"
    return t[:50]


def _segment(line: str):
    if len(line) <= _SEG_TRIGGER:
        yield line; return
    for piece in _SEG_SPLIT_RE.split(line):
        p = piece.strip()
        if p: yield p


def _is_hard_junk(line: str) -> bool:
    s = line.strip()
    if len(s) < 18: return True
    if _TIMESTAMP_ONLY_RE.match(s): return True
    if any(j in s for j in _JUNK_SUBSTRINGS): return True
    if _SPINNER_RE.search(s): return True
    letters = sum(c.isalpha() for c in s)
    return letters < 10 or letters / len(s) < 0.45


def _fails_prose_gate(line: str) -> bool:
    s = line.strip()
    tokens = [t.lower() for t in _WORDTOKEN_RE.findall(s)]
    if not tokens: return True
    n_stop = sum(t in _STOP for t in tokens)
    if len(_EXT_RE.findall(s)) >= 3 and n_stop < 2: return True
    if _CODEISH_RE.search(s): return False
    return n_stop < 2 or n_stop / len(tokens) < 0.08


def _floc_pick(spans: list[dict], k: int, V: np.ndarray, idx: dict[int, int]) -> list[dict]:
    """Max-min diversity selection: seed from longest span, greedily pick most distant."""
    if len(spans) <= k: return spans
    rows = [idx[id(s)] for s in spans]
    sub  = V[rows]
    picked = [int(np.argmax([len(s["line"]) for s in spans]))]
    while len(picked) < k:
        best_j, best_dist = -1, -1.0
        for j in range(len(spans)):
            if j in picked: continue
            dist = 1.0 - float(np.max(sub[j] @ sub[picked].T))
            if dist > best_dist: best_dist, best_j = dist, j
        picked.append(best_j)
    return [spans[j] for j in sorted(picked, key=lambda j: spans[j]["t"])]


def _entity_rescue(
    selected: list[dict], discarded_by_sid: dict[int, list[dict]],
) -> tuple[list[dict], int]:
    """Add back entity-bearing spans dropped by floc whose entities aren't covered yet."""
    coverage: dict[int, str] = defaultdict(str)
    for sp in selected:
        coverage[sp["sid"]] += " " + sp["line"].lower()
    rescued: list[dict] = []; total = 0
    for sid, candidates in discarded_by_sid.items():
        covered = coverage[sid]; added = 0
        for sp in candidates:
            if added >= _ENTITY_RESCUE_CAP: break
            if not NAMED_ENTITY_RE.search(sp["line"]): continue
            entities = NAMED_ENTITY_RE.findall(sp["line"])
            if any(e.lower() not in covered for e in entities):
                rescued.append(sp); covered += " " + sp["line"].lower(); added += 1
        total += added
    return selected + rescued, total


def _empty_stats(label: str) -> DistilStats:
    return DistilStats(
        hour=label, nsess=0, raw_chars=0, out_chars=0, reduction_pct=0.0,
        n_after_junk=0, n_after_df=0, n_after_lex=0, n_after_sem=0,
        n_selected=0, n_session_rescued=0, n_entity_rescued=0, elapsed_s=0.0,
    )


# ── Core pipeline ──────────────────────────────────────────────────────────────

def _distil(rows: list, header_prefix: str, stat_label: str) -> tuple[str, DistilStats]:
    """Run the full compression pipeline on pre-loaded session rows.

    rows columns: id, app_name, started_at, duration_s, window_titles,
                  session_text, task_key, task_session_type
    """
    t_start = time.monotonic()
    nsess     = len(rows)
    raw_chars = sum(len(r[5] or "") for r in rows)

    spans: list[dict] = []
    fallback_by_sid: dict[int, list[tuple[str, str]]] = {}
    # sid → (app, window, hhmm, task_key, task_session_type, started_at, duration_s, session_text_chars)
    session_meta: dict[int, tuple] = {}

    for sid, app, started_at, duration_s, wt_json, session_text, task_key, task_stype in rows:
        try:
            wins = json.loads(wt_json or "[]")
            raw_win = max(wins, key=lambda d: d.get("count", 0)).get("window_name", "") if wins else ""
        except Exception:
            raw_win = ""
        window = _clean_window_title(raw_win)
        session_meta[sid] = (app, window, utc_to_local_hhmm(started_at), task_key or "",
                             task_stype or "", started_at, duration_s, len(session_text or ""))

        fallback: list[tuple[str, str]] = []
        cur_time = utc_to_local_hhmm(started_at)
        for raw_line in (session_text or "").splitlines():
            m = re.match(r"^\[(\d\d:\d\d:\d\d)\]\s*(.*)", raw_line)
            if m:
                cur_time = m.group(1)[:5]; raw_line = m.group(2)
            raw_line = _URL_PREFIX_RE.sub("", raw_line).strip()
            if not raw_line: continue
            for seg in _segment(raw_line):
                letters = sum(c.isalpha() for c in seg)
                if (len(seg) >= 18 and letters >= 12 and letters / len(seg) >= 0.5
                        and not any(j in seg for j in _JUNK_SUBSTRINGS)
                        and not _SPINNER_RE.search(seg)):
                    fallback.append((cur_time, seg))
                if _is_hard_junk(seg) or _fails_prose_gate(seg): continue
                spans.append({"sid": sid, "app": app, "win": window, "t": cur_time, "line": seg})
        fallback_by_sid[sid] = fallback

    n_after_junk = len(spans)

    df: dict[str, set[int]] = defaultdict(set)
    for sp in spans: df[_norm(sp["line"])].add(sp["sid"])
    df_cut = max(3, int(DISTILLER_DF_FRAC * nsess))
    spans = [sp for sp in spans
             if len(df[_norm(sp["line"])]) < df_cut or NAMED_ENTITY_RE.search(sp["line"])]
    n_after_df = len(spans)

    spans.sort(key=lambda s: (s["t"], s["sid"]))
    seen: set[str] = set(); lex: list[dict] = []
    for sp in spans:
        key = _norm(sp["line"])[:80]
        if key not in seen: seen.add(key); lex.append(sp)
    n_after_lex = len(lex)

    with tracer.start_as_current_span("distil.embed") as embed_span:
        t_embed = time.monotonic()
        texts = [sp["line"] for sp in lex]
        V: np.ndarray = _embed(texts)
        embed_elapsed = round(time.monotonic() - t_embed, 2)
        embed_span.set_attribute("n_spans", len(texts))
        embed_span.set_attribute("elapsed_s", embed_elapsed)

    keep_mask = np.ones(len(lex), dtype=bool); kept: list[int] = []
    for i in range(len(lex)):
        if kept:
            sims = V[i] @ V[kept].T
            if float(sims.max()) > DISTILLER_SEM_DEDUP_THR:
                top = kept[int(sims.argmax())]
                if lex[top]["sid"] == lex[i]["sid"] or not NAMED_ENTITY_RE.search(lex[i]["line"]):
                    keep_mask[i] = False; continue
        kept.append(i)

    sem = [lex[i] for i in range(len(lex)) if keep_mask[i]]
    V_sem = V[[i for i in range(len(lex)) if keep_mask[i]]]
    n_after_sem = len(sem)

    idx_map: dict[int, int] = {id(sp): i for i, sp in enumerate(sem)}
    by_sid: dict[int, list[dict]] = defaultdict(list)
    for sp in sem: by_sid[sp["sid"]].append(sp)

    selected: list[dict] = []
    discarded_by_sid: dict[int, list[dict]] = defaultdict(list)
    n_session_rescued = 0
    session_selected_counts: dict[int, int] = {}
    session_rescued_flags: dict[int, bool] = {}

    for sid, _, started_at, _, _, _, _, _ in rows:
        session_spans = by_sid.get(sid, [])
        if session_spans:
            cap = max(_FLOOR, min(_CEIL, len(session_spans)))
            picked = _floc_pick(session_spans, cap, V_sem, idx_map)
            selected.extend(picked)
            picked_ids = {id(sp) for sp in picked}
            discarded_by_sid[sid] = [sp for sp in session_spans if id(sp) not in picked_ids]
            session_selected_counts[sid] = len(picked)
            session_rescued_flags[sid] = False
        else:
            app, window, t0 = session_meta[sid][:3]
            fb = sorted(fallback_by_sid.get(sid, []), key=lambda x: -len(x[1]))
            seen2: set[str] = set(); added = 0
            for t, line in fb:
                key = _norm(line)[:80]
                if key in seen2: continue
                seen2.add(key)
                selected.append({"sid": sid, "app": app, "win": window, "t": t, "line": line})
                added += 1
                if added >= 2: break
            if added:
                n_session_rescued += 1
                session_rescued_flags[sid] = True
            else:
                selected.append({"sid": sid, "app": app, "win": window, "t": t0,
                                  "line": f"(no readable on-screen text; {app} window '{window}')"})
                session_rescued_flags[sid] = True
            session_selected_counts[sid] = added

    selected, n_entity_rescued = _entity_rescue(selected, discarded_by_sid)

    # entity rescue count per session
    session_entity_counts: dict[int, int] = defaultdict(int)
    rescue_start = sum(session_selected_counts.values())
    for sp in selected[rescue_start:]:
        session_entity_counts[sp["sid"]] += 1

    # emit per-session spans
    for sid, app, started_at, duration_s, _, _, task_key, task_stype in rows:
        app, window, hhmm, tk, tst, sa, dur, txt_chars = session_meta[sid]
        with tracer.start_as_current_span("distil.session") as ss:
            ss.set_attribute("sid",                  sid)
            ss.set_attribute("app_name",             app)
            ss.set_attribute("started_at",           sa)
            ss.set_attribute("duration_s",           dur)
            ss.set_attribute("window_title",         window)
            ss.set_attribute("task_key",             tk)
            ss.set_attribute("task_session_type",    tst)
            ss.set_attribute("session_text_chars",   txt_chars)
            ss.set_attribute("n_spans_selected",     session_selected_counts.get(sid, 0))
            ss.set_attribute("n_entity_rescued",     session_entity_counts.get(sid, 0))
            ss.set_attribute("session_rescued",      session_rescued_flags.get(sid, False))
            ss.set_attribute("distil_label",         stat_label)

    # format
    selected.sort(key=lambda s: (s["t"], s["sid"]))
    threads: list[dict] = []
    for sp in selected:
        if threads and threads[-1]["app"] == sp["app"] and threads[-1]["win"] == sp["win"]:
            threads[-1]["spans"].append(sp)
        else:
            threads.append({"app": sp["app"], "win": sp["win"], "t0": sp["t"], "spans": [sp]})

    app_timeline = " → ".join(t["app"] for t in threads)
    blocks: list[str] = []
    for thread in threads:
        hdr   = f'\n[{thread["t0"]} · {thread["app"]} · {thread["win"] or "—"}]'
        lines = [f'  {sp["line"][:220]}' for sp in thread["spans"]]
        blocks.append("\n".join([hdr] + lines))

    span_start  = utc_to_local_hhmm(rows[0][2]); span_end = utc_to_local_hhmm(rows[-1][2])
    active_mins = sum(r[3] for r in rows) // 60
    body_header = (
        f"=== {header_prefix} · {nsess} sessions · "
        f"{active_mins} min active · {span_start}–{span_end} ===\n"
        f"app timeline: {app_timeline}"
    )
    body = body_header + "\n" + "\n".join(blocks)

    elapsed   = round(time.monotonic() - t_start, 2)
    out_chars = len(body)
    reduction = round(100.0 * (1.0 - out_chars / max(raw_chars, 1)), 1)
    stats = DistilStats(
        hour=stat_label, nsess=nsess, raw_chars=raw_chars, out_chars=out_chars,
        reduction_pct=reduction, n_after_junk=n_after_junk, n_after_df=n_after_df,
        n_after_lex=n_after_lex, n_after_sem=n_after_sem, n_selected=len(selected),
        n_session_rescued=n_session_rescued, n_entity_rescued=n_entity_rescued,
        elapsed_s=elapsed,
    )
    log.info(
        "session_distiller: %s nsess=%d raw=%d out=%d reduction=%.0f%% elapsed=%.1fs",
        stat_label, nsess, raw_chars, out_chars, reduction, elapsed,
        extra={
            "distil_label": stat_label,
            "distil_nsess": nsess,
            "distil_raw_chars": raw_chars,
            "distil_out_chars": out_chars,
            "distil_reduction_pct": reduction,
            "distil_n_after_junk": n_after_junk,
            "distil_n_after_df": n_after_df,
            "distil_n_after_lex": stats.n_after_lex,
            "distil_n_after_sem": stats.n_after_sem,
            "distil_n_selected": stats.n_selected,
            "distil_n_session_rescued": stats.n_session_rescued,
            "distil_n_entity_rescued": stats.n_entity_rescued,
            "distil_elapsed_s": elapsed,
            "distil_input_tokens_est": raw_chars // 4,
            "distil_output_tokens_est": out_chars // 4,
        },
    )
    return body, stats


# ── Data loading ───────────────────────────────────────────────────────────────

def _load_rows(
    db: str, where_clause: str, params: tuple, exclude_coding: bool = False
) -> list:
    """Load OCR/app sessions for the window.

    ``exclude_coding=True`` drops coding-agent rows (Claude Code / Codex / Cursor
    …). Their raw transcripts are NOT meaningful to this OCR-tuned compressor, and
    the worklog pipeline instead folds their clean agent-written ``session_summary``
    in verbatim (see worklog_pipeline). Eval callers keep the default (include).
    """
    ph = ",".join("?" * len(DISTILLER_EXCLUDE_APPS))
    coding_filter = " AND coding_agent_session_uuid IS NULL" if exclude_coding else ""
    con = sqlite3.connect(db)
    try:
        rows = con.execute(
            f"""SELECT id, app_name, started_at, duration_s, window_titles, session_text,
                       COALESCE(task_key,'') as task_key,
                       COALESCE(task_session_type,'') as task_session_type
                  FROM app_sessions
                 WHERE app_name NOT IN ({ph})
                   AND duration_s >= ?
                   AND session_text IS NOT NULL AND LENGTH(session_text) > 40
                   AND {where_clause}{coding_filter}
                 ORDER BY started_at""",
            (*DISTILLER_EXCLUDE_APPS, DISTILLER_MIN_SESSION_DUR, *params),
        ).fetchall()
    finally:
        con.close()
    return rows


def _run_root_span(span, stat_label: str, rows: list, body: str, stats: DistilStats) -> None:
    """Set all attributes on the distil.run root span."""
    span.set_attribute("gen_ai.operation.name", "text_generation")
    span.set_attribute("gen_ai.system",         "mlx")
    span.set_attribute("distil_label",              stat_label)
    span.set_attribute("distil_nsess",              stats.nsess)
    span.set_attribute("distil_session_ids",        json.dumps([r[0] for r in rows]))
    span.set_attribute("distil_raw_chars",          stats.raw_chars)
    span.set_attribute("distil_out_chars",          stats.out_chars)
    span.set_attribute("distil_input_tokens_est",   stats.raw_chars // 4)
    span.set_attribute("distil_output_tokens_est",  stats.out_chars // 4)
    span.set_attribute("distil_reduction_pct",      stats.reduction_pct)
    span.set_attribute("distil_n_after_junk",       stats.n_after_junk)
    span.set_attribute("distil_n_after_df",         stats.n_after_df)
    span.set_attribute("distil_n_after_lex",        stats.n_after_lex)
    span.set_attribute("distil_n_after_sem",        stats.n_after_sem)
    span.set_attribute("distil_n_selected",         stats.n_selected)
    span.set_attribute("distil_n_session_rescued",  stats.n_session_rescued)
    span.set_attribute("distil_n_entity_rescued",   stats.n_entity_rescued)
    span.set_attribute("distil_elapsed_s",          stats.elapsed_s)
    # Scrubbed+capped preview of the compressed session_text the report sees.
    span.set_attribute("distil_body_preview",       observability.preview(body))


# ── Public API ─────────────────────────────────────────────────────────────────



def distil_hour(
    hour: str,
    db_path: Optional[Path] = None,
    exclude_coding: bool = False,
) -> tuple[str, DistilStats]:
    """Compress one calendar hour of sessions into a structured activity excerpt.

    Args:
        hour:    'YYYY-MM-DDTHH' (e.g. '2026-06-22T09').
        db_path: Path to meridian.db; defaults to MERIDIAN_DB from agents.config.
        exclude_coding: drop coding-agent rows (the worklog path folds their
                 verbatim summaries in separately — see _load_rows).

    Returns (body, stats). Returns ('', empty_stats) for an empty hour.
    """
    if not _HOUR_RE.match(hour):
        raise ValueError(f"invalid hour format {hour!r}; expected YYYY-MM-DDTHH")
    try:
        datetime.strptime(hour, "%Y-%m-%dT%H")
    except ValueError:
        raise ValueError(f"invalid calendar hour {hour!r}; must be a real date")
    db = str(db_path or MERIDIAN_DB)
    utc_start, utc_end = local_hour_utc_bounds(hour)
    rows = _load_rows(db, "started_at >= ? AND started_at < ?", (utc_start, utc_end), exclude_coding)
    if not rows:
        log.warning("session_distiller: no sessions for hour=%s", hour)
        return "", _empty_stats(hour)
    with tracer.start_as_current_span("distil.run") as span:
        try:
            body, stats = _distil(rows, f"HOUR {hour[11:]}:00", hour)
            _run_root_span(span, hour, rows, body, stats)
            return body, stats
        except Exception as exc:
            span.set_status(StatusCode.ERROR, str(exc))
            raise


def distil_range(
    start: str,
    end: str,
    db_path: Optional[Path] = None,
    exclude_coding: bool = False,
) -> tuple[str, DistilStats]:
    """Compress sessions between two ISO timestamps into a structured excerpt.

    Bounds are arbitrary — cross-hour or sub-hour windows are supported.
    start is inclusive, end is exclusive.

    Args:
        start:   ISO timestamp, e.g. '2026-06-16T09:30' or '2026-06-16T09'.
        end:     ISO timestamp, e.g. '2026-06-16T11:00' or '2026-06-16T11'.
        db_path: Path to meridian.db; defaults to MERIDIAN_DB from agents.config.

    Returns (body, stats). stats.hour is set to 'start..end'.
    """
    db = str(db_path or MERIDIAN_DB)
    rows = _load_rows(db, "started_at >= ? AND started_at < ?", (start, end), exclude_coding)
    label = f"{start}..{end}"
    if not rows:
        log.warning("session_distiller: no sessions for range %s", label)
        return "", _empty_stats(label)
    with tracer.start_as_current_span("distil.run") as span:
        try:
            body, stats = _distil(rows, f"{start[:13]}..{end[:13]}", label)
            _run_root_span(span, label, rows, body, stats)
            return body, stats
        except Exception as exc:
            span.set_status(StatusCode.ERROR, str(exc))
            raise
