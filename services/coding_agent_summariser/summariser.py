"""Summarise one sealed coding-agent segment: Claude → MLX fallback → write.

`summarise_one` is the single unit of work, shared by the daemon and the CLI.
It never raises — it returns an `Outcome` describing what happened, so callers
(loops) stay simple and a bad row can't kill the process.
"""
from __future__ import annotations

import logging
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Optional

from coding_agent_summariser import claude_runner, codex_runner, config, db, mlx_fallback
from coding_agent_summariser.claude_runner import RateLimited, SummariserError
from coding_agent_summariser.db import PendingRow

log = logging.getLogger(__name__)


class Source(str, Enum):
    CLAUDE = "claude"
    CODEX  = "codex"
    MLX    = "mlx"
    NONE   = "none"


def _primary_engine(agent: str):
    """Pick the per-agent engine: Codex sessions → codex, else → claude."""
    if (agent or "").strip().lower() == "codex":
        return codex_runner.run_codex, Source.CODEX
    return claude_runner.run_claude, Source.CLAUDE


@dataclass(frozen=True)
class Outcome:
    row_id:       int
    written:      bool                  # summary persisted this call
    source:       Source                # which engine produced it
    rate_limited: bool = False          # Claude hit a usage/rate limit (→ daemon backs off)
    error:        Optional[str] = None
    summary:      Optional[str] = None  # the generated text (always set on success; for --dry-run)


def summarise_one(row: PendingRow, *, write: bool = True, db_path: Optional[Path] = None) -> Outcome:
    """Produce (and by default persist) a summary for one segment. Never raises.

    `write=False` generates the summary but does not touch the DB — used by the
    CLI's `--dry-run` to eyeball output.
    """
    transcript = db.fetch_transcript(row.id, db_path=db_path)
    if not transcript.strip():
        # Sealed-but-empty shouldn't reach the queue (it filters on non-empty
        # session_text), but guard anyway rather than send an empty prompt.
        return Outcome(row.id, False, Source.NONE, error="empty transcript")

    prior = db.fetch_prior_summary(row.session_uuid, row.segment_started_at, db_path=db_path)
    stdin_text = _build_prompt(transcript, prior)

    summary: Optional[str] = None
    source = Source.NONE
    rate_limited = False
    errors: list[str] = []

    # 1. Primary: the session's own agent (Codex → codex exec, else claude -p),
    #    on the user's subscription for that tool.
    primary_fn, primary_source = _primary_engine(row.agent)
    try:
        summary = primary_fn(stdin_text)["summary"]
        source = primary_source
    except RateLimited as exc:
        rate_limited = True
        errors.append(f"{primary_source.value} rate-limited: {exc}")
    except SummariserError as exc:
        errors.append(f"{primary_source.value} failed: {exc}")

    # 2. Fallback: local MLX (on any primary failure).
    if summary is None:
        try:
            summary = mlx_fallback.summarise(stdin_text)
            source = Source.MLX
        except SummariserError as exc:
            errors.append(f"mlx failed: {exc}")

    if summary is None:
        return Outcome(row.id, False, Source.NONE, rate_limited=rate_limited,
                       error="; ".join(errors))

    wrote = db.write_summary(row.id, summary, source=source.value, db_path=db_path) if write else False
    log.info(
        "summarised: row_id=%d uuid=%s seg=%s source=%s wrote=%s chars=%d",
        row.id, row.session_uuid[:8], row.segment_started_at, source.value, wrote, len(summary),
    )
    return Outcome(row.id, wrote, source, rate_limited=rate_limited, summary=summary)


# ──────────────────────── Prompt assembly ──────────────────────────────────────


def _build_prompt(transcript: str, prior_summary: Optional[str]) -> str:
    """stdin for the model: optional prior-burst context + (capped) transcript."""
    parts: list[str] = []
    if prior_summary:
        parts.append("## EARLIER IN THIS SESSION (context — do not repeat)\n" + prior_summary)
    parts.append("## TRANSCRIPT\n" + _cap_transcript(transcript))
    return "\n\n".join(parts)


def _cap_transcript(transcript: str, cap: Optional[int] = None) -> str:
    """Bound transcript size: keep the head (task setup) and tail (outcome).

    A pathological multi-MB transcript would otherwise blow token cost / memory.
    Most bursts are well under the cap and pass through untouched.
    """
    cap = cap or config.TRANSCRIPT_CAP_CHARS
    if len(transcript) <= cap:
        return transcript
    head_len = cap * 7 // 10
    tail_len = cap - head_len
    elided = len(transcript) - cap
    return (
        transcript[:head_len]
        + f"\n\n…[{elided} chars elided — long autonomous stretch omitted]…\n\n"
        + transcript[-tail_len:]
    )
