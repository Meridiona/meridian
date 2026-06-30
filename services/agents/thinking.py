"""Unified thinking-mode generation for every MLX generative endpoint.

This is the SINGLE source of truth for how the agent server runs Qwen3.5-2B in
thinking mode. Every generative call — `/classify_tasks`, `/generate_worklog`,
`/propose_ticket`, `/activity_report`, `/summarise` — goes through
``generate_thinking()`` so they all share one implementation of:

  - **Sampling** — the validated anti-loop settings (``temp=0.1``, ``top_p=0.95``,
    ``top_k=20``, ``repetition_penalty=1.1``, ``presence_penalty=1.5``). These are
    deliberately NOT Qwen's recommended ``temp=1.0`` / ``repetition_penalty=1.0``,
    which Qwen's own docs say makes the 2B model loop in thinking mode.
  - **Thinking-budget enforcement** — ``ThinkingBudgetProcessor`` forces ``</think>``
    once the budget is hit. The model's own ``thinking_budget`` chat-template kwarg
    is a NO-OP for this checkpoint (verified), and the 2B is documented to loop, so
    without this cap ~40% of calls burn the whole ``max_tokens`` budget thinking and
    emit no answer.
  - **``</think>`` stripping + token counting**, and a clean ``ThinkingResult``.

Tuned on a labelled eval set: ``temp=0.1`` + ``thinking_budget=2000`` gives 100%
classification accuracy with zero runaways, ~14s/call (was 60s+ runaways).

# Who calls this
``routes/classify.py``, ``routes/generate.py``, ``routes/activity.py``,
``routes/summarise.py`` — each builds its messages, then calls ``generate_thinking``
inside ``run_in_threadpool`` (the generate call is blocking).
"""
from __future__ import annotations

from dataclasses import dataclass

import mlx.core as mx

# ── Unified generation settings (single source of truth) ──────────────────────
# Every endpoint shares these unless it explicitly overrides temp/budget/mode.
DEFAULT_TEMP                    = 0.1    # structured / classification calls (deterministic-ish)
DEFAULT_PROSE_TEMP              = 0.4    # free-prose calls (activity report, summary) — low enough to
                                         # stay faithful and preserve concrete identifiers (ticket keys,
                                         # file paths) the matcher needs, while still reading naturally
DEFAULT_TOP_P                   = 0.95
DEFAULT_TOP_K                   = 20
DEFAULT_REPETITION_PENALTY      = 1.1
DEFAULT_REPETITION_CONTEXT_SIZE = 64
DEFAULT_PRESENCE_PENALTY        = 1.5
DEFAULT_THINKING_BUDGET         = 2000   # cap on <think> tokens; rest of max_tokens is the answer
# The thinking budget must leave room for the answer: if it were >= max_tokens the
# cap could never fire before generation ends, and the model would run away to
# max_tokens with no </think>. Always reserve at least this many tokens for the answer.
_MIN_ANSWER_RESERVE             = 512

# `</think>` is a single token for Qwen3.5; resolved per-tokenizer at runtime.
_END_THINK = "</think>"


# ── Thinking-budget logits processor ──────────────────────────────────────────

class ThinkingBudgetProcessor:
    """Force ``</think>`` once the thinking budget is exhausted (mlx-lm logits processor).

    mlx-lm calls a processor as ``logits = processor(tokens, logits)`` where
    ``tokens`` is the full mx.array of tokens so far. Behaviour:

      - below budget: pass logits through unchanged (model thinks freely);
      - at ~90% of budget: softly bias newline / ``</think>`` so the model can wrap
        up its current sentence and close on its own;
      - at the budget: hard-force a newline then ``</think>`` (set all logits to
        ``-inf`` except the target), then disengage — generation continues into the
        answer;
      - once the model emits ``</think>`` itself (the healthy path) it disengages.

    Mirrors NVIDIA NIM / vLLM reasoning-budget control. ``.forced`` reports whether
    the hard cap fired (telemetry).
    """

    def __init__(self, tokenizer, max_thinking_tokens: int):
        self.max_thinking_tokens = int(max_thinking_tokens)
        self._end_id = self._resolve_end_think_id(tokenizer)
        self._nl_id = self._resolve_newline_id(tokenizer)

        self.start_len: int | None = None
        self.stopped_thinking = False
        self.forced = False
        self._forcing = 0   # 0=no, 1=force newline next, 2=force </think> next

    @staticmethod
    def _resolve_end_think_id(tokenizer) -> int:
        try:
            tid = tokenizer.convert_tokens_to_ids(_END_THINK)
            if isinstance(tid, int) and tid >= 0:
                return tid
        except Exception:  # noqa: BLE001
            pass
        ids = tokenizer.encode(_END_THINK, add_special_tokens=False)
        return ids[-1]

    @staticmethod
    def _resolve_newline_id(tokenizer) -> int:
        ids = tokenizer.encode("\n", add_special_tokens=False)
        return ids[-1] if ids else -1

    def __call__(self, tokens: mx.array, logits: mx.array) -> mx.array:
        if self.start_len is None:
            self.start_len = int(tokens.size)

        if self.stopped_thinking:
            return logits

        # Model closed </think> on its own — healthy path, disengage.
        if int(tokens.size) > 0 and int(tokens[-1].item()) == self._end_id:
            self.stopped_thinking = True
            return logits

        generated = int(tokens.size) - self.start_len

        if self._forcing == 1:
            forced = self._force_only(logits, self._nl_id if self._nl_id >= 0 else self._end_id)
            if self._nl_id >= 0:
                self._forcing = 2
            else:
                self._forcing = 0
                self.stopped_thinking = True
            return forced
        if self._forcing == 2:
            forced = self._force_only(logits, self._end_id)
            self._forcing = 0
            self.stopped_thinking = True
            return forced

        if generated >= self.max_thinking_tokens:
            self.forced = True
            self._forcing = 1
            return self.__call__(tokens, logits)   # apply step 1 now

        if generated >= int(0.9 * self.max_thinking_tokens):
            logits = self._boost(logits, self._end_id, 4.0)
            if self._nl_id >= 0:
                logits = self._boost(logits, self._nl_id, 2.0)
        return logits

    @staticmethod
    def _force_only(logits: mx.array, token_id: int) -> mx.array:
        forced = mx.full(logits.shape, -mx.inf, dtype=logits.dtype)
        forced[..., token_id] = 0.0
        return forced

    @staticmethod
    def _boost(logits: mx.array, token_id: int, amount: float) -> mx.array:
        logits[..., token_id] = logits[..., token_id] + amount
        return logits


# ── Result + generation entrypoint ────────────────────────────────────────────

@dataclass
class ThinkingResult:
    """Outcome of one thinking-mode generation."""
    text:          str    # answer with the <think> block stripped (JSON or prose)
    raw:           str    # full raw model output (pre-strip), for debugging
    input_tokens:  int
    output_tokens: int
    think_tokens:  int
    budget_forced: bool   # the hard thinking-budget cap fired
    closed_think:  bool   # a </think> was emitted (vs. JSON-fallback / prose)


def generate_thinking(
    m,
    messages: list[dict],
    *,
    max_tokens: int,
    thinking_budget: int = DEFAULT_THINKING_BUDGET,
    enable_thinking: bool = True,
    json_mode: bool = False,
    temp: float = DEFAULT_TEMP,
    top_p: float = DEFAULT_TOP_P,
    top_k: int = DEFAULT_TOP_K,
    presence_penalty: float = DEFAULT_PRESENCE_PENALTY,
) -> ThinkingResult:
    """Run one thinking-mode generation and return a :class:`ThinkingResult`.

    Args:
        m: the loaded MLX module (``app_state["mlx_module"]``) — exposes
           ``_get_tokenizer()`` and ``model_session()``.
        messages: chat messages (system + user).
        max_tokens: hard cap on thinking + answer combined.
        thinking_budget: cap on <think> tokens (enforced by the processor).
        enable_thinking: when False, the <think> block is skipped entirely and the
            budget processor is not attached.
        json_mode: when True and no ``</think>`` was emitted, fall back to the last
            ``{...}`` object in the output (structured callers); when False the whole
            post-think text is kept (prose callers).
        temp / top_p / top_k / presence_penalty: sampling. Default to the unified
            ``DEFAULT_*`` constants so every fixed-purpose pipeline call generates
            identically; the OpenAI-compat gateway (`routes/chat.py`) overrides them
            with caller-supplied values while still getting budget enforcement.

    ``repetition_penalty`` / ``repetition_context_size`` always come from the
    ``DEFAULT_*`` constants.
    """
    from mlx_lm import generate
    from mlx_lm.sample_utils import make_sampler, make_logits_processors

    sampler = make_sampler(temp=temp, top_p=top_p, top_k=top_k)
    logits_processors = make_logits_processors(
        repetition_penalty=DEFAULT_REPETITION_PENALTY,
        repetition_context_size=DEFAULT_REPETITION_CONTEXT_SIZE,
        presence_penalty=presence_penalty,
    )
    hf_tokenizer = m._get_tokenizer()

    # Clamp the budget below max_tokens so the cap always fires with room for the
    # answer — otherwise a caller passing thinking_budget >= max_tokens would get a
    # runaway to max_tokens with no </think>.
    effective_budget = min(thinking_budget, max(1, max_tokens - _MIN_ANSWER_RESERVE))
    budget_proc = ThinkingBudgetProcessor(hf_tokenizer, max_thinking_tokens=effective_budget)
    if enable_thinking:
        logits_processors.append(budget_proc)

    prompt_ids = hf_tokenizer.apply_chat_template(
        messages,
        add_generation_prompt=True,
        enable_thinking=enable_thinking,
    )
    if hasattr(prompt_ids, "keys") and "input_ids" in prompt_ids:
        prompt_ids = prompt_ids["input_ids"]
    input_tokens = len(prompt_ids)

    with m.model_session() as model:
        raw = generate(
            model.model, hf_tokenizer,
            prompt=prompt_ids,
            max_tokens=max_tokens,
            sampler=sampler,
            logits_processors=logits_processors,
            verbose=False,
        )

    think_tokens = 0
    closed_think = False
    text = raw
    if _END_THINK in raw:
        think_part, text = raw.split(_END_THINK, 1)
        think_tokens = len(hf_tokenizer.encode(think_part + _END_THINK))
        text = text.strip()
        closed_think = True
    elif json_mode:
        # Model wrote reasoning without tags — recover the last {...} object.
        start, end = raw.rfind("{"), raw.rfind("}")
        text = raw[start : end + 1] if (start != -1 and end > start) else ""

    output_tokens = len(hf_tokenizer.encode(text)) if text else 0
    return ThinkingResult(
        text=text, raw=raw,
        input_tokens=input_tokens, output_tokens=output_tokens,
        think_tokens=think_tokens, budget_forced=budget_proc.forced,
        closed_think=closed_think,
    )
