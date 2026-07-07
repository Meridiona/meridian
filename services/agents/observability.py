"""OpenTelemetry + structured-logging bootstrap for Meridian Python agents.

A single `setup(agent_name)` call wires up:

  * an OTel `TracerProvider` with `service.name=agent_name`
  * export ALWAYS goes through the durable disk spool
    (`~/.meridian/telemetry/pending/`), never a live HTTP call from this
    process — see "Spool-only export" below
  * a `LoggerProvider` + matching log handler so every `logging.LogRecord`
    is correlated to the active span via trace_id/span_id
  * W3C `TraceContextTextMapPropagator` as the global propagator so each
    agent can pick up the Rust daemon's `traceparent` and continue the trace
  * `LoggingInstrumentor` so every `logging.LogRecord` carries
    `otelTraceID` / `otelSpanID` attributes for correlation

The OTel spool is the ONLY sink `logging`/spans write to — there is no JSONL
file handler and no stdout/stderr mirror (removed: they duplicated the exact
same events this module already spools). launchd still redirects this
process's raw stdout/stderr to `~/.meridian/logs/mlx-server.log`/`-error.log`
as an OS-level crash safety net (unrelated to `logging`/`tracing`), but that's
the only other place any of this process's output lands.

Capture is unconditional: every span/log batch is ALWAYS written to the spool
(`_write_spool`, an atomic tmp-then-rename into
`~/.meridian/telemetry/pending/`) — the exact same
`<signal>-<unix_micros>-<seq>.otlp` layout `src/telemetry_spool/writer.rs`
produces — regardless of `~/.meridian/settings.json`'s `otlp_enabled` toggle.
Capture is a local disk write, not a network call, so there is no reason to
gate it; only `MERIDIAN_TRACING_DISABLED` (an explicit dev/test escape hatch,
see `_capture_disabled()`) skips it. Generation (this process) and delivery
are fully decoupled:

  * The Rust daemon's `telemetry_spool::shipper` background task drains
    `pending/` into OpenObserve whenever it's reachable AND the install is
    allowed to ship — a Canonical/packaged install (the shipped DMG) never
    ships at all, regardless of `otlp_enabled`; only a Dev/Bare checkout with
    `otlp_enabled` + credentials configured ships live (see
    `src/observability.rs`'s `resolve_otlp_target`/`is_canonical_install`).
    Independent of whether this process is even still running — `ship_one` in
    `telemetry_spool/mod.rs` classifies failures as Terminal (payload is bad,
    quarantined) or Retryable (network/5xx/429, retried next tick), so a
    down or flaky OpenObserve never loses or corrupts anything, it just
    backs up on disk until OO comes back (subject to the same 7-day
    retention as a Canonical install's spool).
  * `meridian telemetry export`/`import` (`telemetry_spool/cli.rs`), or the
    tray's "Export Diagnostics" button for end users, lets a user hand
    someone else the pending spool directly (or import one) — the "give a
    customer's log bundle to support, load it into our own OpenObserve" path
    this architecture exists for.

This process NEVER opens a live connection to OpenObserve itself. Earlier
revisions shipped directly via `OTLPSpanExporter`/`OTLPLogExporter` when OO
credentials were configured in settings.json (skipping the Rust daemon) —
that meant a long-running process (the MLX server) held live HTTP
export/retry state against OpenObserve, and when OO went down for hours,
continuous failed-export retries correlated with the process's memory
growing from ~100MB to 20+GB. Spool-only export removes that whole failure
class: writing to disk can't hang, retry-loop, or accumulate connection
state, and the already-hardened Rust shipper (atomic writes, terminal/
retryable classification, quarantine) is the only thing that ever talks to
OpenObserve.

`extract_parent_context(traceparent)` is the helper agents use to continue
a span emitted by another process — typically the Rust ETL or another
agent stage.

Idempotent: calling `setup` twice is a no-op for the second call (returns
the existing tracer).
"""
from __future__ import annotations

import logging
import os
import threading
import time
from pathlib import Path
from typing import Optional

from opentelemetry import trace
from opentelemetry._logs import set_logger_provider
from opentelemetry.context import Context
from opentelemetry.instrumentation.logging import LoggingInstrumentor
from opentelemetry.propagate import set_global_textmap
from opentelemetry.sdk._logs import LoggerProvider, LoggingHandler
from opentelemetry.sdk._logs.export import BatchLogRecordProcessor, SimpleLogRecordProcessor, LogExportResult
from opentelemetry.sdk.resources import Resource
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor, SimpleSpanProcessor, SpanExportResult
from opentelemetry.trace.propagation.tracecontext import (
    TraceContextTextMapPropagator,
)


# ──────────────────────── Spool exporters ──────────────────────────────────────

_spool_seq_lock = threading.Lock()
_spool_seq = 0


def _next_spool_seq() -> int:
    global _spool_seq
    with _spool_seq_lock:
        val = _spool_seq
        _spool_seq += 1
    return val


def _resolve_telemetry_dir() -> Path:
    """Mirror of the Rust writer's resolve_telemetry_dir().

    Precedence: MERIDIAN_TELEMETRY_DIR env → ~/.meridian/telemetry.
    """
    env = os.environ.get("MERIDIAN_TELEMETRY_DIR", "").strip()
    if env:
        return Path(env).expanduser()
    home = Path.home()
    return home / ".meridian" / "telemetry"


def _write_spool(signal: str, payload: bytes) -> None:
    """Atomically write payload to ~/.meridian/telemetry/pending/.

    Filename: <signal>-<unix_micros>-<seq>.otlp
    Write via <name>.tmp then rename so the Rust shipper never sees partial files.
    """
    base = _resolve_telemetry_dir()
    pending = base / "pending"
    pending.mkdir(parents=True, exist_ok=True)

    micros = int(time.time() * 1_000_000)
    seq = _next_spool_seq()
    filename = f"{signal}-{micros}-{seq}.otlp"
    final_path = pending / filename
    tmp_path = pending / f"{filename}.tmp"

    try:
        # fsync the tmp file before the rename so a power loss can't leave a
        # rename (metadata) durable while the data blocks are still in page
        # cache — that would surface a truncated .otlp the shipper POSTs (→ a
        # 400). Mirrors the Rust writer's sync_all() + dir fsync.
        with open(tmp_path, "wb") as fh:
            fh.write(payload)
            fh.flush()
            os.fsync(fh.fileno())
        tmp_path.rename(final_path)
        try:
            dir_fd = os.open(str(pending), os.O_RDONLY)
            try:
                os.fsync(dir_fd)
            finally:
                os.close(dir_fd)
        except OSError:
            # Directory fsync is best-effort (not all FS/platforms support it);
            # the tmp-file fsync already guarantees the data is on disk.
            pass
    except Exception as exc:
        logging.getLogger(__name__).warning(
            "telemetry spool write failed — payload dropped",
            extra={"signal": signal, "error": str(exc)},
        )
        # Best-effort cleanup so a failed write never strands a .tmp orphan
        # (the Rust shipper sweeps these, but don't rely on it).
        try:
            tmp_path.unlink(missing_ok=True)
        except OSError as cleanup_exc:
            # Cleanup failure is non-fatal: write already failed and we avoid
            # raising secondary errors from best-effort orphan removal.
            logging.getLogger(__name__).debug(
                "telemetry spool tmp cleanup failed",
                extra={"tmp_path": str(tmp_path), "error": str(cleanup_exc)},
            )


class SpoolSpanExporter:
    """Span exporter that writes serialised OTLP payloads to the spool dir.

    Wraps the SDK's encode_spans() to produce the same wire bytes the real
    OTLPSpanExporter would POST.  The Rust shipper drains pending/ to OO.
    """

    def export(self, spans):  # type: ignore[override]
        try:
            from opentelemetry.exporter.otlp.proto.common.trace_encoder import (
                encode_spans,
            )
            payload = encode_spans(spans).SerializeToString()
            _write_spool("traces", payload)
        except Exception as exc:
            logging.getLogger(__name__).warning(
                "SpoolSpanExporter.export failed", extra={"error": str(exc)}
            )
        return SpanExportResult.SUCCESS

    def shutdown(self) -> None:
        pass

    def force_flush(self, timeout_millis: int = 30_000) -> bool:
        return True


class SpoolLogExporter:
    """Log exporter that writes serialised OTLP payloads to the spool dir.

    Mirrors SpoolSpanExporter for the log signal.
    """

    def export(self, log_data):  # type: ignore[override]
        try:
            from opentelemetry.exporter.otlp.proto.common._log_encoder import (
                encode_logs,
            )
            payload = encode_logs(log_data).SerializeToString()
            _write_spool("logs", payload)
        except Exception as exc:
            logging.getLogger(__name__).warning(
                "SpoolLogExporter.export failed", extra={"error": str(exc)}
            )
        return LogExportResult.SUCCESS

    def shutdown(self) -> None:
        pass

    def force_flush(self, timeout_millis: int = 30_000) -> bool:
        return True


# ──────────────────────── Config ───────────────────────────────────────────────
_NOISY_LOGGERS = ("urllib3", "httpx", "httpcore", "openai", "botocore")

# Track which agents have been configured so a second setup() call is a no-op.
_INITIALISED: dict[str, trace.Tracer] = {}
_PROCESS_SERVICE_NAME: str | None = None
# Held so shutdown() can flush log records the same way it flushes spans.
_LOGGER_PROVIDER: LoggerProvider | None = None


def _capture_disabled() -> bool:
    """Hard kill switch for OTel capture (spans/logs to the local spool).

    Capture is a local disk write, not a network call, so it always runs
    regardless of `otlp_enabled` / shipping config — this is the only escape
    hatch, for local dev/testing where even the spool write is unwanted.
    """
    return os.environ.get("MERIDIAN_TRACING_DISABLED", "").lower() in ("1", "true", "yes")


# ──────────────────────── Public API ───────────────────────────────────────────
def setup(agent_name: str) -> trace.Tracer:
    """Configure OpenTelemetry + JSON logging for one agent process.

    The FIRST call in a process wins ownership of the global TracerProvider
    + JSON logging handlers — its `agent_name` becomes the process's
    `service.name` resource attribute. Subsequent calls (e.g. when the
    tagger entry point imports stage2 / stage3 which each call `setup`)
    return a fresh `Tracer` scoped to that agent's name — those spans still
    carry the original process service.name as their resource, but their
    instrumentation-scope.name distinguishes the producer in OpenObserve.

    This compromise matches OTel's "one resource per process" model while
    keeping `setup` idempotent in shared-library imports.
    """
    import re
    if not re.fullmatch(r"[A-Za-z0-9_\-]+", agent_name):
        raise ValueError(f"agent_name must be alphanumeric/dash/underscore only: {agent_name!r}")
    global _PROCESS_SERVICE_NAME

    if agent_name in _INITIALISED:
        return _INITIALISED[agent_name]

    if _PROCESS_SERVICE_NAME is None:
        _PROCESS_SERVICE_NAME = agent_name
        _configure_tracing(agent_name)
        _configure_logging(agent_name)
        logging.getLogger(agent_name).info(
            "observability initialised",
            extra={"service.name": agent_name},
        )

    tracer = trace.get_tracer(agent_name)
    _INITIALISED[agent_name] = tracer
    return tracer


def shutdown() -> None:
    """Shut down the global TracerProvider and log provider.

    BatchSpanProcessor/BatchLogRecordProcessor queue spans asynchronously;
    calling shutdown() flushes the queue before releasing resources.
    """
    provider = trace.get_tracer_provider()
    if hasattr(provider, "shutdown"):
        provider.shutdown()

    if _LOGGER_PROVIDER is not None:
        _LOGGER_PROVIDER.shutdown()


def extract_parent_context(traceparent: Optional[str]) -> Optional[Context]:
    """Parse an incoming W3C `traceparent` header into an OTel `Context`.

    Returns `None` when the header is empty/missing so callers can pass the
    result straight to `tracer.start_as_current_span(..., context=ctx)`
    without a branch — `None` means "start a fresh root span".
    """
    if not traceparent:
        return None
    return TraceContextTextMapPropagator().extract({"traceparent": traceparent})


# ──────────────────────── Tracing setup ────────────────────────────────────────
# Spool-only by design — see the module docstring's "Spool-only export"
# section for why this process never opens a live connection to OpenObserve.
# `src/telemetry_spool/shipper.rs` (Rust daemon, runs independently) is the
# only thing that ever ships these bytes to OO; `meridian telemetry export/
# import` is the manual escape hatch when the daemon isn't running at all.


def _configure_tracing(agent_name: str) -> None:
    # The W3C propagator is always installed so traceparent round-trips work.
    set_global_textmap(TraceContextTextMapPropagator())

    if _capture_disabled():
        return

    resource = Resource.create({"service.name": agent_name})
    provider = TracerProvider(resource=resource)
    # BatchSpanProcessor avoids blocking inference threads on each span end;
    # SpoolSpanExporter itself is a synchronous local file write, so this can
    # never hang, retry-loop, or accumulate connection state regardless of
    # whether OpenObserve is reachable.
    provider.add_span_processor(BatchSpanProcessor(SpoolSpanExporter()))
    trace.set_tracer_provider(provider)


def _configure_log_export(agent_name: str) -> Optional[logging.Handler]:
    """Build an OTel log handler so every ``log.*`` record reaches OpenObserve,
    correlated to the active span by trace_id/span_id.

    Returns the handler (caller attaches it to root) or ``None`` when export is
    disabled — logs still go to the JSONL file + stdout/stderr regardless.
    """
    global _LOGGER_PROVIDER

    if _capture_disabled():
        return None

    resource = Resource.create({"service.name": agent_name})
    provider = LoggerProvider(resource=resource)
    # BatchLogRecordProcessor so log export never blocks callers — see
    # _configure_tracing above for why SpoolLogExporter is always used.
    provider.add_log_record_processor(BatchLogRecordProcessor(SpoolLogExporter()))
    set_logger_provider(provider)
    _LOGGER_PROVIDER = provider
    return LoggingHandler(level=logging.NOTSET, logger_provider=provider)


# ──────────────────────── Logging setup ────────────────────────────────────────
def _configure_logging(agent_name: str) -> None:
    """Wire the stdlib `logging` root logger to the OTel spool — the ONLY
    sink. No file handler, no stdout/stderr mirror: see the module docstring's
    "Capture is unconditional" section for why a single pipeline is the goal.
    launchd still redirects this process's raw stdout/stderr to
    `~/.meridian/logs/mlx-server.log`/`-error.log` as an OS-level crash safety
    net, but `logging` itself no longer writes anything there directly.
    """
    level_name = os.environ.get("LOG_LEVEL", "INFO").upper()
    level = getattr(logging, level_name, logging.INFO)

    # Hook the std-lib logging module so each LogRecord receives
    # otelTraceID / otelSpanID attributes from the active span context —
    # needed for trace/log correlation on the OTel log records below.
    LoggingInstrumentor().instrument(set_logging_format=False)

    root = logging.getLogger()
    # Clear any pre-existing handlers — long-running daemons that import
    # third-party libs (mcp, etc.) often leave a default basicConfig handler
    # behind that would duplicate every line.
    root.handlers.clear()
    # Spool every record to the local OTel pipeline (see `_capture_disabled`
    # for the one escape hatch). The OTel LoggingHandler reads the active span
    # context, so each log record carries the trace_id/span_id that ties it to
    # the classifier's span waterfall, and the OTel Resource already carries
    # service.name — no per-record filter needed for that.
    otlp_log_h = _configure_log_export(agent_name)
    if otlp_log_h is not None:
        # Do NOT feed the spool handler's OWN transport/encoder logs back into
        # itself: on a hiccup httpx/urllib3/opentelemetry emit WARNING+
        # records which would otherwise re-enter the spool (a
        # log→export→log loop).
        _otlp_excluded = ("httpx", "httpcore", "urllib3", "grpc", "opentelemetry")
        otlp_log_h.addFilter(lambda r: not r.name.startswith(_otlp_excluded))
        root.addHandler(otlp_log_h)
    root.setLevel(level)

    for noisy in _NOISY_LOGGERS:
        logging.getLogger(noisy).setLevel(logging.WARNING)


def current_traceparent() -> Optional[str]:
    """Return the W3C traceparent header for the currently active OTel span.

    Returns ``None`` when no span is active or the span context is invalid.
    Callers pass this to loopback HTTP requests so downstream stages attach
    their spans to the same trace (same pattern the Rust daemon uses via
    ``crate::observability::current_traceparent()``).
    """
    span = trace.get_current_span()
    if not span.get_span_context().is_valid:
        return None
    carrier: dict[str, str] = {}
    TraceContextTextMapPropagator().inject(carrier)
    return carrier.get("traceparent")


def setup_agno_tracing():
    """Route agno's native (openinference) spans into the SAME standard OTel
    pipeline as everything else — the global `TracerProvider` `setup()`
    already installed (spool-backed, always-capture; see the module
    docstring). No separate SQLite store, no separate viewer: agno's spans
    just become more spans in the one exported/imported stream. A no-op with
    a warning if `openinference-instrumentation-agno` isn't installed.
    """
    try:
        from openinference.instrumentation.agno import AgnoInstrumentor
    except ImportError as e:
        logging.getLogger(__name__).warning(
            "setup_agno_tracing: dependency missing (%s); agno tracing disabled", e
        )
        return
    # No explicit tracer_provider — defaults to the global one `setup()` set.
    AgnoInstrumentor().instrument()
    logging.getLogger(__name__).info("setup_agno_tracing: agno spans -> standard OTel pipeline")


def preview(text: Optional[str], max_chars: int = 200) -> str:
    """Truncate text to `max_chars` for use as a span attribute value."""
    if not text:
        return ""
    return text[:max_chars] + ("…" if len(text) > max_chars else "")


def record_gen_params(
    span,
    *,
    temp: float,
    max_tokens: int,
    thinking_budget: int,
    budget_forced: bool,
    enable_thinking: bool = True,
    model: str = "",
) -> None:
    """Stamp the generation parameters used for ONE LLM call onto its span.

    Records the per-call sampling settings — the variable ``temp`` plus the
    shared constants from :mod:`agents.thinking` (top_p / top_k / presence /
    repetition penalty) — together with the thinking-budget config and whether
    the hard cap actually fired (``budget_forced``). Call inside the call's main
    span so every LLM span in the worklog trace shows exactly which parameters
    produced its output. ``model`` is set only when non-empty (some callers set
    it earlier on the span themselves).
    """
    from agents.thinking import (
        DEFAULT_TOP_P, DEFAULT_TOP_K, DEFAULT_PRESENCE_PENALTY,
        DEFAULT_REPETITION_PENALTY,
    )
    if model:
        span.set_attribute("model", model)
    span.set_attribute("temp", temp)
    span.set_attribute("top_p", DEFAULT_TOP_P)
    span.set_attribute("top_k", DEFAULT_TOP_K)
    span.set_attribute("presence_penalty", DEFAULT_PRESENCE_PENALTY)
    span.set_attribute("repetition_penalty", DEFAULT_REPETITION_PENALTY)
    span.set_attribute("thinking_budget", thinking_budget)
    span.set_attribute("enable_thinking", enable_thinking)
    span.set_attribute("max_tokens", max_tokens)
    span.set_attribute("budget_forced", budget_forced)


def record_fsm_params(
    span,
    *,
    temp: float,
    max_tokens: int,
    schema: str,
    model: str = "",
) -> None:
    """Stamp the ACTUAL decoding config for one FSM (grammar-constrained) JSON call.

    The FSM path (:mod:`agents.structured`) decodes against an outlines logits
    processor compiled from ``schema`` and samples among the grammar-legal tokens with
    ``temp`` / ``top_p`` / ``top_k`` (via ``make_sampler``). It runs with thinking OFF
    and — unlike the thinking path — applies NO presence/repetition penalty (outlines
    owns ``logits_processors``). This recorder reflects only what is genuinely applied,
    so the trace never advertises phantom penalties or a thinking budget that don't
    exist on these calls. Use it (not :func:`record_gen_params`) for the FSM endpoints.
    """
    from agents.thinking import DEFAULT_TOP_P, DEFAULT_TOP_K

    if model:
        span.set_attribute("model", model)
    span.set_attribute("decoding", "fsm")            # grammar-constrained (outlines)
    span.set_attribute("grammar_constrained", True)
    span.set_attribute("fsm_schema", schema)         # the Pydantic output schema enforced
    span.set_attribute("enable_thinking", False)     # thinking is off for FSM JSON calls
    span.set_attribute("temp", temp)
    span.set_attribute("top_p", DEFAULT_TOP_P)
    span.set_attribute("top_k", DEFAULT_TOP_K)
    span.set_attribute("max_tokens", max_tokens)


def record_llm_io(
    tracer,
    prefix: str,
    *,
    system_prompt: str,
    llm_input: str,
    llm_output: str,
    input_tokens: Optional[int] = None,
    output_tokens: Optional[int] = None,
    think_tokens: Optional[int] = None,
    max_input_chars: int = 8000,
    max_output_chars: int = 8000,
) -> None:
    """Emit the three `<prefix>.prompt` / `.input` / `.output` child spans that
    OpenObserve renders as dedicated Prompt / Input / Output panels.

    This is the same shape the ``activity_report`` endpoint uses, so every LLM
    call in the worklog trace (classify / propose / generate) is debuggable the
    same way: the exact system prompt, the exact user content, and the raw model
    output — plus token counts on the output span. Call this INSIDE the call's
    main span so the three land as its children.
    """
    with tracer.start_as_current_span(f"{prefix}.prompt") as sp:
        sp.set_attribute("total_chars", len(system_prompt or ""))
        sp.set_attribute("llm_input", preview(system_prompt, max_chars=max_input_chars))
    with tracer.start_as_current_span(f"{prefix}.input") as sp:
        sp.set_attribute("total_chars", len(llm_input or ""))
        sp.set_attribute("llm_input", preview(llm_input, max_chars=max_input_chars))
    with tracer.start_as_current_span(f"{prefix}.output") as sp:
        sp.set_attribute("total_chars", len(llm_output or ""))
        sp.set_attribute("llm_output", preview(llm_output, max_chars=max_output_chars))
        if input_tokens is not None:
            sp.set_attribute("input_tokens", input_tokens)
        if output_tokens is not None:
            sp.set_attribute("output_tokens", output_tokens)
        if think_tokens is not None:
            sp.set_attribute("think_tokens", think_tokens)


__all__ = [
    "setup",
    "extract_parent_context",
    "setup_agno_tracing",
    "current_traceparent",
    "preview",
    "record_gen_params",
    "record_fsm_params",
    "record_llm_io",
]
