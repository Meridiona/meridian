"""Embedding model loader + sqlite-backed vector store.

Stage-2 of the tagger uses these primitives to:
  * embed a session and the pm_task list
  * upsert the BLOBs into session_embeddings / pm_task_embeddings
  * fetch top-K nearest sessions / tasks via numpy brute-force cosine

Storage is raw float32 BLOBs in meridian.db. At 1k–100k vectors that's
1.5 MB–150 MB total and a single `M @ q` matmul on Apple Accelerate gives
sub-5ms scoring even at 100k. We'll graduate to the sqlite-vec extension
the moment we cross ~50k vectors *or* hit p95 > 20 ms — the BLOB column
stays authoritative either way.

The model defaults to `BAAI/bge-small-en-v1.5` (384 dims). It's a
retrieval-tuned small encoder that beats MiniLM on MTEB without changing
the dimensionality, so a swap doesn't require schema changes — only a
re-embed (the `model` column on each row makes that safe).
"""
from __future__ import annotations

import logging
import sqlite3
import threading
from typing import Any

import numpy as np

from agents import observability, text_for_embedding as tfe

log = logging.getLogger("agents.embeddings")
# Embeddings is a shared library — `setup` here gives the module its own
# tracer so spans surface as `meridian-embeddings` when there's no active
# caller-supplied span. When the tagger's per-session span is active, this
# tracer still emits child spans under that parent — the SDK uses the
# current Context regardless of which TracerProvider created the Tracer.
tracer = observability.setup("meridian-embeddings")

# ────────────────────────── Model cache ───────────────────────────────────────
EMBED_MODEL_NAME = "BAAI/bge-small-en-v1.5"
EMBED_MODEL_SHORT = "bge-small-en-v1.5"  # what we record in the DB
EMBED_DIM = 384

_model = None
_model_lock = threading.Lock()


def get_model():
    """Return the loaded SentenceTransformer, importing/loading lazily.

    First call costs ~300–500 ms (cold weights); subsequent calls are free.
    Raises ImportError with a clear message if the dep is missing.
    """
    global _model
    if _model is not None:
        return _model
    with _model_lock:
        if _model is not None:
            return _model
        try:
            from sentence_transformers import SentenceTransformer
        except ImportError as exc:
            raise ImportError(
                "Stage 2 needs `sentence-transformers`. "
                "Install with: pip install 'sentence-transformers>=3.0,<4'"
            ) from exc
        log.info("Loading embedding model: %s", EMBED_MODEL_NAME)
        _model = SentenceTransformer(EMBED_MODEL_NAME)
        _model.eval()
        return _model


# ────────────────────────── Encoding helpers ──────────────────────────────────
def encode(text: str) -> np.ndarray:
    """Encode one string to an L2-normalised float32 vector of length EMBED_DIM."""
    return encode_batch([text])[0]


def encode_batch(texts: list[str], *, batch_size: int = 32) -> np.ndarray:
    """Encode N strings; returns (N, EMBED_DIM) float32, L2-normalised.

    `normalize_embeddings=True` so cosine == dot product downstream.
    """
    with tracer.start_as_current_span("embeddings.encode_batch") as span:
        span.set_attribute("batch_size", len(texts))
        span.set_attribute("vector_dim", EMBED_DIM)
        span.set_attribute("model", EMBED_MODEL_SHORT)
        model = get_model()
        out = model.encode(
            texts,
            batch_size=batch_size,
            normalize_embeddings=True,
            convert_to_numpy=True,
            show_progress_bar=False,
        )
        return np.ascontiguousarray(out, dtype=np.float32)


# ────────────────────────── BLOB <-> ndarray ──────────────────────────────────
def _vec_to_blob(vec: np.ndarray) -> bytes:
    return np.ascontiguousarray(vec, dtype=np.float32).tobytes()


def _blob_to_vec(blob: bytes) -> np.ndarray:
    return np.frombuffer(blob, dtype=np.float32)


# ────────────────────────── Session embeddings (multi-sample) ────────────────
def fetch_session_embeddings(
    conn: sqlite3.Connection,
    session_id: int,
    *,
    model: str = EMBED_MODEL_SHORT,
) -> tuple[list[str], np.ndarray, str | None]:
    """Return (sample_labels, matrix, combined_text_hash) for one session.

    Matrix shape: (M, EMBED_DIM). M is the number of samples. Empty session
    returns (`[]`, zeros((0, dim)), None).
    """
    rows = conn.execute(
        """
        SELECT sample_label, embedding, text_hash
          FROM session_embeddings
         WHERE session_id = ? AND model = ?
         ORDER BY sample_idx
        """,
        (int(session_id), model),
    ).fetchall()
    if not rows:
        return [], np.zeros((0, EMBED_DIM), dtype=np.float32), None
    labels = [r["sample_label"] for r in rows]
    mat = np.stack([_blob_to_vec(r["embedding"]) for r in rows], axis=0)
    return labels, mat, rows[0]["text_hash"]


def upsert_session_embeddings(
    conn: sqlite3.Connection,
    session: dict,
    *,
    force: bool = False,
) -> tuple[np.ndarray, list[str], bool]:
    """Embed `session` as a multi-vector matrix and store one row per sample.

    Returns (matrix, sample_labels, embedded). `embedded=True` if we actually
    re-encoded (False means the cached rows were up-to-date).
    """
    sid = int(session["id"])
    samples = tfe.session_text_samples(session)  # [(label, text), ...]
    combined_hash = tfe.text_hash("|".join(t for _, t in samples))

    if not force:
        cached_labels, cached_mat, cached_hash = fetch_session_embeddings(conn, sid)
        if cached_hash == combined_hash and cached_mat.shape[0] == len(samples):
            return cached_mat, cached_labels, False

    texts = [t for _, t in samples]
    matrix = encode_batch(texts)  # (M, dim)

    # Replace all rows for this (session, model). Cleaner than per-row UPSERT
    # because the sample count can change run-to-run.
    conn.execute(
        "DELETE FROM session_embeddings WHERE session_id = ? AND model = ?",
        (sid, EMBED_MODEL_SHORT),
    )
    for idx, ((label, _text), vec) in enumerate(zip(samples, matrix)):
        conn.execute(
            """
            INSERT INTO session_embeddings (
                session_id, model, sample_idx, sample_label, dim, embedding, text_hash, created_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
            """,
            (sid, EMBED_MODEL_SHORT, idx, label, EMBED_DIM, _vec_to_blob(vec), combined_hash),
        )
    return matrix, [s[0] for s in samples], True


def fetch_all_session_samples(
    conn: sqlite3.Connection,
    *,
    model: str = EMBED_MODEL_SHORT,
    exclude_session_id: int | None = None,
) -> tuple[list[int], list[str], np.ndarray]:
    """Flat per-sample dump for past_vote retrieval.

    Returns (session_ids, sample_labels, matrix). matrix shape: (N_total, dim).
    Each row in the matrix corresponds to one sample on one session.
    `session_ids[i]` and `sample_labels[i]` describe row i.
    """
    sql = (
        "SELECT session_id, sample_label, embedding "
        "  FROM session_embeddings "
        " WHERE model = ?"
    )
    args: list[Any] = [model]
    if exclude_session_id is not None:
        sql += " AND session_id != ?"
        args.append(int(exclude_session_id))
    sql += " ORDER BY session_id, sample_idx"
    rows = conn.execute(sql, args).fetchall()
    if not rows:
        return [], [], np.zeros((0, EMBED_DIM), dtype=np.float32)
    sids = [int(r["session_id"]) for r in rows]
    labels = [r["sample_label"] for r in rows]
    mat = np.stack([_blob_to_vec(r["embedding"]) for r in rows], axis=0)
    return sids, labels, mat


# Backwards-compat shims ----------------------------------------------------
# Older call sites used the single-vector shape; keep stubs that return the
# best-of-the-multi-vec representation (max-pooled is the natural choice).
def fetch_session_embedding(
    conn: sqlite3.Connection, session_id: int, *, model: str = EMBED_MODEL_SHORT
) -> tuple[np.ndarray | None, str | None]:
    """Returns the FIRST sample vector for compatibility with old code paths.

    New code should use `fetch_session_embeddings` (plural).
    """
    labels, mat, h = fetch_session_embeddings(conn, session_id, model=model)
    if not labels:
        return None, None
    return mat[0], h


def upsert_session_embedding(
    conn: sqlite3.Connection, session: dict, *, force: bool = False
) -> tuple[np.ndarray, bool]:
    """Compat shim — embeds the session and returns (first_sample_vec, embedded)."""
    mat, _labels, embedded = upsert_session_embeddings(conn, session, force=force)
    return mat[0], embedded


# ────────────────────────── pm_task embeddings ────────────────────────────────
def fetch_pm_task_embedding(
    conn: sqlite3.Connection, task_key: str, *, model: str = EMBED_MODEL_SHORT
) -> tuple[np.ndarray | None, str | None, dict | None]:
    """Return (vector, text_hash, expected_dims_json) for a task, or (None, None, None)."""
    row = conn.execute(
        """
        SELECT embedding, text_hash, expected_dims
          FROM pm_task_embeddings
         WHERE task_key = ? AND model = ?
        """,
        (task_key, model),
    ).fetchone()
    if not row:
        return None, None, None
    import json
    expected = None
    if row["expected_dims"]:
        try:
            expected = json.loads(row["expected_dims"])
        except (TypeError, ValueError):
            expected = None
    return _blob_to_vec(row["embedding"]), row["text_hash"], expected


def upsert_pm_task_embedding(
    conn: sqlite3.Connection,
    task: dict,
    *,
    expected_dims: dict | None = None,
    force: bool = False,
) -> tuple[np.ndarray, bool]:
    """Embed a pm_task and store it. Returns (vector, embedded)."""
    import json
    key = task["task_key"]
    text = tfe.task_text(task)
    h = tfe.text_hash(text)

    if not force:
        cached, cached_hash, _ = fetch_pm_task_embedding(conn, key)
        if cached is not None and cached_hash == h and cached.size == EMBED_DIM:
            return cached, False

    vec = encode(text)
    conn.execute(
        """
        INSERT INTO pm_task_embeddings (
            task_key, model, dim, embedding, text_hash, pm_updated_at, expected_dims, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        ON CONFLICT(task_key, model) DO UPDATE SET
            dim           = excluded.dim,
            embedding     = excluded.embedding,
            text_hash     = excluded.text_hash,
            pm_updated_at = excluded.pm_updated_at,
            expected_dims = excluded.expected_dims,
            created_at    = excluded.created_at
        """,
        (
            key,
            EMBED_MODEL_SHORT,
            EMBED_DIM,
            _vec_to_blob(vec),
            h,
            task.get("updated_at") or "",
            json.dumps(expected_dims) if expected_dims else None,
        ),
    )
    return vec, True


def fetch_all_pm_task_embeddings(
    conn: sqlite3.Connection, *, model: str = EMBED_MODEL_SHORT
) -> tuple[list[str], np.ndarray, list[dict | None]]:
    """Return (task_keys, matrix, expected_dims_list)."""
    import json
    rows = conn.execute(
        """
        SELECT task_key, embedding, expected_dims
          FROM pm_task_embeddings
         WHERE model = ?
         ORDER BY task_key
        """,
        (model,),
    ).fetchall()
    if not rows:
        return [], np.zeros((0, EMBED_DIM), dtype=np.float32), []
    keys = [r["task_key"] for r in rows]
    mat  = np.stack([_blob_to_vec(r["embedding"]) for r in rows], axis=0)
    expected = []
    for r in rows:
        if r["expected_dims"]:
            try:
                expected.append(json.loads(r["expected_dims"]))
            except (TypeError, ValueError):
                expected.append(None)
        else:
            expected.append(None)
    return keys, mat, expected


# ────────────────────────── Cosine retrieval ──────────────────────────────────
def cosine_top_k(query: np.ndarray, matrix: np.ndarray, k: int) -> list[tuple[int, float]]:
    """Return top-k (row_index, similarity) pairs by cosine.

    `query` and `matrix` rows are assumed L2-normalised, so the dot product
    is the cosine similarity directly.
    """
    if matrix.size == 0:
        return []
    sims = matrix @ query
    if k >= matrix.shape[0]:
        order = np.argsort(-sims)
    else:
        # argpartition is faster than full sort for top-k
        idx = np.argpartition(-sims, kth=k - 1)[:k]
        order = idx[np.argsort(-sims[idx])]
    return [(int(i), float(sims[i])) for i in order[:k]]


def fetch_top_k_similar_sessions(
    conn: sqlite3.Connection,
    query_matrix: np.ndarray,
    *,
    k: int = 10,
    exclude_session_id: int | None = None,
) -> list[tuple[int, float, str]]:
    """Top-k *past sessions* most similar to the query session.

    Similarity is `max over (query_sample, past_sample) of cosine` — the
    standard MaxSim score. Returns [(session_id, sim, best_query_label), ...]
    sorted descending.

    `query_matrix` is the multi-vec representation of the active session
    (rows = samples). When you only have a single vector, pass it as a
    `(1, EMBED_DIM)` array.
    """
    if query_matrix.size == 0:
        return []
    sids, _labels, samples = fetch_all_session_samples(
        conn, exclude_session_id=exclude_session_id
    )
    if samples.size == 0:
        return []
    # (M_q, dim) @ (dim, N_total) → (M_q, N_total)
    sim_matrix = query_matrix @ samples.T
    # For each past sample, take the max over query samples (which query
    # sample matched best). Then group by past session_id and keep the max.
    best_per_sample = sim_matrix.max(axis=0)         # (N_total,)
    best_query_idx  = sim_matrix.argmax(axis=0)      # (N_total,)
    best_per_session: dict[int, tuple[float, int]] = {}
    for i, sid in enumerate(sids):
        sim = float(best_per_sample[i])
        prev = best_per_session.get(sid)
        if prev is None or sim > prev[0]:
            best_per_session[sid] = (sim, int(best_query_idx[i]))
    ordered = sorted(best_per_session.items(), key=lambda kv: -kv[1][0])
    out: list[tuple[int, float, str]] = []
    for sid, (sim, _qi) in ordered[:k]:
        out.append((sid, sim, ""))
    return out


def fetch_top_k_similar_tasks(
    conn: sqlite3.Connection,
    query_matrix: np.ndarray,
    *,
    k: int = 10,
) -> list[tuple[str, float]]:
    """Top-k pm_tasks most similar to the multi-vec query session.

    Similarity per task = max over query samples of cosine(sample, task_vec).
    """
    if query_matrix.size == 0:
        return []
    keys, mat, _ = fetch_all_pm_task_embeddings(conn)
    if not keys:
        return []
    sim_matrix = query_matrix @ mat.T   # (M_q, N_tasks)
    best = sim_matrix.max(axis=0)
    order = np.argsort(-best)[:k]
    return [(keys[int(i)], float(best[int(i)])) for i in order]


__all__ = [
    "EMBED_MODEL_NAME", "EMBED_MODEL_SHORT", "EMBED_DIM",
    "get_model", "encode", "encode_batch",
    "upsert_session_embeddings", "fetch_session_embeddings",
    "upsert_session_embedding", "fetch_session_embedding",   # compat
    "fetch_all_session_samples",
    "upsert_pm_task_embedding", "fetch_pm_task_embedding",
    "fetch_all_pm_task_embeddings",
    "cosine_top_k",
    "fetch_top_k_similar_sessions", "fetch_top_k_similar_tasks",
]
