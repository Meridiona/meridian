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

from agents import text_for_embedding as tfe

log = logging.getLogger("agents.embeddings")

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


# ────────────────────────── Session embeddings ────────────────────────────────
def fetch_session_embedding(
    conn: sqlite3.Connection, session_id: int, *, model: str = EMBED_MODEL_SHORT
) -> tuple[np.ndarray | None, str | None]:
    """Return (vector, text_hash) for a session, or (None, None)."""
    row = conn.execute(
        "SELECT embedding, text_hash FROM session_embeddings WHERE session_id = ? AND model = ?",
        (int(session_id), model),
    ).fetchone()
    if not row:
        return None, None
    return _blob_to_vec(row["embedding"]), row["text_hash"]


def upsert_session_embedding(
    conn: sqlite3.Connection,
    session: dict,
    *,
    force: bool = False,
) -> tuple[np.ndarray, bool]:
    """Embed `session` and store it.

    Returns (vector, embedded) where `embedded` is True if we actually re-encoded
    (False means the cached row was up-to-date and we just returned its vector).
    """
    sid = int(session["id"])
    text = tfe.session_text(session)
    h = tfe.text_hash(text)

    if not force:
        cached, cached_hash = fetch_session_embedding(conn, sid)
        if cached is not None and cached_hash == h and cached.size == EMBED_DIM:
            return cached, False

    vec = encode(text)
    conn.execute(
        """
        INSERT INTO session_embeddings (session_id, model, dim, embedding, text_hash, created_at)
        VALUES (?, ?, ?, ?, ?, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        ON CONFLICT(session_id, model) DO UPDATE SET
            dim        = excluded.dim,
            embedding  = excluded.embedding,
            text_hash  = excluded.text_hash,
            created_at = excluded.created_at
        """,
        (sid, EMBED_MODEL_SHORT, EMBED_DIM, _vec_to_blob(vec), h),
    )
    return vec, True


def fetch_all_session_embeddings(
    conn: sqlite3.Connection,
    *,
    model: str = EMBED_MODEL_SHORT,
    exclude_session_id: int | None = None,
) -> tuple[list[int], np.ndarray]:
    """Return (session_ids, matrix) — matrix shape (N, EMBED_DIM)."""
    sql = "SELECT session_id, embedding FROM session_embeddings WHERE model = ?"
    args: list[Any] = [model]
    if exclude_session_id is not None:
        sql += " AND session_id != ?"
        args.append(int(exclude_session_id))
    rows = conn.execute(sql, args).fetchall()
    if not rows:
        return [], np.zeros((0, EMBED_DIM), dtype=np.float32)
    ids = [int(r["session_id"]) for r in rows]
    mat = np.stack([_blob_to_vec(r["embedding"]) for r in rows], axis=0)
    return ids, mat


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
    query_vec: np.ndarray,
    *,
    k: int = 10,
    exclude_session_id: int | None = None,
) -> list[tuple[int, float]]:
    """Top-k sessions most similar to `query_vec`. Returns [(session_id, sim), ...]."""
    ids, mat = fetch_all_session_embeddings(conn, exclude_session_id=exclude_session_id)
    if not ids:
        return []
    pairs = cosine_top_k(query_vec, mat, k=min(k, len(ids)))
    return [(ids[i], sim) for (i, sim) in pairs]


def fetch_top_k_similar_tasks(
    conn: sqlite3.Connection,
    query_vec: np.ndarray,
    *,
    k: int = 10,
) -> list[tuple[str, float]]:
    """Top-k pm_tasks most similar to `query_vec`."""
    keys, mat, _ = fetch_all_pm_task_embeddings(conn)
    if not keys:
        return []
    pairs = cosine_top_k(query_vec, mat, k=min(k, len(keys)))
    return [(keys[i], sim) for (i, sim) in pairs]


__all__ = [
    "EMBED_MODEL_NAME", "EMBED_MODEL_SHORT", "EMBED_DIM",
    "get_model", "encode", "encode_batch",
    "upsert_session_embedding", "fetch_session_embedding",
    "fetch_all_session_embeddings",
    "upsert_pm_task_embedding", "fetch_pm_task_embedding",
    "fetch_all_pm_task_embeddings",
    "cosine_top_k",
    "fetch_top_k_similar_sessions", "fetch_top_k_similar_tasks",
]
