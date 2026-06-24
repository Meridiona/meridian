"""Embedding backends for the clustering experiment, with on-disk caching.

Caches per (model_tag, content_hash) so re-runs with the same cleaned text are free.
Uses sentence-transformers (torch/MPS) — reliable across bge / Qwen3-Embedding / jina.
"""
from __future__ import annotations
import os, hashlib, json
import numpy as np

CACHE = os.path.join(os.path.dirname(__file__), "cache")
os.makedirs(CACHE, exist_ok=True)

# model_tag -> (hf_id, kwargs, prompt_name/instruction, max_seq_len, batch)
MODELS = {
    "bge-small":   ("BAAI/bge-small-en-v1.5", {}, None, 512, 32),
    "qwen3-0.6b":  ("Qwen/Qwen3-Embedding-0.6B", {}, "clustering", 1024, 4),
    "jina-v3":     ("jinaai/jina-embeddings-v3", {"trust_remote_code": True}, "separation", 1024, 4),
    "e5-small":    ("intfloat/e5-small-v2", {}, None, 512, 32),
}

_LOADED = {}


def _key(tag: str, contents: list[str]) -> str:
    h = hashlib.sha1()
    h.update(tag.encode())
    for c in contents:
        h.update(hashlib.sha1(c.encode("utf-8", "ignore")).digest())
    return os.path.join(CACHE, f"emb_{tag}_{h.hexdigest()[:16]}.npy")


def embed(tag: str, contents: list[str], batch_size: int = 16, cache_tag: str | None = None) -> np.ndarray:
    path = _key(cache_tag or tag, contents)
    if os.path.exists(path):
        return np.load(path)
    from sentence_transformers import SentenceTransformer
    hf_id, kw, instr, max_len, batch = MODELS[tag]
    if tag not in _LOADED:
        print(f"  loading {hf_id} ...", flush=True)
        m = SentenceTransformer(hf_id, device="mps", **kw)
        m.max_seq_length = max_len
        _LOADED[tag] = m
    model = _LOADED[tag]
    batch_size = batch
    enc = {}
    if instr:
        # jina uses `task`; Qwen3 uses `prompt_name`. Try both gracefully.
        if tag == "jina-v3":
            enc["task"] = instr
        elif tag.startswith("qwen3"):
            enc["prompt"] = "Instruct: Identify the unit of engineering work in this session.\nText: "
    vecs = model.encode(contents, batch_size=batch_size, normalize_embeddings=True,
                        show_progress_bar=True, **enc)
    vecs = np.asarray(vecs, dtype="float32")
    np.save(path, vecs)
    return vecs


def free():
    import gc
    _LOADED.clear()
    gc.collect()
    try:
        import torch
        if torch.backends.mps.is_available():
            torch.mps.empty_cache()
    except Exception:
        pass
