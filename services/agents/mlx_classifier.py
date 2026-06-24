#!/usr/bin/env python3
"""MLX in-process model backend — session classification and summarisation.

Loads the MLX model (Qwen3), manages its lifecycle (lazy load, idle eviction),
and exposes the classification + summarisation functions used by server.py.

Reads a JSON payload from stdin:
    {"session_ids": [int, ...], "meridian_db": str, "traceparent": str | null}

Uses FSM-constrained decoding to guarantee classification responses are always
a valid SessionClassification JSON object.

OTel span hierarchy (when invoked as a script via main()):
    run_task_linker_mlx          ← root span, parented to Rust traceparent
        classify_session         ← one per session_id
            db_fetch
            classifier_input     ← the COMPLETE model input (system + user)
                system_prompt        — classifier skill + context
                recent_context       — 30-min per-ticket continuity prior
                session_block        — the input session being classified
                candidate_tickets    — ranked candidate tickets (★ = today)
            llm_inference
            classifier_output    ← the COMPLETE raw model output
                reasoning            — chain-of-thought that drove the verdict (emitted first)
                category             — the category (+ category_confidence)
                dimensions           — inferred activity tags
                session_summary      — PM-facing prose deliverable

When imported by server.py the same child spans are emitted under whatever
parent span the server establishes (propagated via contextvars).

Requires: outlines[mlxlm]>=1.3, mlx-lm>=0.22 (installed in .venv).
Method tag in results: "mlx_direct".
"""
from __future__ import annotations

import datetime as _dt
import gc
import json
import logging
import os
import sqlite3 as _sqlite3
import threading
import time
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Literal, Optional, Iterator

from opentelemetry import trace
from opentelemetry.trace import StatusCode
from pydantic import BaseModel, Field, field_validator

_SERVICES_DIR = Path(__file__).parent.parent

from agents import observability
from agents._prompts import (
    build_user_message,
    _format_candidates,
    _format_continuity,
    _format_session,
    _CONTINUITY_WINDOW_MIN,
)
from agents._system_context import SYSTEM_CONTEXT

log = logging.getLogger("agents.run_task_linker_mlx")
tracer = observability.setup("meridian-task-linker-mlx")

# Recent-work continuity: only count a prior session toward the continuity block
# if its task link is confident enough to trust (a shaky 0.5 generic match
# shouldn't compound into a continuity nudge). 0.7 sits at the top of the SKILL's
# "generic project-level match" band (0.50-0.65), so this keeps real alignments
# and drops weak guesses. The window length lives in _prompts._CONTINUITY_WINDOW_MIN
# (shared with the prompt label). Override via CONTINUITY_MIN_CONFIDENCE.
_CONTINUITY_MIN_CONFIDENCE = float(os.environ.get("CONTINUITY_MIN_CONFIDENCE", "0.7"))
_MAX_TOKENS = 1024
_TEMPERATURE = 0.0  # greedy decoding — deterministic classification

# Candidate-set policy. When the dev has CONFIRMED a daily plan, restrict the
# classifier's candidate tickets to exactly those planned tickets instead of
# offering every open ticket (the historical "boost-never-filter" behaviour).
# Rationale: a focused candidate set sharpens precision on the day's declared
# work; off-plan work then intentionally falls through to `untracked` — a
# deliberate holding state — rather than being mis-linked onto an unrelated open
# ticket. NOTE: until a recall-recovery stage exists, `untracked` sessions do
# not produce PM worklogs, so off-plan work is not written back while this is on.
#   "1" (default) → plan-only filtering whenever a plan is confirmed
#   "0"           → legacy boost-never-filter (plan tickets floated up, all kept)
# Read once at import — flipping it requires an MLX-server restart. Only ever
# active on days with a confirmed, non-empty plan; unplanned days are unaffected.
_PLAN_ONLY_CANDIDATES = os.environ.get("CLASSIFY_PLAN_ONLY_CANDIDATES", "1") == "1"

# Default model — override with MLX_MODEL_ID env var.
_DEFAULT_MLX_MODEL_ID = "mlx-community/Qwen3.5-2B-OptiQ-4bit"

# Explicit pin — set MLX_MODEL_ID to bypass dynamic selection entirely (eval
# experiments, reproducible benchmarks). When unset, the model is chosen at
# runtime by llm_selector.select_mlx_model_id() based on available compute.
_MLX_MODEL_ID_PIN = os.environ.get("MLX_MODEL_ID")

# Resolved lazily and cached for the process lifetime by _resolve_model_id().
# Kept as a module attribute (not just a function return) so /info, /v1/models,
# and the llm_inference span all report the same, truthful id.
_MLX_MODEL_ID: str | None = None


def _resolve_model_id() -> str:
    """Return the MLX model id — MLX_MODEL_ID env pin or the hardcoded default."""
    global _MLX_MODEL_ID
    if _MLX_MODEL_ID is not None:
        return _MLX_MODEL_ID
    _MLX_MODEL_ID = _MLX_MODEL_ID_PIN or _DEFAULT_MLX_MODEL_ID
    log.info("run_task_linker_mlx: resolved MLX model=%s", _MLX_MODEL_ID)
    return _MLX_MODEL_ID


_SKILL_PATH = (
    _SERVICES_DIR / "skills" / "activity" / "task-classifier" / "SKILL.md"
)



# ---------------------------------------------------------------------------
# Pydantic schema — outlines uses this for FSM-constrained decoding, which
# guarantees the model output is always a valid instance of this class.
# ---------------------------------------------------------------------------

class SessionClassification(BaseModel):
    # FSM-constrained decoding emits these fields in declaration order, so the
    # ORDER is load-bearing. `reasoning` is declared FIRST on purpose: it turns
    # those tokens into genuine chain-of-thought the model produces BEFORE it
    # commits to task_key/session_type/category — the verdict is then conditioned
    # on the reasoning instead of being a blind one-shot match the reasoning only
    # justifies after the fact. The long `session_summary` stays LAST so that if
    # generation ever hits the _MAX_TOKENS ceiling, the full verdict has already
    # been emitted and only the prose deliverable is truncated.
    reasoning: str = Field(
        ..., min_length=1, max_length=600,
        description=(
            "Think FIRST, before deciding. In 1-4 sentences, reason over the "
            "evidence (window titles, OCR/a11y text, file/branch names, recent "
            "work context) toward the classification: does it clearly match one "
            "candidate ticket's scope, is it real work with no matching ticket "
            "(untracked), or is it idle/personal (overhead)? Cite the specific "
            "evidence you used. Bounded so it can't consume the whole generation "
            "budget before the verdict fields below are emitted."
        ),
    )
    task_key: Optional[str] = Field(
        None,
        description="One of the supplied candidate task keys, or null if none fit.",
    )
    confidence: float = Field(
        ..., ge=0.0, le=1.0,
        description="How certain you are. See scoring heuristics for ranges.",
    )
    session_type: Literal["task", "overhead", "untracked"] = Field(
        ...,
        description=(
            "'task' = matched to a ticket; "
            "'overhead' = idle/personal/unrelated — discarded; "
            "'untracked' = real work with no matching ticket — retained."
        ),
    )
    category: Literal[
        "coding", "code_review", "meeting", "communication", "design",
        "documentation", "planning", "deployment_devops", "research",
        "idle_personal",
    ] = Field(
        ...,
        description=(
            "The single best activity category for this session. Derive it from "
            "the evidence (app, window titles, screen content); no category is "
            "supplied in the input."
        ),
    )
    category_confidence: float = Field(
        ..., ge=0.0, le=1.0,
        description="How certain you are about `category` (0.0-1.0).",
    )
    dimensions: dict[str, list[str]] = Field(
        default_factory=dict,
        description=(
            "Inferred activity tags. Keys: activity, intent, engagement, "
            "collaboration, tool, topic, practice. "
            "Values: lowercase snake_case lists. Omit keys with no evidence."
        ),
    )
    session_summary: str = Field(
        ..., min_length=100,
        description=(
            "A factual prose summary of EVERYTHING the user did in this "
            "session, written for downstream project-management updates. "
            "Length is adaptive: aim for ~10 sentences for short trivial "
            "sessions and up to ~40-80 sentences for content-rich sessions; "
            "match depth to the evidence. Past tense, third person. "
            "PRESERVE every SDLC-relevant signal: "
            "(1) specific files, paths, modules touched; "
            "(2) commands, scripts, queries run + their outcome; "
            "(3) errors hit, stack traces, failing tests; "
            "(4) technical decisions made and the alternative considered; "
            "(5) tests written or run + pass/fail; "
            "(6) commits, branches, PRs opened/merged; "
            "(7) blockers and unanswered questions; "
            "(8) external research / docs / Stack Overflow / Claude advice consulted; "
            "(9) validations, manual QA, screenshots reviewed; "
            "(10) design choices, schema changes, migrations. "
            "DO NOT write marketing language, vague claims, or speculation "
            "about future work. Cite the evidence you saw in the session_text — "
            "this is the single source of truth the PM updater will compose from."
        ),
    )

    @field_validator("confidence", "category_confidence", mode="before")
    @classmethod
    def _clamp_unit_interval(cls, v: object) -> float:
        """Clamp a confidence into [0, 1] instead of rejecting it.

        Outlines' FSM constrains the JSON STRUCTURE and TYPE but NOT a float's
        numeric range, so a model can emit e.g. -0.85 or 1.3. Without this,
        model_validate_json() raises on the `ge=0/le=1` bound and the ENTIRE
        classification is lost (observed: loop eval seed 34371 → confidence
        -0.85). Clamping keeps a usable verdict. Falls back to 0.7 for a non-numeric value.
        """
        try:
            return max(0.0, min(1.0, float(v)))  # type: ignore[arg-type]
        except (TypeError, ValueError):
            return 0.7


# ---------------------------------------------------------------------------
# Skill content — read once at module load, injected into every system prompt.
# ---------------------------------------------------------------------------

def _load_skill() -> str:
    try:
        return _SKILL_PATH.read_text(encoding="utf-8")
    except OSError:
        log.warning("run_task_linker_mlx: SKILL.md not found at %s", _SKILL_PATH)
        return ""


_SKILL_CONTENT = _load_skill()

_SYSTEM_PROMPT = (
    SYSTEM_CONTEXT
    + ("\n\n---\n\n" + _SKILL_CONTENT if _SKILL_CONTENT else "")
)


# ---------------------------------------------------------------------------
# Model loading — loaded lazily on first use, evicted when idle.
#
# The MLX model holds ~7 GB of Metal unified memory while resident (measured;
# note `ps`/Activity Monitor RSS does NOT show it). Classification is bursty,
# so we keep the model only while it's being used: load on first inference,
# and evict after MLX_IDLE_EVICT_S of inactivity (server.py runs the evictor).
# `del + gc.collect() + mx.clear_cache()` reclaims the full 7 GB; cold reload
# is ~3 s. `_model_lock` + `_in_flight` guarantee the evictor never frees the
# model out from under an in-flight inference.
# ---------------------------------------------------------------------------

_model_cache: dict[str, Any] = {}
# Tokenizer kept alongside the model so we can render the EXACT chat-template
# prompt (the wire format outlines feeds the model) for the classifier_input span.
_tokenizer_cache: dict[str, Any] = {}
_model_lock = threading.Lock()       # guards _model_cache mutation, _in_flight, _last_used, eviction
_in_flight = 0                       # inferences currently using the model
_last_used = time.monotonic()        # monotonic ts of the last finished inference

# Aggressive default (2 min): the model is present only during active bursts.
# Tune via env without a code change; 0 disables idle eviction entirely.
_IDLE_EVICT_S = float(os.environ.get("MLX_IDLE_EVICT_S", "120"))

# ---------------------------------------------------------------------------
# Prompt-cache for the static system+skill prefix.
#
# Every classification shares the SAME ~3.1k-token system+skill prefix; mlx_lm
# reprocesses the whole prompt each call by default. Caching that prefix's KV and
# reprocessing only the per-session suffix is a large, accuracy-NEUTRAL CPU saving
# — greedy decoding (temp 0) makes cached and uncached output byte-identical
# (verified by services/tests/test_prompt_cache_equivalence.py).
#
# Mechanism: the public trim_prompt_cache multi-turn idiom (NOT private cache
# `.state`). We keep one persistent KV cache plus the token ids it currently
# holds; each call reuses the longest common prefix with the PREVIOUS prompt,
# reprocesses only the divergent suffix, then trims the generated tokens back off
# so the cache again holds exactly the prompt. The runtime common-prefix check is
# what makes this correctness-proof: if anything diverges earlier than the shared
# prefix it simply reprocesses from the first differing token — never reuses KV
# for a token that wasn't actually there. A non-blocking lock guards the shared
# cache: an overlapping call that can't take the lock falls back to an uncached
# full-prompt generation rather than corrupting it. Invalidated on model
# evict/reload (the KV is tied to the resident model instance).
_prompt_cache: "list | None" = None
_prompt_cache_ids: "list[int] | None" = None
_prompt_cache_lock = threading.Lock()
_PROMPT_CACHE_ENABLED = os.environ.get("CLASSIFY_PROMPT_CACHE", "1") == "1"

# ---------------------------------------------------------------------------
# Constrained-generation FSM cache.
#
# outlines compiles the SessionClassification schema into a token-level FSM
# (the OutlinesCoreLogitsProcessor `index`). Measured: building that Generator
# costs ~6 s — and the schema is STATIC, so rebuilding it per session was burning
# ~6 s/classification (≈30% of wall-clock) for nothing. We build it ONCE per
# resident model and reuse it, calling the processor's own reset() before each
# generation to clear the per-sequence FSM state (greedy decoding → reused-FSM
# output is byte-identical to a freshly built one). Tied to the model instance,
# so it's invalidated alongside the KV cache on evict/reload.
_generator_cache: dict[str, tuple] = {}
_generator_lock = threading.Lock()


def _invalidate_prompt_cache() -> None:
    """Drop the per-model caches (prefix KV + compiled FSM generator). Call
    whenever the resident model is freed or swapped — both are valid only for that
    exact model instance.
    """
    global _prompt_cache, _prompt_cache_ids
    _prompt_cache = None
    _prompt_cache_ids = None
    _generator_cache.clear()


def _get_constrained_logits_processors(model: Any) -> "list":
    """Return the cached FSM logits-processor list for SessionClassification,
    building it once per resident model. The processor carries per-generation
    state, so it is reset() before being handed back — making reuse equivalent to
    a freshly built generator while skipping the ~6 s FSM compile each call.
    """
    mid = _resolve_model_id()
    cached = _generator_cache.get(mid)
    if cached is None:
        with _generator_lock:
            cached = _generator_cache.get(mid)  # re-check under lock
            if cached is None:
                from outlines.generator import Generator

                t0 = time.time()
                gen = Generator(model, SessionClassification)
                lp = model.type_adapter.format_output_type(gen.logits_processor)
                _generator_cache[mid] = (gen, lp)
                cached = _generator_cache[mid]
                log.info(
                    "run_task_linker_mlx: compiled classification FSM in %.1fs "
                    "(cached for process lifetime)", time.time() - t0,
                )
    _gen, lp = cached
    # Clear per-sequence FSM state so this generation starts clean.
    reset = getattr(_gen.logits_processor, "reset", None)
    if callable(reset):
        reset()
    for p in lp:
        p_reset = getattr(p, "reset", None)
        if callable(p_reset) and p is not _gen.logits_processor:
            p_reset()
    return lp


def _common_prefix_len(a: "list[int] | None", b: "list[int] | None") -> int:
    """Length of the longest shared leading run of two token-id sequences."""
    if not a or not b:
        return 0
    n = min(len(a), len(b))
    i = 0
    while i < n and a[i] == b[i]:
        i += 1
    return i


_prompt_cache_trimmable: "bool | None" = None  # lazily detected per model


def _cache_offset(prompt_cache: "list") -> int:
    """Current cached sequence length, across cache implementations. Standard
    KVCache exposes `.offset`; batched caches (ArraysCache) expose `.lengths`.
    Returns -1 when neither is available (→ caller must not reuse)."""
    c0 = prompt_cache[0]
    off = getattr(c0, "offset", None)
    if isinstance(off, int):
        return off
    lengths = getattr(c0, "lengths", None)  # ArraysCache: per-batch lengths
    try:
        if lengths is not None:
            return int(max(lengths))
    except (TypeError, ValueError):
        # `lengths` may be non-numeric or malformed on some cache variants;
        # treat as unsupported and fall through to -1 (caller skips reuse).
        pass
    return -1


def _generate_constrained(
    model: Any,
    full_ids: "list[int]",
    logits_processors: "list",
    sampler: Any,
) -> "tuple[str, Any, int]":
    """FSM-constrained generation for `full_ids`, reusing the cached system+skill
    prefix's KV across sessions WHEN the model's cache supports trimming.

    Returns ``(raw_text, gen_stats, cache_hit_tokens)`` where ``cache_hit_tokens``
    is the number of leading prompt tokens served from the persistent KV cache
    (0 → full reprocess / uncached path). Greedy decoding makes the cached and
    uncached outputs byte-identical.

    Prefix reuse uses the public trim_prompt_cache multi-turn idiom: keep one
    persistent cache + the token ids it holds; reuse the longest common prefix
    with the previous prompt, reprocess only the divergent suffix, then trim the
    generated tail back off. Models whose cache is NOT trimmable (e.g. the batched
    ArraysCache some quantized builds use) can't support this safely, so we detect
    that ONCE and fall back to plain uncached generation — correct, just without
    the prefill saving (which is then better recovered via speculative decoding /
    a smaller prompt).
    """
    from mlx_lm import stream_generate
    from mlx_lm.models import cache as _kc

    global _prompt_cache, _prompt_cache_ids, _prompt_cache_trimmable

    gen_kwargs = dict(
        max_tokens=_MAX_TOKENS,
        logits_processors=logits_processors,
        sampler=sampler,
    )

    def _stream(prompt_ids: "list[int]", prompt_cache: Any) -> "tuple[str, Any]":
        kw = dict(gen_kwargs)
        if prompt_cache is not None:
            kw["prompt_cache"] = prompt_cache
        parts: list[str] = []
        gen_stats: Any = None
        for gr in stream_generate(model.model, model.mlx_tokenizer, prompt_ids, **kw):
            parts.append(gr.text)
            gen_stats = gr
        return "".join(parts), gen_stats

    # One-time capability probe: is this model's KV cache trimmable? If not, prefix
    # reuse can't work, so don't even take the lock — run uncached every call.
    if _prompt_cache_trimmable is None:
        try:
            _prompt_cache_trimmable = bool(
                _kc.can_trim_prompt_cache(_kc.make_prompt_cache(model.model))
            )
        except Exception:  # noqa: BLE001
            _prompt_cache_trimmable = False
        if not _prompt_cache_trimmable:
            log.info(
                "run_task_linker_mlx: model KV cache is not trimmable — prefix "
                "prompt-caching disabled (FSM-compile cache still active)"
            )

    # Uncached path: caching disabled, cache not trimmable, or another call holds
    # the shared cache. A fresh per-call cache (mlx_lm makes one internally) is
    # correct in every case.
    if not (
        _PROMPT_CACHE_ENABLED
        and _prompt_cache_trimmable
        and _prompt_cache_lock.acquire(blocking=False)
    ):
        raw, gen_stats = _stream(full_ids, None)
        return raw, gen_stats, 0

    try:
        if _prompt_cache is None:
            _prompt_cache = _kc.make_prompt_cache(model.model)
            _prompt_cache_ids = []

        common = _common_prefix_len(_prompt_cache_ids, full_ids)
        # Drop any cached tail that doesn't match this prompt, so the cache holds
        # exactly full_ids[:common] before the suffix is processed.
        extra = len(_prompt_cache_ids or []) - common
        if extra > 0:
            _kc.trim_prompt_cache(_prompt_cache, extra)

        suffix = full_ids[common:]
        # stream_generate needs ≥1 token to process. An identical-to-previous
        # prompt (no suffix) is vanishingly unlikely (the per-session block always
        # differs), but handle it: back the cache off one token and reprocess it.
        if not suffix:
            if common > 0:
                _kc.trim_prompt_cache(_prompt_cache, 1)
                common -= 1
                suffix = full_ids[common:]
            else:
                _prompt_cache = _kc.make_prompt_cache(model.model)
                common, suffix = 0, full_ids

        try:
            raw, gen_stats = _stream(suffix, _prompt_cache)
        except Exception:
            # Generation failed mid-stream → the shared cache is now inconsistent.
            _invalidate_prompt_cache()
            raise

        # Restore the cache to exactly the prompt boundary using its OWN physical
        # offset — not a count of generated tokens, which would risk an off-by-one
        # and silently corrupt the reused prefix. After this, the cache holds
        # exactly full_ids, ready for the next session to reuse.
        cur_offset = _cache_offset(_prompt_cache)
        trim_amount = cur_offset - len(full_ids)
        if trim_amount > 0:
            _kc.trim_prompt_cache(_prompt_cache, trim_amount)
            _prompt_cache_ids = list(full_ids)
        elif trim_amount == 0:
            _prompt_cache_ids = list(full_ids)
        else:
            # Couldn't realign the cache to the prompt boundary — rebuild next call
            # rather than risk reusing a misaligned prefix.
            _invalidate_prompt_cache()
        return raw, gen_stats, common
    finally:
        _prompt_cache_lock.release()


def _get_model() -> Any:
    """Return an outlines-wrapped model, loading from disk on the first call.

    Cache-miss load is done under _model_lock (double-checked) so concurrent
    callers can't double-load and the idle evictor can't race the load.
    """
    model_id = _resolve_model_id()
    cached = _model_cache.get(model_id)
    if cached is not None:
        return cached

    with _model_lock:
        cached = _model_cache.get(model_id)   # re-check under lock
        if cached is not None:
            return cached
        try:
            import mlx_lm
            import outlines
        except ImportError as exc:
            raise ImportError(
                f"Required package not installed: {exc}. "
                "Install with: pip install 'mlx-lm>=0.22' 'outlines[mlxlm]>=1.3'"
            ) from exc

        log.info(
            "run_task_linker_mlx: loading %s (first call this process)", model_id
        )
        t0 = time.time()
        mlx_model, tokenizer = mlx_lm.load(
            model_id,
            tokenizer_config={"trust_remote_code": True},
        )
        outlines_model = outlines.from_mlxlm(mlx_model, tokenizer)
        log.info("run_task_linker_mlx: model loaded in %.1fs", time.time() - t0)

        _model_cache[model_id] = outlines_model
        _tokenizer_cache[model_id] = tokenizer
        # Any prior prefix KV belongs to a now-gone model instance — drop it so
        # the next classification rebuilds against this freshly loaded model.
        _invalidate_prompt_cache()
        return outlines_model


@contextmanager
def model_session() -> Iterator[Any]:
    """Yield the loaded model, marking it in-flight so the idle evictor never
    frees it mid-inference. Wrap every direct ``model(...)`` call in this.

    Lock is held only briefly (to bump/clear the in-flight counter), never for
    the duration of inference. NOTE: production serialises all MLX calls upstream
    via the Rust llm_gate (1-permit semaphore), so inferences don't actually
    overlap — this lock scope just avoids adding a second, redundant serialisation
    point, NOT a claim that concurrent generation on the shared model is safe.
    """
    global _in_flight, _last_used
    with _model_lock:
        _in_flight += 1
    try:
        yield _get_model()
    finally:
        with _model_lock:
            _in_flight -= 1
            _last_used = time.monotonic()


def maybe_evict_idle(idle_s: float | None = None) -> float | None:
    """Evict the model if it's resident, nothing is in flight, and it's been
    idle longer than ``idle_s`` (default MLX_IDLE_EVICT_S). Returns the GB freed,
    or None if no eviction happened. Safe to call from a threadpool worker.

    Uses a non-blocking lock acquire: if an inference/load is mutating state we
    simply skip this tick and try again on the next one.
    """
    ttl = _IDLE_EVICT_S if idle_s is None else idle_s
    if ttl <= 0:
        return None
    if not _model_lock.acquire(blocking=False):
        return None
    try:
        if _in_flight > 0 or not _model_cache:
            return None
        if (time.monotonic() - _last_used) < ttl:
            return None
        try:
            import mlx.core as mx
            before = mx.get_active_memory()
        except Exception:               # noqa: BLE001 — mx should always import here
            mx, before = None, 0
        _model_cache.clear()
        # The prefix KV cache is tied to the model we're about to free — drop it
        # too, or the next load would reuse KV from a dead model instance.
        _invalidate_prompt_cache()
        gc.collect()
        freed = 0.0
        if mx is not None:
            mx.clear_cache()
            freed = max(0.0, (before - mx.get_active_memory()) / 1e9)
        log.info(
            "run_task_linker_mlx: evicted idle model (idle ≥ %.0fs), freed ~%.1f GB",
            ttl, freed,
        )
        return freed
    finally:
        _model_lock.release()


def evict_resident_model() -> float | None:
    """Force-evict the resident generative model NOW, ignoring the idle timer.

    The single-slot guarantee: the embedder (distill) and reranker must never be
    resident alongside the generative model. Callers that are about to load a
    different model kind call this first. Respects ``_in_flight`` — if an
    inference is running it returns None rather than freeing under it (the
    worklog pipeline is serialised, so nothing is in flight at a phase boundary).
    Returns GB freed, or None if nothing was evicted.
    """
    if not _model_lock.acquire(blocking=False):
        return None
    try:
        if _in_flight > 0 or not _model_cache:
            return None
        try:
            import mlx.core as mx
            before = mx.get_active_memory()
        except Exception:               # noqa: BLE001
            mx, before = None, 0
        _model_cache.clear()
        _invalidate_prompt_cache()
        gc.collect()
        freed = 0.0
        if mx is not None:
            mx.clear_cache()
            freed = max(0.0, (before - mx.get_active_memory()) / 1e9)
        log.info("run_task_linker_mlx: force-evicted resident model, freed ~%.1f GB", freed)
        return freed
    finally:
        _model_lock.release()


def model_resident() -> bool:
    """True if the MLX model is currently loaded in memory."""
    return bool(_model_cache)


def model_active_memory_gb() -> float | None:
    """Live Metal active-memory footprint in GB, or None if MLX is unavailable.

    Process-wide Metal active memory (≈ the model when resident — the model
    dominates, though a transient load allocation can briefly inflate it), and
    the only honest measure: `ps`/Activity Monitor can't see Metal unified
    memory (they undercount by ~6.5 GB).
    """
    try:
        import mlx.core as mx
        return round(mx.get_active_memory() / 1e9, 2)
    except Exception:  # noqa: BLE001 — mx absent on non-MLX machines
        return None


# ---------------------------------------------------------------------------
# DB helpers
# ---------------------------------------------------------------------------

def _fetch_recent_ticket_activity(
    con: _sqlite3.Connection,
    current_started_at: str,
    candidate_keys: list[str],
) -> list[dict[str, Any]]:
    """The developer's tracked-ticket work in the _CONTINUITY_WINDOW_MIN minutes
    before the current session, aggregated per ticket → a calibrated continuity
    prior (NOT a raw session log).

    Returns one entry per ticket worked in the window:
        {"task_key", "total_s", "sessions", "last_ended_at", "ago_s"}
    ordered by recency (most-recently-active ticket first). Empty when there is no
    qualifying recent work — the caller then omits the block entirely rather than
    asserting a continuity that doesn't exist.

    A session counts only if it is (a) already CLASSIFIED to a ticket
    (task_session_type='task' — "last classified", never pending/in-flight),
    (b) confident enough to trust as a prior (task_confidence >=
    _CONTINUITY_MIN_CONFIDENCE), and (c) mapped to a ticket in the CURRENT
    candidate set — a prior on a ticket the model can't even pick is pure noise.
    Windowing is done in Python (fromisoformat) so it's robust to the stored
    timestamp's timezone/precision; the SQL only does the cheap "strictly before
    current" + confidence prefilter (consistent ISO format → lexicographic '<' is
    chronological).
    """
    candidates = set(candidate_keys)
    if not current_started_at or not candidates:
        return []
    try:
        anchor = _dt.datetime.fromisoformat(current_started_at)
    except (ValueError, TypeError):
        return []
    window_start = anchor - _dt.timedelta(minutes=_CONTINUITY_WINDOW_MIN)
    rows = con.execute(
        "SELECT task_key, started_at, ended_at, duration_s, task_confidence"
        " FROM app_sessions"
        " WHERE started_at < ?"
        "   AND task_key IS NOT NULL"
        "   AND task_session_type = 'task'"
        "   AND task_confidence >= ?"
        " ORDER BY started_at DESC LIMIT 200",
        (current_started_at, _CONTINUITY_MIN_CONFIDENCE),
    ).fetchall()

    agg: dict[str, dict[str, Any]] = {}
    for r in rows:
        d = dict(r)
        tk = d.get("task_key")
        if tk not in candidates:
            continue
        try:
            s_at = _dt.datetime.fromisoformat(d["started_at"])
        except (ValueError, TypeError):
            continue
        if s_at < window_start:
            continue  # outside the continuity window
        try:
            e_at = _dt.datetime.fromisoformat(d.get("ended_at") or d["started_at"])
        except (ValueError, TypeError):
            e_at = s_at
        entry = agg.get(tk)
        if entry is None:
            entry = {"task_key": tk, "total_s": 0.0, "sessions": 0, "last_ended": e_at}
            agg[tk] = entry
        entry["total_s"] += float(d.get("duration_s") or 0.0)
        entry["sessions"] += 1
        if e_at > entry["last_ended"]:
            entry["last_ended"] = e_at

    result: list[dict[str, Any]] = []
    for entry in agg.values():
        ago_s = max(0.0, (anchor - entry["last_ended"]).total_seconds())
        result.append(
            {
                "task_key":      entry["task_key"],
                "total_s":       entry["total_s"],
                "sessions":      entry["sessions"],
                "last_ended_at": entry["last_ended"].isoformat(),
                "ago_s":         ago_s,
            }
        )
    result.sort(key=lambda e: e["ago_s"])  # most-recently-active ticket first
    return result


def _fetch_pm_tasks(
    con: _sqlite3.Connection, focus_keys: list[str] | None = None
) -> list[dict[str, Any]]:
    # Candidate set for classification. Tickets the user explicitly EXCLUDED during
    # onboarding board-cleanup (pm_task_curation.decision = 'excluded') are dropped
    # so a cleaned-up dead ticket can never be a classification target. Everything
    # else flows through, including not-yet-decided `looks_stale` rows — the triage
    # only *proposes*; nothing is removed without the human's confirmed decision.
    # LEFT JOIN keeps this safe if curation has no row for a ticket yet.
    base_cols = (
        "SELECT t.task_key, t.title,"
        "       COALESCE(t.description_text,'') AS description_text,"
        "       COALESCE(t.status_raw,'') AS status_raw,"
        "       COALESCE(t.is_terminal,0) AS is_terminal,"
        "       COALESCE(t.issue_type,'') AS issue_type,"
        "       COALESCE(t.parent_key,'') AS parent_key,"
        "       COALESCE(t.epic_title,'') AS epic_title,"
        "       COALESCE(t.sprint_name,'') AS sprint_name,"
        "       COALESCE(t.tags,'') AS tags"
        " FROM pm_tasks t"
    )
    try:
        rows = con.execute(
            base_cols
            + " LEFT JOIN pm_task_curation c ON c.task_key = t.task_key"
            " WHERE c.decision IS NULL OR c.decision != 'excluded'",
        ).fetchall()
    except _sqlite3.OperationalError:
        # Pre-migration-038 DB (no pm_task_curation): degrade to the unfiltered
        # candidate set rather than crashing the whole /classify_sessions call.
        rows = con.execute(base_cols).fetchall()
    tasks = [dict(r) for r in rows]

    # Candidate-set policy (see _PLAN_ONLY_CANDIDATES). `focus_keys` are the
    # tickets the dev CONFIRMED for this session's day (empty when no plan).
    focus = focus_keys or []
    if not focus:
        # No confirmed plan → offer every candidate. Unchanged behaviour for
        # users who don't use the plan, or days that aren't confirmed yet.
        # `is_today_focus` is left unset (falsy) on every ticket.
        return tasks

    order = {key: i for i, key in enumerate(focus)}

    if _PLAN_ONLY_CANDIDATES:
        # Plan-only: the candidate set IS the confirmed plan, in declared order.
        # Off-plan work then has no candidate to match, so the model returns
        # `untracked` (the intended holding state) instead of being shoehorned
        # onto an unrelated ticket.
        in_plan = [t for t in tasks if t["task_key"] in order]
        # GUARD: never return an empty candidate set. If the confirmed plan's
        # tickets are all absent from the live pool (curation-excluded, closed,
        # or not yet synced), fall back to the full set — an empty list would
        # force EVERY session that day to `untracked`.
        if not in_plan:
            log.warning(
                "plan-only candidates: confirmed plan has no live candidate "
                "tickets (focus=%s) — falling back to full candidate set",
                focus,
            )
            return tasks
        for t in in_plan:
            t["is_today_focus"] = True
        in_plan.sort(key=lambda t: order[t["task_key"]])
        return in_plan

    # Legacy boost-never-filter: tag the declared tickets and float them to the
    # top in declared order, but keep every other candidate so recall is
    # untouched. A focus key not in `tasks` (e.g. excluded by curation) simply
    # has no effect — we never resurrect a filtered-out ticket.
    for t in tasks:
        t["is_today_focus"] = t["task_key"] in order
    tasks.sort(key=lambda t: (0, order[t["task_key"]]) if t.get("is_today_focus") else (1, 0))
    return tasks

