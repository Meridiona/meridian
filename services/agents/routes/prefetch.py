"""Prefetch routes — /prefetch_model and /prefetch_status.

Eager, registry-driven download of EVERY model the end-to-end pipeline needs
(llm + reranker + embedder — see agents.model_registry), so the onboarding wizard
can fetch the whole set before the user reaches the dashboard and no first
worklog run stalls on a silent mid-pipeline download.

The wizard's Model step calls /prefetch_model once the runtime is up, then polls
/prefetch_status for live progress. Downloads run in one background thread
(sequential, so bandwidth isn't fragmented) and never block the event loop;
progress is shared via prefetch_state in agents._state. `received`/`total` are
aggregate byte counts across all models — the single denominator the wizard's
progress bar renders.
"""
from __future__ import annotations

import asyncio
import logging
import threading
import time
from pathlib import Path

from fastapi import APIRouter
from opentelemetry import trace

from agents import model_registry
from agents._state import app_state, prefetch_lock, prefetch_state

log = logging.getLogger("agents.server")

router = APIRouter()

# Speed is derived from the growth of on-disk `received` between /prefetch_status
# polls — NOT from a tqdm hook. hf_xet (the Xet accelerator these models use) does
# its download in Rust and bypasses the classic tqdm bar, so a tqdm_class never
# fires; the byte-delta approach works for any backend. These hold the last
# (received, monotonic-time) sample to diff against.
_last_recv = 0
_last_recv_ts = 0.0


def _hf_cache_dir_for(model_id: str) -> Path:
    """The HF hub cache directory for `model_id` (where partial + complete blobs land)."""
    from huggingface_hub.constants import HF_HUB_CACHE
    return Path(HF_HUB_CACHE) / ("models--" + model_id.replace("/", "--"))


def _dir_size_bytes(path: Path) -> int:
    """Bytes actually downloaded under `path` (HF blobs + `.incomplete` partials).

    Counts only real files, NEVER the ``snapshots/`` symlinks: HF stores each file
    once in ``blobs/`` and symlinks it into ``snapshots/``, so following the links
    would double-count every finished file and push progress past 100%.
    """
    total = 0
    if path.exists():
        for f in path.rglob("*"):
            try:
                if f.is_file() and not f.is_symlink():
                    total += f.stat().st_size
            except OSError:
                pass  # file may be deleted mid-walk; skip silently
    return total


def _spec_total_bytes(spec: model_registry.ModelSpec) -> int:
    """Authoritative download size for one model: sum HF sibling sizes filtered to
    the spec's load() patterns.

    Computed upfront so the wizard's progress bar has a stable denominator, instead
    of summing concurrent per-file tqdm totals (which lurch as new bars spawn).
    """
    import fnmatch

    from huggingface_hub import HfApi

    info = HfApi().model_info(spec.model_id, files_metadata=True)
    total = 0
    for sib in info.siblings or []:
        if any(fnmatch.fnmatch(sib.rfilename, pat) for pat in spec.allow_patterns):
            total += sib.size or 0
    return total


def _download_spec(spec: model_registry.ModelSpec) -> None:
    """Download one model's weights to the HF cache (no load into memory).

    All specs go through ``snapshot_download`` (filtered to the spec's
    allow_patterns); ``hf-xet`` transparently accelerates the Xet-backed transfers.
    ``mlx_lm.load`` / ``mlx_embeddings.load`` resolve the files straight from the
    cache afterwards. Only disk is touched — the single-slot runtime loads each
    model lazily on first use.
    """
    from huggingface_hub import snapshot_download
    snapshot_download(spec.model_id, allow_patterns=spec.allow_patterns)


def _run_prefetch(specs: list[model_registry.ModelSpec]) -> None:
    """Background worker: download every model in `specs` to the HF cache in order."""
    tracer = trace.get_tracer(__name__)
    with tracer.start_as_current_span("model_prefetch_all") as root:
        root.set_attribute("model_count", len(specs))
        try:
            for i, spec in enumerate(specs):
                with prefetch_lock:
                    prefetch_state["models"][i]["state"] = "downloading"
                with tracer.start_as_current_span("model_prefetch") as span:
                    span.set_attribute("role", spec.role)
                    span.set_attribute("model_id", spec.model_id)
                    try:
                        _download_spec(spec)
                        received = _dir_size_bytes(_hf_cache_dir_for(spec.model_id))
                        span.set_attribute("received_bytes", received)
                    except Exception as exc:
                        span.set_status(trace.Status(trace.StatusCode.ERROR, str(exc)))
                        span.record_exception(exc)
                        with prefetch_lock:
                            prefetch_state["models"][i]["state"] = "error"
                        raise
                with prefetch_lock:
                    row = prefetch_state["models"][i]
                    row["received"] = received or row["total"]
                    row["state"] = "done"
                    prefetch_state["received"] = sum(m["received"] for m in prefetch_state["models"])
                log.info(
                    "server: model prefetch complete",
                    extra={"role": spec.role, "model_id": spec.model_id, "received_bytes": received},
                )
            with prefetch_lock:
                prefetch_state["received"] = (
                    sum(m["received"] for m in prefetch_state["models"]) or prefetch_state["total"]
                )
                prefetch_state["state"] = "done"
                prefetch_state["speed"] = 0.0
            root.set_attribute("received_bytes", prefetch_state["received"])
            log.info("server: all model prefetch complete", extra={"received_bytes": prefetch_state["received"]})
        except Exception as exc:  # noqa: BLE001 — report, never crash the server
            with prefetch_lock:
                prefetch_state["state"] = "error"
                prefetch_state["error"] = str(exc)
                prefetch_state["speed"] = 0.0
            root.set_status(trace.Status(trace.StatusCode.ERROR, str(exc)))
            log.error("server: model prefetch failed", extra={"error": str(exc)})


@router.post("/prefetch_model")
async def prefetch_model() -> dict:
    """Start the eager, registry-driven download of all pipeline models (idempotent).

    The model set is fixed per run (the registry), so re-POSTing while
    `downloading`/`done` returns the live state without spawning a second
    download; an earlier `error` is retried.

    On Apple Intelligence / non-MLX backends, `mlx_module` is None and there
    are no weights to download — return `done` immediately so the wizard
    advances without trying to prefetch.
    """
    if app_state.get("mlx_module") is None:
        return {"state": "done", "received": 0, "total": 0, "speed": 0.0, "error": None, "models": []}

    from fastapi.concurrency import run_in_threadpool

    global _last_recv, _last_recv_ts
    specs = list(model_registry.ALL_SPECS)

    with prefetch_lock:
        if prefetch_state["state"] in ("downloading", "done"):
            return dict(prefetch_state)  # idempotent — no duplicate downloads
        _last_recv, _last_recv_ts = 0, 0.0
        prefetch_state.update(
            state="downloading",
            received=0,
            total=0,
            error=None,
            speed=0.0,
            models=[
                {"role": s.role, "model_id": s.model_id, "loader": s.loader,
                 "received": 0, "total": 0, "state": "pending"}
                for s in specs
            ],
        )

    # Per-model size probe (best-effort; a failed probe just leaves that model's
    # denominator at 0 — the download still runs). All three probes fire in
    # parallel so pre-download latency is bounded by the slowest single HF API
    # call rather than tripling it.
    probe_results = await asyncio.gather(
        *[run_in_threadpool(_spec_total_bytes, s) for s in specs],
        return_exceptions=True,
    )
    totals: list[int] = []
    for spec, r in zip(specs, probe_results):
        if isinstance(r, int):
            totals.append(r)
        else:
            totals.append(0)
            log.warning(
                "server: prefetch size-probe failed (bar partially indeterminate)",
                extra={"model_id": spec.model_id, "error": str(r)},
            )
    with prefetch_lock:
        for i, t in enumerate(totals):
            prefetch_state["models"][i]["total"] = t
        prefetch_state["total"] = sum(totals)

    threading.Thread(target=_run_prefetch, args=(specs,), daemon=True).start()
    log.info(
        "server: model prefetch started",
        extra={"model_count": len(specs), "total_bytes": sum(totals)},
    )
    with prefetch_lock:
        return dict(prefetch_state)


@router.get("/prefetch_status")
async def prefetch_status() -> dict:
    """Live prefetch progress. While downloading, `received` is recomputed from the
    on-disk cache dirs of every model so the aggregate bar advances smoothly.

    The disk walk runs in a threadpool so this endpoint never blocks the event
    loop — the wizard polls it ~1 Hz and must stay responsive even while a model
    is downloading or loading.
    """
    from fastapi.concurrency import run_in_threadpool

    global _last_recv, _last_recv_ts
    with prefetch_lock:
        st = dict(prefetch_state)
        models = list(st.get("models", []))
    if st["state"] == "downloading" and models:
        def _recompute() -> int:
            # Clamp each model's on-disk bytes to its probed total: a cache
            # populated by an earlier full-repo pull can hold more than the
            # patterns-filtered total, which would otherwise push the bar past 100%.
            agg = 0
            for mrow in models:
                size = _dir_size_bytes(_hf_cache_dir_for(mrow["model_id"]))
                cap = mrow.get("total") or 0
                agg += min(size, cap) if cap > 0 else size
            return agg

        agg = await run_in_threadpool(_recompute)
        with prefetch_lock:
            # Re-check state: _run_prefetch may have finished while the disk
            # walk was running. If so, return the authoritative completed state.
            if prefetch_state["state"] != "downloading":
                st = dict(prefetch_state)
            else:
                st["received"] = min(agg, st["total"]) if st["total"] > 0 else agg

    # Derive speed (bytes/sec) from how much `received` grew since the last poll.
    # EMA-smoothed; decays to 0 when bytes stop flowing (delta 0) or the run ends.
    now = time.monotonic()
    with prefetch_lock:
        if st["state"] == "downloading" and _last_recv_ts and now > _last_recv_ts:
            inst = max(0, st["received"] - _last_recv) / (now - _last_recv_ts)
            prev = prefetch_state.get("speed", 0.0) or 0.0
            prefetch_state["speed"] = inst if prev == 0.0 else prev * 0.5 + inst * 0.5
        elif st["state"] != "downloading":
            prefetch_state["speed"] = 0.0
        _last_recv, _last_recv_ts = st["received"], now
        st["speed"] = prefetch_state["speed"]
    return st
