"""OpenTelemetry + structured-logging bootstrap for Meridian Python agents.

A single `setup(agent_name)` call wires up:

  * an OTel `TracerProvider` with `service.name=agent_name`
  * direct OTLP/HTTP export to OpenObserve when `otlp_enabled=true` and
    credentials are set in `~/.meridian/settings.json` — no Rust daemon needed
  * spool fallback (`~/.meridian/telemetry/pending/`) when toggle is on but
    credentials are absent; the Rust daemon drains that directory to OO
  * a `LoggerProvider` + matching log handler so every `logging.LogRecord`
    is correlated to the active span via trace_id/span_id
  * W3C `TraceContextTextMapPropagator` as the global propagator so each
    agent can pick up the Rust daemon's `traceparent` and continue the trace
  * `LoggingInstrumentor` so every `logging.LogRecord` carries
    `otelTraceID` / `otelSpanID` attributes for OpenObserve correlation
  * a JSON formatter (`python-json-logger`) writing daily-rotated JSONL files
    under `~/.meridian/logs/{agent_name}.jsonl` plus stderr

Export gate: `~/.meridian/settings.json` `otlp_enabled` toggle + OO credentials
(`oo_email` / `oo_password`). When both are present the Python service ships
directly to OO — it does NOT depend on the Rust daemon for delivery. The spool
path is a fallback for when credentials are absent (daemon delivers later).

`extract_parent_context(traceparent)` is the helper agents use to continue
a span emitted by another process — typically the Rust ETL or another
agent stage.

Idempotent: calling `setup` twice is a no-op for the second call (returns
the existing tracer).
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
from opentelemetry.sdk._logs.export import BatchLogRecordProcessor, SimpleLogRecordProcessor, LogExportResult
from opentelemetry.sdk.resources import Resource
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor, SimpleSpanProcessor, SpanExportResult
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
# Same settings.json the Rust daemon reads. When otlp_enabled=true and
# oo_email/oo_password are set, Python ships directly to OO (no daemon needed).
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
    """Return True when the otlp_enabled toggle is on and tracing is not disabled."""
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
def _oo_otlp_headers() -> dict[str, str]:
    """Build the Basic-auth header for direct OO OTLP export from settings.json."""
    import base64
    s = _load_settings()
    email = s.get("oo_email") or ""
    passwd = s.get("oo_password") or ""
    if not email or not passwd:
        return {}
    token = base64.b64encode(f"{email}:{passwd}".encode()).decode()
    return {"Authorization": f"Basic {token}"}


def _oo_otlp_base_url() -> str:
    """Return the OO OTLP base URL (without /v1/traces or /v1/logs suffix)."""
    s = _load_settings()
    endpoint = s.get("otlp_endpoint") or "http://localhost:5080/api/default/v1/traces"
    # Strip the /v1/traces suffix to get the org-level base used by both signals.
    for suffix in ("/v1/traces", "/v1/logs"):
        if endpoint.endswith(suffix):
            return endpoint[: -len(suffix)]
    return endpoint.rstrip("/")


def _configure_tracing(agent_name: str) -> None:
    # The W3C propagator is always installed so traceparent round-trips work.
    set_global_textmap(TraceContextTextMapPropagator())

    if not _is_otlp_enabled():
        return

    resource = Resource.create({"service.name": agent_name})
    provider = TracerProvider(resource=resource)

    headers = _oo_otlp_headers()
    if headers:
        # Credentials present — ship directly to OO, no Rust daemon dependency.
        from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
        from opentelemetry.sdk.trace.export import BatchSpanProcessor
        exporter = OTLPSpanExporter(
            endpoint=f"{_oo_otlp_base_url()}/v1/traces",
            headers=headers,
        )
        provider.add_span_processor(BatchSpanProcessor(exporter))
    else:
        # Credentials absent — spool for the Rust daemon to deliver later.
        # BatchSpanProcessor avoids blocking inference threads on each span end.
        provider.add_span_processor(BatchSpanProcessor(SpoolSpanExporter()))

    trace.set_tracer_provider(provider)


def _configure_log_export(agent_name: str) -> Optional[logging.Handler]:
    """Build an OTel log handler so every ``log.*`` record reaches OpenObserve,
    correlated to the active span by trace_id/span_id.

    Returns the handler (caller attaches it to root) or ``None`` when export is
    disabled — logs still go to the JSONL file + stdout/stderr regardless.
    """
    global _LOGGER_PROVIDER

    if not _is_otlp_enabled():
        return None

    headers = _oo_otlp_headers()
    resource = Resource.create({"service.name": agent_name})
    provider = LoggerProvider(resource=resource)

    if headers:
        # Credentials present — ship directly to OO, no Rust daemon dependency.
        from opentelemetry.exporter.otlp.proto.http._log_exporter import OTLPLogExporter
        log_exporter = OTLPLogExporter(
            endpoint=f"{_oo_otlp_base_url()}/v1/logs",
            headers=headers,
        )
    else:
        # Credentials absent — spool for the Rust daemon to deliver later.
        log_exporter = SpoolLogExporter()  # type: ignore[assignment]

    # BatchLogRecordProcessor for both paths so log export never blocks callers.
    provider.add_log_record_processor(BatchLogRecordProcessor(log_exporter))
    set_logger_provider(provider)
    _LOGGER_PROVIDER = provider
    return LoggingHandler(level=logging.NOTSET, logger_provider=provider)


# ──────────────────────── Logging setup ────────────────────────────────────────
def _configure_logging(agent_name: str) -> None:
    log_dir = Path(os.environ.get("MERIDIAN_LOG_DIR") or DEFAULT_LOG_DIR)
    log_dir.mkdir(parents=True, exist_ok=True)
    # Sanitise agent_name to prevent path traversal — only allow chars that are
    # safe as a filename component (alphanumeric, hyphen, underscore).
    safe_name = "".join(c if c.isalnum() or c in "-_" else "_" for c in agent_name)
    if not safe_name:
        safe_name = "agent"
    log_path = log_dir / f"{safe_name}.jsonl"

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


# Process-level handle so the agno TracerProvider isn't garbage-collected.
_AGNO_TRACER_PROVIDER = None


def _build_agno_db_exporter(db, workflow_id: str, agent_id: str):
    """agno ``DatabaseSpanExporter`` subclass that stamps the pipeline identity
    onto each trace's ROOT span.

    AgentOS hides any trace whose ``workflow_id`` AND ``agent_id`` are both
    null. With OpenObserve off, the meridian wrapper span (``worklog.hour``) is
    non-recording, so agno's worklog Workflow runs as its OWN root trace; its
    root ``*.run`` span is parentless here and gets the ids, surfacing the trace
    in the dashboard. Child spans are written unchanged.
    """
    from collections import defaultdict

    from agno.tracing.exporter import DatabaseSpanExporter
    from agno.tracing.schemas import Span
    from opentelemetry.sdk.trace.export import SpanExportResult

    class _AgnoDbExporter(DatabaseSpanExporter):
        def export(self, spans):  # type: ignore[override]
            if self._shutdown:
                return SpanExportResult.FAILURE
            if not spans:
                return SpanExportResult.SUCCESS
            converted = []
            for s in spans:
                try:
                    cs = Span.from_otel_span(s)
                except Exception:  # noqa: BLE001 — skip a span we can't convert
                    continue
                if not cs.parent_span_id:
                    attrs = dict(cs.attributes or {})
                    attrs.setdefault("workflow_id", workflow_id)
                    attrs.setdefault("agent_id", agent_id)
                    # Group hour-runs into one AgentOS session. agno normally
                    # stamps session_id when run(session_id=...) is passed; this
                    # is a fallback that derives the day-level session from the
                    # run_id ("wl-<day>T<hh>" → "wl-<day>") so the trace always
                    # surfaces under a session in the dashboard.
                    if not (attrs.get("session_id") or attrs.get("agno.session.id")):
                        rid = attrs.get("run_id") or attrs.get("agno.run.id") or ""
                        if isinstance(rid, str) and "T" in rid:
                            attrs["session_id"] = rid.rsplit("T", 1)[0]
                    cs.attributes = attrs
                converted.append(cs)
            if not converted:
                return SpanExportResult.SUCCESS
            by_trace: dict = defaultdict(list)
            for cs in converted:
                by_trace[cs.trace_id].append(cs)
            try:
                self._export_sync(by_trace)  # SqliteDb is synchronous
            except Exception as e:  # noqa: BLE001
                logging.getLogger(__name__).warning("agno trace export failed: %s", e)
                return SpanExportResult.FAILURE
            return SpanExportResult.SUCCESS

    return _AgnoDbExporter(db=db)


def setup_agno_tracing(
    db_file: Optional[str] = None,
    workflow_id: str = "worklog-hour",
    agent_id: str = "meridian-worklog-pipeline",
):
    """Route agno's native (openinference) spans — and ONLY those — to a SqliteDb
    the AgentOS viewer reads.

    The TracerProvider here is EXPLICIT, not global: meridian's own manual spans
    live on the no-op global provider and never reach this exporter, so the
    dashboard shows agno's ``tracing=True`` output alone. Idempotent per process.
    Returns the provider (or ``None`` if agno/openinference aren't installed).
    """
    global _AGNO_TRACER_PROVIDER
    if _AGNO_TRACER_PROVIDER is not None:
        return _AGNO_TRACER_PROVIDER
    try:
        from agno.db.sqlite import SqliteDb
        from openinference.instrumentation.agno import AgnoInstrumentor
        from opentelemetry.sdk.trace import TracerProvider as _TP
        from opentelemetry.sdk.trace.export import SimpleSpanProcessor
    except ImportError as e:
        logging.getLogger(__name__).warning(
            "setup_agno_tracing: dependencies missing (%s); agno tracing disabled", e
        )
        return None

    path = db_file or os.environ.get("AGNO_TRACE_DB") or str(
        Path("~/.meridian/agno_traces.db").expanduser()
    )
    path = str(Path(path).expanduser())
    exporter = _build_agno_db_exporter(SqliteDb(db_file=path), workflow_id, agent_id)
    provider = _TP()
    provider.add_span_processor(SimpleSpanProcessor(exporter))
    AgnoInstrumentor().instrument(tracer_provider=provider)
    _AGNO_TRACER_PROVIDER = provider
    logging.getLogger(__name__).info("setup_agno_tracing: agno spans -> %s", path)
    return provider


def preview(text: Optional[str], max_chars: int = 200) -> str:
    """Truncate text to `max_chars` for use as a span attribute value."""
    if not text:
        return ""
    return text[:max_chars] + ("…" if len(text) > max_chars else "")


__all__ = [
    "setup",
    "extract_parent_context",
    "instrument_agno",
    "setup_agno_tracing",
    "current_traceparent",
    "preview",
]
