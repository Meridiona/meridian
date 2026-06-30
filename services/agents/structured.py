"""Grammar-constrained (FSM) JSON generation for the structured MLX endpoints.

This is the SINGLE source of truth for every LLM call that must return a JSON
object the pipeline parses — ``/classify_tasks``, ``/generate_worklog``,
``/propose_ticket``. It uses `outlines` to drive `mlx-lm` with a finite-state
logits processor compiled from a JSON schema, so the decoder *cannot* emit a
token that breaks the grammar. The output is therefore always structurally valid
JSON — eliminating the ~7% silent parse failures the native-thinking + budget-cap
path leaves behind (the model getting severed mid-thought and continuing in prose
instead of JSON).

Thinking is OFF for these calls (``enable_thinking=False``): the FSM guarantees
the shape, so the scratch-space the <think> block bought is no longer needed, and
it's the thinking runaway that caused the failures in the first place. A bounded
``reasoning`` field in each schema gives the model a small, grammar-capped place to
reason *inside* the JSON (the maxLength bound is what makes FSM safe — without it a
string field can run to ``max_tokens`` mid-value and never close). Thinking can be
layered back later as a separate step.

Prose endpoints (``/activity_report``, ``/summarise``, ``/distill_hour``) do NOT use
this — they keep ``agents.thinking.generate_thinking``; FSM is for JSON only.

# Who calls this
``routes/classify.py`` (classify_tasks), ``routes/generate.py`` (generate_worklog,
propose_ticket) — each builds its messages + a Pydantic output model, then calls
``generate_structured`` inside ``run_in_threadpool``.

# Related
[`agents.thinking`] — the thinking-mode sibling for prose calls.
"""
from __future__ import annotations

import logging
from dataclasses import dataclass
from typing import Any

log = logging.getLogger("agents.structured")

# Reuse the validated anti-loop sampling from the thinking path so structured and
# prose calls generate with the same temperament. FSM owns the structure; the
# sampler only picks among grammar-legal tokens.
from agents.thinking import DEFAULT_TEMP, DEFAULT_TOP_P, DEFAULT_TOP_K  # noqa: E402

# (model_id → outlines MLXLM wrapper) and ((model_id, schema_name) → Generator).
# Keyed by the live mlx model's id() so an idle-evicted-then-reloaded model (new
# object) misses the cache and rebuilds, never serving a generator bound to a freed
# model. Compiling the FSM index for a schema is the expensive step (~hundreds of ms),
# so caching it per schema is what keeps per-call overhead negligible.
_model_cache: dict[int, Any] = {}
_gen_cache: dict[tuple[int, str], Any] = {}


@dataclass
class StructuredResult:
    """Outcome of one FSM-constrained generation. Mirrors the subset of
    :class:`agents.thinking.ThinkingResult` the routes read, so wiring is a drop-in.
    """
    text:          str    # the JSON object (guaranteed grammar-valid)
    input_tokens:  int
    output_tokens: int
    think_tokens:  int = 0       # always 0 — thinking is off for FSM calls
    budget_forced: bool = False  # N/A under FSM; kept for record_gen_params parity


def _outlines_model(bundle: Any):
    """Wrap the live mlx model bundle as an outlines MLXLM (cached per model id)."""
    import outlines

    key = id(bundle.model)
    cached = _model_cache.get(key)
    if cached is not None:
        return cached
    # A fresh model object → drop any generators bound to a previous one.
    _model_cache.clear()
    _gen_cache.clear()
    model = outlines.from_mlxlm(bundle.model, bundle.mlx_tokenizer)
    # We render the chat template ourselves (with enable_thinking=False) and pass the
    # finished prompt string. outlines' MLXLMTypeAdapter, seeing a tokenizer that HAS a
    # chat template, would otherwise re-wrap our string as a fresh user turn and template
    # it AGAIN — burying our system role inside a user message and appending a stray
    # <think>. Disabling has_chat_template makes format_str_input pass our string through
    # verbatim, so the model sees exactly the prompt we built. (Verified against the
    # outlines 1.3.0 source: MLXLMTypeAdapter.format_str_input.)
    model.type_adapter.has_chat_template = False
    _model_cache[key] = model
    return model


def _generator(bundle: Any, output_type: Any):
    """Return a cached outlines Generator for ``output_type`` against the live model.

    The FSM index is compiled once per (model, schema) and reused — the per-call
    cost after warmup is just constrained decoding, not grammar compilation.
    """
    import outlines

    model = _outlines_model(bundle)
    name = getattr(output_type, "__name__", repr(output_type))
    key = (id(bundle.model), name)
    gen = _gen_cache.get(key)
    if gen is None:
        gen = outlines.Generator(model, output_type)
        _gen_cache[key] = gen
        log.info("structured: compiled FSM generator for schema=%s", name)
    return gen


def generate_structured(
    m,
    messages: list[dict],
    *,
    output_type: Any,
    max_tokens: int,
    temp: float = DEFAULT_TEMP,
) -> StructuredResult:
    """Run one FSM-constrained generation and return a :class:`StructuredResult`.

    Args:
        m: the loaded MLX module (``app_state["mlx_module"]``) — exposes
           ``_get_tokenizer()`` and ``model_session()``.
        messages: chat messages (system + user). Rendered with the model's chat
           template and ``enable_thinking=False`` (no <think> block).
        output_type: a Pydantic model (or ``outlines.types`` value) describing the
           JSON to produce. Bound string/list fields keep the FSM from running a
           field to ``max_tokens`` mid-value.
        max_tokens: hard cap on generated tokens (answer only — there's no thinking).
        temp: sampling temperature (defaults to the structured ``DEFAULT_TEMP``).

    The returned ``text`` is always a structurally valid JSON object matching
    ``output_type`` — callers still ``json.loads`` it but never have to handle a
    grammar-broken string.
    """
    from mlx_lm.sample_utils import make_sampler

    hf_tokenizer = m._get_tokenizer()
    prompt = hf_tokenizer.apply_chat_template(
        messages,
        add_generation_prompt=True,
        enable_thinking=False,
        tokenize=False,
    )
    input_tokens = len(hf_tokenizer.encode(prompt))
    sampler = make_sampler(temp=temp, top_p=DEFAULT_TOP_P, top_k=DEFAULT_TOP_K)

    with m.model_session() as bundle:
        generator = _generator(bundle, output_type)
        text = generator(prompt, max_tokens=max_tokens, sampler=sampler)

    text = (text or "").strip()
    output_tokens = len(hf_tokenizer.encode(text)) if text else 0
    return StructuredResult(
        text=text, input_tokens=input_tokens, output_tokens=output_tokens,
    )
