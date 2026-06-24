"""Worklog-salience scorer using the validated MLX Qwen3-Reranker-0.6B (yes/no logit).

Reframes the reranker from ticket-binding to span salience: query = "concrete dev work
worth recording in a work-log"; document = one captured screen line. Returns yes-probability.
Loaded lazily; call free() before loading another model (one-resident-at-a-time rule).
"""
from __future__ import annotations
import os

_M = None
_TOK = None
_YES = _NO = None
REPO = os.environ.get("RR_REPO", "kerncore/Qwen3-Reranker-0.6B-MLX-4bit")

INSTRUCT = ("Judge whether this line of captured screen text is evidence of concrete work a "
            "developer would record in a work-log — an edit, command, file path, code, result, "
            "error, decision, ticket, or the specific topic/page/feature being worked on. "
            "Answer no for UI chrome, menus, navigation, ads, app shells, or unreadable OCR noise.")
PREFIX = ("<|im_start|>system\nJudge whether the Document meets the requirements based on the "
          "Query and the Instruct provided. Note that the answer can only be \"yes\" or \"no\"."
          "<|im_end|>\n<|im_start|>user\n")
SUFFIX = "<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"
QUERY = "concrete development work worth recording in a work-log"


def load_rr():
    global _M, _TOK, _YES, _NO
    if _M is not None:
        return
    import mlx.core as mx  # noqa
    from mlx_lm import load
    print(f"  loading reranker {REPO} ...", flush=True)
    _M, _TOK = load(REPO)
    _YES = _TOK.encode("yes", add_special_tokens=False)[0]
    _NO = _TOK.encode("no", add_special_tokens=False)[0]


def score_lines(lines: list[str]) -> list[float]:
    import mlx.core as mx
    import mlx.nn as nn
    load_rr()
    out = []
    for d in lines:
        ids = _TOK.encode(f"{PREFIX}<Instruct>: {INSTRUCT}\n<Query>: {QUERY}\n<Document>: {d[:300]}{SUFFIX}",
                          add_special_tokens=False)
        lg = _M(mx.array([ids]))[0, -1, :]
        p = nn.softmax(mx.array([lg[_NO].item(), lg[_YES].item()]))[1].item()
        out.append(p)
    return out


def peak_gb():
    try:
        import mlx.core as mx
        return round(mx.get_peak_memory() / 1e9, 2)
    except Exception:
        return None


def free():
    global _M, _TOK
    _M = _TOK = None
    import gc
    gc.collect()
    try:
        import mlx.core as mx
        mx.clear_cache()
    except Exception:
        pass
