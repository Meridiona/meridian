"""Single source of truth for the on-device models the pipeline needs.

The end-to-end worklog pipeline (classification → worklog update) runs three
distinct models, each in its own single-slot turn (the runtime never keeps two
resident at once — see ``mlx_classifier.evict_resident_model``):

    role      default checkpoint                       loader          used by
    ────────  ───────────────────────────────────────  ──────────────  ──────────────────────────────────
    llm       mlx-community/Qwen3.5-2B-OptiQ-4bit       mlx_lm          classify · match · /summarise · /activity_report · worklog synth · propose
    reranker  kerncore/Qwen3-Reranker-0.6B-MLX-4bit    mlx_lm          ticket↔worklog scoring (/rerank)
    embedder  mlx-community/Qwen3-Embedding-0.6B-8bit   mlx_embeddings  session-distillation SemDeDup (/distill_hour)

There is no separate "classifier" model: classification and matching run on the
``llm`` checkpoint via the OpenAI-compatible endpoint.

Every checkpoint is env-overridable (defaults below) so eval/experiments can
swap any role without code edits. This module is the ONLY place these ids and
their HuggingFace download filesets are declared — ``mlx_classifier``,
``reranker``, ``session_distiller`` and the ``/prefetch_model`` route all read
from here, so onboarding can eagerly fetch exactly the set the runtime will load.

# Who reads this
    agents.mlx_classifier      → MODEL_ID            (llm)
    agents.reranker            → _RERANKER_ID        (reranker)
    agents.session_distiller   → embedder load       (embedder)
    agents.routes.prefetch     → eager multi-model download for the setup wizard
"""
from __future__ import annotations

import os
from dataclasses import dataclass

# mlx_lm.load()'s default fileset — exactly what a generative or reranker MLX
# repo resolves on load(). The mlx-community embedding repos are plain
# transformer encoders shipping the same config/safetensors/tokenizer layout,
# so this pattern set covers all three roles.
_MLX_ALLOW_PATTERNS: list[str] = [
    "*.json", "model*.safetensors", "*.py", "tokenizer.model",
    "*.tiktoken", "tiktoken.model", "*.txt", "*.jsonl", "*.jinja",
]


@dataclass(frozen=True)
class ModelSpec:
    """One model role: its env override, default checkpoint, loader, and fileset.

    ``loader`` selects the runtime entry point — ``"mlx_lm"`` for generative /
    reranker weights (``mlx_lm.load``) and ``"mlx_embeddings"`` for the encoder
    (``mlx_embeddings.load``). ``allow_patterns`` is the HF download filter the
    prefetch route applies so it fetches exactly what ``load()`` will resolve.
    """

    role: str
    env_var: str
    default_id: str
    loader: str
    allow_patterns: list[str]

    @property
    def model_id(self) -> str:
        """Resolved checkpoint id — the env override if set, else the default."""
        return os.environ.get(self.env_var, self.default_id)


LLM = ModelSpec(
    role="llm",
    env_var="MERIDIAN_LLM_ID",
    default_id="mlx-community/Qwen3.5-2B-OptiQ-4bit",
    loader="mlx_lm",
    allow_patterns=_MLX_ALLOW_PATTERNS,
)

RERANKER = ModelSpec(
    role="reranker",
    env_var="WORKLOG_RERANKER_ID",
    default_id="kerncore/Qwen3-Reranker-0.6B-MLX-4bit",
    loader="mlx_lm",
    allow_patterns=_MLX_ALLOW_PATTERNS,
)

EMBEDDER = ModelSpec(
    role="embedder",
    env_var="MERIDIAN_EMBEDDER_ID",
    default_id="mlx-community/Qwen3-Embedding-0.6B-8bit",
    loader="mlx_embeddings",
    allow_patterns=_MLX_ALLOW_PATTERNS,
)

# Ordered llm → reranker → embedder: the wizard prefetches in this order, and the
# llm is largest so its progress dominates the bar first.
ALL_SPECS: tuple[ModelSpec, ...] = (LLM, RERANKER, EMBEDDER)


def llm_id() -> str:
    """Resolved generative/classifier checkpoint id."""
    return LLM.model_id


def reranker_id() -> str:
    """Resolved reranker checkpoint id."""
    return RERANKER.model_id


def embedder_id() -> str:
    """Resolved embedder checkpoint id."""
    return EMBEDDER.model_id
