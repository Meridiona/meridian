"""OpenTelemetry + structured-logging bootstrap for Meridian Python agents.

A single `setup(agent_name)` call wires up:

  * an OTel `TracerProvider` with `service.name=agent_name`
  * a `BatchSpanProcessor` exporting OTLP/HTTP-protobuf spans to OpenObserve
    (`MERIDIAN_OTLP_TRACES_ENDPOINT`, with Basic auth via `MERIDIAN_OO_AUTH`)
  * W3C `TraceContextTextMapPropagator` as the global propagator so each
    agent can pick up the Rust daemon's `traceparent` and continue the trace
  * `LoggingInstrumentor` so every `logging.LogRecord` carries
    `otelTraceID` / `otelSpanID` attributes for OpenObserve correlation
  * a JSON formatter (`python-json-logger`) writing daily-rotated JSONL files
    under `~/.meridian/logs/{agent_name}.jsonl` plus stderr — both ingestable
    by OpenObserve's log pipeline without further parsing.

`extract_parent_context(traceparent)` is the helper agents use to continue
a span emitted by another process — typically the Rust ETL or another
agent stage.

Idempotent: calling `setup` twice is a no-op for the second call (returns
the existing tracer). This matters because both the daemon and the
single-shot CLI paths funnel through the same module.
"""
from __future__ import annotations

import logging
import logging.handlers
import os
import sys
from pathlib import Path
from typing import Optional

from opentelemetry import trace
from opentelemetry.context import Context
from opentelemetry.exporter.otlp.proto.http.trace_exporter import (
    OTLPSpanExporter,
)
from opentelemetry.instrumentation.logging import LoggingInstrumentor
from opentelemetry.propagate import set_global_textmap
from opentelemetry.sdk.resources import Resource
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.trace.propagation.tracecontext import (
    TraceContextTextMapPropagator,
)
from pythonjsonlogger import jsonlogger


# ──────────────────────── Config ───────────────────────────────────────────────
DEFAULT_TRACES_ENDPOINT = "http://localhost:5080/api/default/v1/traces"
DEFAULT_LOGS_ENDPOINT   = "http://localhost:5080/api/default/v1/logs"
DEFAULT_LOG_DIR         = Path.home() / ".meridian" / "logs"

_NOISY_LOGGERS = ("urllib3", "httpx", "httpcore", "openai", "botocore")

# Track which agents have been configured so a second setup() call is a no-op.
_INITIALISED: dict[str, trace.Tracer] = {}
_PROCESS_SERVICE_NAME: str | None = None


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

    disabled = os.environ.get("MERIDIAN_TRACING_DISABLED", "").lower() in ("1", "true", "yes")
    endpoint = (
        os.environ.get("MERIDIAN_OTLP_TRACES_ENDPOINT", "").strip()
        or os.environ.get("MERIDIAN_OTLP_ENDPOINT", "").strip()
    )
    if not disabled and endpoint:
        headers: dict[str, str] = {}
        auth = os.environ.get("MERIDIAN_OO_AUTH")
        if auth:
            headers["Authorization"] = f"Basic {auth}"
        exporter = OTLPSpanExporter(endpoint=endpoint, headers=headers)
        provider.add_span_processor(BatchSpanProcessor(exporter))

    # Set as the global provider. OTel's `set_tracer_provider` warns if
    # someone already configured a provider in-process; we accept that and
    # overwrite — the agent process is the authority on its own telemetry.
    trace.set_tracer_provider(provider)
    set_global_textmap(TraceContextTextMapPropagator())


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
    # third-party libs (hermes, mcp) often leave a default basicConfig handler
    # behind that would duplicate every line.
    root.handlers.clear()
    root.addHandler(file_h)
    root.addHandler(stdout_h)
    root.addHandler(stderr_h)
    root.setLevel(level)

    for noisy in _NOISY_LOGGERS:
        logging.getLogger(noisy).setLevel(logging.WARNING)


__all__ = ["setup", "extract_parent_context"]
