"""OpenTelemetry + structured-logging bootstrap for Meridian Python agents.

A single `setup(agent_name)` call wires up:

  * an OTel `TracerProvider` with `service.name=agent_name`
  * a `BatchSpanProcessor` exporting OTLP/HTTP-protobuf spans to OpenObserve
  * a `LoggerProvider` + OTLP-logs handler so every `logging.LogRecord` is also
    shipped to OpenObserve (correlated to the active span), mirroring the Rust
    daemon's `OpenTelemetryTracingBridge`
  * W3C `TraceContextTextMapPropagator` as the global propagator so each
    agent can pick up the Rust daemon's `traceparent` and continue the trace
  * `LoggingInstrumentor` so every `logging.LogRecord` carries
    `otelTraceID` / `otelSpanID` attributes for OpenObserve correlation
  * a JSON formatter (`python-json-logger`) writing daily-rotated JSONL files
    under `~/.meridian/logs/{agent_name}.jsonl` plus stderr — both ingestable
    by OpenObserve's log pipeline without further parsing.

Export config (endpoint + Basic-auth credentials) is resolved from the SAME
`~/.meridian/settings.json` the Rust daemon reads — `otlp_enabled`,
`otlp_endpoint`, `oo_email`, `oo_password` — so the dashboard Settings page is
the single source of truth for both processes. The legacy `MERIDIAN_OO_AUTH`
env credential is deprecated and ignored, matching the daemon.

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
its own shipper.
"""
from __future__ import annotations

import base64
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
from opentelemetry.exporter.otlp.proto.http._log_exporter import OTLPLogExporter
from opentelemetry.exporter.otlp.proto.http.trace_exporter import (
    OTLPSpanExporter,
)
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
        tmp_path.write_bytes(payload)
        tmp_path.rename(final_path)
    except Exception as exc:
        logging.getLogger(__name__).warning(
            "telemetry spool write failed — payload dropped",
            extra={"signal": signal, "error": str(exc)},
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
DEFAULT_TRACES_ENDPOINT = "http://localhost:5080/api/default/v1/traces"
DEFAULT_LOGS_ENDPOINT   = "http://localhost:5080/api/default/v1/logs"
DEFAULT_LOG_DIR         = Path.home() / ".meridian" / "logs"
# Single source of truth for OpenObserve export config — the SAME file the Rust
# daemon reads (see `src/observability.rs::resolve_otlp_target`). Keeps the two
# processes credential-aligned: the dashboard Settings page writes here and both
# the daemon and this MLX server pick it up with no env plumbing.
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


# ──────────────────────── OTLP target resolution ───────────────────────────────
class _OtlpTarget:
    """Resolved OTLP export target: signal endpoint + Basic-auth header value."""

    __slots__ = ("traces_endpoint", "logs_endpoint", "headers")

    def __init__(self, traces_endpoint: str, logs_endpoint: str, headers: dict[str, str]):
        self.traces_endpoint = traces_endpoint
        self.logs_endpoint = logs_endpoint
        self.headers = headers


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
    so telemetry is captured even when OO credentials are absent (the shipper
    delivers when they are provided later).
    """
    if os.environ.get("MERIDIAN_TRACING_DISABLED", "").lower() in ("1", "true", "yes"):
        return False
    return bool(_load_settings().get("otlp_enabled"))


def _resolve_otlp_target() -> Optional[_OtlpTarget]:
    """Mirror of the Rust daemon's `resolve_otlp_target()`.

    Returns `None` (→ export disabled) when the toggle is off or credentials
    are missing. Endpoint precedence: settings.json `otlp_endpoint` → the
    `MERIDIAN_OTLP_TRACES_ENDPOINT`/`MERIDIAN_OTLP_ENDPOINT` env override →
    the localhost default. Auth is `base64(oo_email:oo_password)` — settings.json
    only; the legacy `MERIDIAN_OO_AUTH` env path is deprecated and ignored, the
    same decision the daemon made.
    """
    if os.environ.get("MERIDIAN_TRACING_DISABLED", "").lower() in ("1", "true", "yes"):
        return None

    settings = _load_settings()
    if not settings.get("otlp_enabled"):
        return None

    email = str(settings.get("oo_email") or "")
    password = str(settings.get("oo_password") or "")
    if not email or not password:
        return None
    # Guard against HTTP header injection / malformed user:password splits —
    # matches the daemon's same-named check.
    if any(c in email for c in "\r\n:") or any(c in password for c in "\r\n"):
        return None
    auth = base64.standard_b64encode(f"{email}:{password}".encode()).decode()

    configured = str(settings.get("otlp_endpoint") or "").strip()
    env_endpoint = (
        os.environ.get("MERIDIAN_OTLP_TRACES_ENDPOINT", "").strip()
        or os.environ.get("MERIDIAN_OTLP_ENDPOINT", "").strip()
    )
    traces_endpoint = configured or env_endpoint or DEFAULT_TRACES_ENDPOINT
    # OpenObserve serves logs at the sibling `/v1/logs` path; derive it from the
    # traces endpoint so a custom base host carries over to both signals.
    if traces_endpoint.endswith("/v1/traces"):
        logs_endpoint = traces_endpoint[: -len("/v1/traces")] + "/v1/logs"
    else:
        logs_endpoint = DEFAULT_LOGS_ENDPOINT

    return _OtlpTarget(traces_endpoint, logs_endpoint, {"Authorization": f"Basic {auth}"})


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
    """Build an OTLP-logs handler so every `log.*` record reaches OpenObserve,
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
    # Ship every record to OpenObserve via OTLP/HTTP logs too, when export is
    # configured. The OTel LoggingHandler reads the active span context, so each
    # OO log row carries the trace_id/span_id that ties it to the classifier's
    # span waterfall. No-op (None) when OTLP is disabled.
    # The OTLP handler already carries service.name via the OTel Resource, so it
    # needs no _ServiceFilter (that would duplicate the attribute on each record).
    otlp_log_h = _configure_log_export(agent_name)
    if otlp_log_h is not None:
        root.addHandler(otlp_log_h)
    root.setLevel(level)

    for noisy in _NOISY_LOGGERS:
        logging.getLogger(noisy).setLevel(logging.WARNING)


__all__ = ["setup", "extract_parent_context"]
