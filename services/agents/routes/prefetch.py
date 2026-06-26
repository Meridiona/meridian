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

import logging
import threading
import time
from pathlib import Path

from fastapi import APIRouter
from opentelemetry import trace
from tqdm.auto import tqdm

from agents import model_registry
from agents._state import prefetch_lock, prefetch_state

log = logging.getLogger("agents.server")

router = APIRouter()

# Monotonic time of the last transfer-rate sample. Used to zero `speed` when a
# download stalls — HF stops calling tqdm.update(), so the last rate would
# otherwise linger and read as if bytes were still flowing.
_last_rate_ts = 0.0


class _SpeedTqdm(tqdm):
    """Captures HuggingFace's own live transfer rate (bytes/sec) into prefetch_state.

    Passed as ``tqdm_class`` to ``snapshot_download`` so the wizard reports the
    ACTUAL download speed measured by huggingface_hub / hf_transfer, rather than a
    client-side estimate from polled byte deltas. Byte bars only (``unit == "B"``);
    best-effort — a progress hiccup must never fail the download.
    """

    def update(self, n: float = 1):  # type: ignore[override]
        ret = super().update(n)
        if getattr(self, "unit", "") == "B":
            self._record_rate()
        return ret

    def _record_rate(self) -> None:
        global _last_rate_ts
        try:
            rate = self.format_dict.get("rate")  # tqdm's smoothed bytes/sec
            with prefetch_lock:
                prefetch_state["speed"] = float(rate) if rate else 0.0
                _last_rate_ts = time.monotonic()
        except Exception:  # noqa: BLE001 — progress is best-effort
            pass


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
    allow_patterns) so the ``_SpeedTqdm`` rate hook and ``hf_transfer`` apply
    uniformly; ``mlx_lm.load`` / ``mlx_embeddings.load`` resolve the files straight
    from the cache afterwards. Only disk is touched — the single-slot runtime
    loads each model lazily on first use.
    """
    from huggingface_hub import snapshot_download
    snapshot_download(spec.model_id, allow_patterns=spec.allow_patterns, tqdm_class=_SpeedTqdm)


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
                    _download_spec(spec)
                    received = _dir_size_bytes(_hf_cache_dir_for(spec.model_id))
                    span.set_attribute("received_bytes", received)
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
    """
    from fastapi.concurrency import run_in_threadpool

    global _last_rate_ts
    specs = list(model_registry.ALL_SPECS)

    with prefetch_lock:
        if prefetch_state["state"] in ("downloading", "done"):
            return dict(prefetch_state)  # idempotent — no duplicate downloads
        _last_rate_ts = 0.0
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
    # denominator at 0 — the download still runs).
    totals: list[int] = []
    for spec in specs:
        try:
            totals.append(await run_in_threadpool(_spec_total_bytes, spec))
        except Exception as exc:  # noqa: BLE001
            totals.append(0)
            log.warning(
                "server: prefetch size-probe failed (bar partially indeterminate)",
                extra={"model_id": spec.model_id, "error": str(exc)},
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

    with prefetch_lock:
        st = dict(prefetch_state)
        models = list(st.get("models", []))
        last_rate_ts = _last_rate_ts
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
        st["received"] = min(agg, st["total"]) if st["total"] > 0 else agg
    # Zero the speed unless a transfer rate was reported in the last few seconds —
    # otherwise a finished/stalled download keeps showing its last MB/s.
    if st["state"] != "downloading" or (time.monotonic() - last_rate_ts) > 3.0:
        st["speed"] = 0.0
    return st
