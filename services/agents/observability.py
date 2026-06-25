"""OpenTelemetry + structured-logging bootstrap for Meridian Python agents.

A single `setup(agent_name)` call wires up:

  * an OTel `TracerProvider` with `service.name=agent_name`
  * a `BatchSpanProcessor` writing OTLP/HTTP-protobuf spans to the durable
    telemetry spool (the Rust daemon's shipper drains it to OpenObserve)
  * a `LoggerProvider` + spool log handler so every `logging.LogRecord` is also
    spooled (correlated to the active span), mirroring the Rust daemon's
    `OpenTelemetryTracingBridge`
  * W3C `TraceContextTextMapPropagator` as the global propagator so each
    agent can pick up the Rust daemon's `traceparent` and continue the trace
  * `LoggingInstrumentor` so every `logging.LogRecord` carries
    `otelTraceID` / `otelSpanID` attributes for OpenObserve correlation
  * a JSON formatter (`python-json-logger`) writing daily-rotated JSONL files
    under `~/.meridian/logs/{agent_name}.jsonl` plus stderr — both ingestable
    by OpenObserve's log pipeline without further parsing.

Export is gated by the SAME `~/.meridian/settings.json` the Rust daemon reads —
the `otlp_enabled` toggle — so the dashboard Settings page is the single source
of truth for both processes. Delivery (endpoint + Basic-auth credentials) is
owned entirely by the Rust shipper; Python only ever writes to the spool. The
legacy `MERIDIAN_OO_AUTH` env credential is deprecated and ignored.

`extract_parent_context(traceparent)` is the helper agents use to continue
a span emitted by another process — typically the Rust ETL or another
agent stage.

Idempotent: calling `setup` twice is a no-op for the second call (returns
the existing tracer). This matters because both the daemon and the
single-shot CLI paths funnel through the same module.

Spool durability: when `otlp_enabled` is true (regardless of whether
credentials are present), spans and logs are written atomically to
`~/.meridian/telemetry/pending/<signal>-<unix_micros>-<seq>.otlp` via
`SpoolSpanExporter` and `SpoolLogExporter`.  The Rust daemon's shipper
task drains that shared directory to OpenObserve — Python does NOT need
its own shipper, and credential resolution lives there, not here.
"""
from __future__ import annotations

import json
import logging
import logging.handlers
import os
import sys
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
from opentelemetry.sdk._logs.export import BatchLogRecordProcessor, LogExportResult
from opentelemetry.sdk.resources import Resource
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor, SpanExportResult
from opentelemetry.trace.propagation.tracecontext import (
    TraceContextTextMapPropagator,
)
from pythonjsonlogger import jsonlogger


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
DEFAULT_LOG_DIR = Path.home() / ".meridian" / "logs"
# Single source of truth for the export TOGGLE — the SAME file the Rust daemon
# reads (see `src/observability.rs::resolve_otlp_target`). Delivery credentials
# live there too: the dashboard Settings page writes here and the Rust shipper
# picks them up. This process only reads `otlp_enabled` to decide whether to
# spool — it never delivers, so it needs no endpoint/credentials of its own.
_SETTINGS_PATH = Path(
    os.environ.get("MERIDIAN_SETTINGS_PATH")
    or (Path.home() / ".meridian" / "settings.json")
)

_NOISY_LOGGERS = ("urllib3", "httpx", "httpcore", "openai", "botocore")

# Track which agents have been configured so a second setup() call is a no-op.
_INITIALISED: dict[str, trace.Tracer] = {}
_PROCESS_SERVICE_NAME: str | None = None
# Held so shutdown() can flush log records the same way it flushes spans.
_LOGGER_PROVIDER: LoggerProvider | None = None


def _load_settings() -> dict[str, object]:
    """Read `~/.meridian/settings.json`; empty dict if absent/unreadable."""
    try:
        with _SETTINGS_PATH.open(encoding="utf-8") as fh:
            data = json.load(fh)
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


def _is_otlp_enabled() -> bool:
    """Return True when the otlp_enabled toggle is on and tracing is not disabled.

    Deliberately does NOT check credentials — used to gate the spool exporters
    so telemetry is captured even when OO credentials are absent (the Rust
    shipper delivers when they are provided later, and warns once if a user
    leaves the toggle on with no credentials).
    """
    if os.environ.get("MERIDIAN_TRACING_DISABLED", "").lower() in ("1", "true", "yes"):
        return False
    return bool(_load_settings().get("otlp_enabled"))


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
    """Flush and shut down the global TracerProvider.

    Must be called before a short-lived process exits — BatchSpanProcessor
    queues spans asynchronously and drops them on interpreter shutdown unless
    explicitly flushed.
    """
    provider = trace.get_tracer_provider()
    if hasattr(provider, "force_flush"):
        provider.force_flush(timeout_millis=5_000)
    if hasattr(provider, "shutdown"):
        provider.shutdown()

    # Flush queued log records too — BatchLogRecordProcessor drops them on
    # interpreter exit otherwise, the same hazard as spans.
    if _LOGGER_PROVIDER is not None:
        _LOGGER_PROVIDER.force_flush(timeout_millis=5_000)
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
def _configure_tracing(agent_name: str) -> None:
    resource = Resource.create({"service.name": agent_name})
    provider = TracerProvider(resource=resource)

    # Wire the spool exporter when otlp_enabled is true (even without creds).
    # The Rust shipper drains the shared spool dir when a target is available.
    if _is_otlp_enabled():
        provider.add_span_processor(BatchSpanProcessor(SpoolSpanExporter()))

    # Set as the global provider. OTel's `set_tracer_provider` warns if
    # someone already configured a provider in-process; we accept that and
    # overwrite — the agent process is the authority on its own telemetry.
    trace.set_tracer_provider(provider)
    set_global_textmap(TraceContextTextMapPropagator())


def _configure_log_export(agent_name: str) -> Optional[logging.Handler]:
    """Build a spool-logs handler so every `log.*` record reaches OpenObserve,
    correlated to the active span by trace_id/span_id — the Python counterpart
    of the Rust daemon's `OpenTelemetryTracingBridge`.

    Returns the handler (caller attaches it to root) or `None` when export is
    disabled, in which case logs still go to the JSONL file + stdout/stderr.

    When `otlp_enabled` is true the spool log exporter is always wired (even
    without credentials) — the Rust shipper handles delivery.
    """
    global _LOGGER_PROVIDER

    if not _is_otlp_enabled():
        return None

    resource = Resource.create({"service.name": agent_name})
    provider = LoggerProvider(resource=resource)
    provider.add_log_record_processor(BatchLogRecordProcessor(SpoolLogExporter()))
    set_logger_provider(provider)
    _LOGGER_PROVIDER = provider
    return LoggingHandler(level=logging.NOTSET, logger_provider=provider)


# ──────────────────────── Logging setup ────────────────────────────────────────
def _configure_logging(agent_name: str) -> None:
    log_dir = Path(os.environ.get("MERIDIAN_LOG_DIR") or DEFAULT_LOG_DIR)
    log_dir.mkdir(parents=True, exist_ok=True)
    log_path = log_dir / f"{agent_name}.jsonl"

    level_name = os.environ.get("LOG_LEVEL", "INFO").upper()
    level = getattr(logging, level_name, logging.INFO)

    # Hook the std-lib logging module so each LogRecord receives
    # otelTraceID / otelSpanID attributes from the active span context.
    # `set_logging_format=False` because we install our own JSON formatter.
    LoggingInstrumentor().instrument(set_logging_format=False)

    formatter = jsonlogger.JsonFormatter(
        "%(asctime)s %(levelname)s %(name)s %(message)s "
        "%(otelTraceID)s %(otelSpanID)s",
        rename_fields={
            "asctime":     "timestamp",
            "levelname":   "level",
            "name":        "logger",
            "otelTraceID": "trace_id",
            "otelSpanID":  "span_id",
        },
        json_default=str,
    )

    # Inject service.name on every record so a single OpenObserve stream can
    # be sliced per agent without parsing the logger name.
    class _ServiceFilter(logging.Filter):
        def filter(self, record: logging.LogRecord) -> bool:
            record.__dict__.setdefault("service.name", agent_name)
            return True

    file_h = logging.handlers.TimedRotatingFileHandler(
        log_path, when="midnight", backupCount=14, encoding="utf-8",
    )
    file_h.setFormatter(formatter)
    file_h.addFilter(_ServiceFilter())

    # Split streams by level so the launchd plist can route them to separate
    # files: INFO/DEBUG → stdout (mlx-server.log), WARNING+ → stderr
    # (mlx-server-error.log). Errors still appear in the file/JSONL handler too.
    stdout_h = logging.StreamHandler(sys.stdout)
    stdout_h.setFormatter(formatter)
    stdout_h.addFilter(_ServiceFilter())
    stdout_h.addFilter(lambda r: r.levelno < logging.WARNING)  # below WARNING only

    stderr_h = logging.StreamHandler(sys.stderr)
    stderr_h.setFormatter(formatter)
    stderr_h.addFilter(_ServiceFilter())
    stderr_h.setLevel(logging.WARNING)  # WARNING / ERROR / CRITICAL only

    root = logging.getLogger()
    # Clear any pre-existing handlers — long-running daemons that import
    # third-party libs (mcp, etc.) often leave a default basicConfig handler
    # behind that would duplicate every line.
    root.handlers.clear()
    root.addHandler(file_h)
    root.addHandler(stdout_h)
    root.addHandler(stderr_h)
    # Spool every record for OpenObserve via OTLP/HTTP logs too, when export is
    # enabled. The OTel LoggingHandler reads the active span context, so each
    # OO log row carries the trace_id/span_id that ties it to the classifier's
    # span waterfall. No-op (None) when OTLP is disabled.
    # The spool handler already carries service.name via the OTel Resource, so it
    # needs no _ServiceFilter (that would duplicate the attribute on each record).
    otlp_log_h = _configure_log_export(agent_name)
    if otlp_log_h is not None:
        # Do NOT feed the spool handler's OWN transport/encoder logs back into
        # the spool: on a hiccup httpx/urllib3/opentelemetry emit WARNING+
        # records which this root handler would try to spool → more failures (a
        # log→export→log loop). Drop those from THIS handler only — they still
        # reach the file/stderr handlers.
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


def instrument_agno() -> None:
    """Instrument the agno framework for OpenTelemetry tracing.

    No-op when openinference-instrumentation-agno is not installed; the package
    is optional and the server must start cleanly without it.
    """
    try:
        from opentelemetry.instrumentation.agno import AgnoInstrumentor  # type: ignore[import]
        AgnoInstrumentor().instrument()
    except ImportError:
        logging.getLogger(__name__).debug(
            "opentelemetry-instrumentation-agno not installed; agno spans will not be exported"
        )


def preview(text: Optional[str], max_chars: int = 200) -> str:
    """Truncate text to `max_chars` for use as a span attribute value."""
    if not text:
        return ""
    return text[:max_chars] + ("…" if len(text) > max_chars else "")


__all__ = ["setup", "extract_parent_context", "instrument_agno", "current_traceparent", "preview"]
