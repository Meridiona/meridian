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
  * a JSON formatter (`python-json-logger`) writing to stdout/stderr — ingestable
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
import os
import re
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


# ──────────────────────── Secret scrubbing + previews ───────────────────────────
#
# Span text attributes ship raw screen OCR and rendered LLM prompts/outputs to
# OpenObserve. Before any of that leaves the process we (1) redact known secret
# token shapes and (2) cap the length. `preview()` is the helper call sites use
# for attributes we set by hand; `_scrub_span_attributes()` is applied to EVERY
# exported span (in SpoolSpanExporter) so agno's auto-captured OpenInference
# text attributes (`input.value` / `output.value` / `llm.*_messages.*`) get the
# same treatment without each call site having to know about them.

_PREVIEW_CAP = 4000

# Ported from the old pm_worklog_update ProjectSecretGuard. Each pattern redacts
# a credential shape to «redacted:KIND» so a leaked token can't reach OO.
_SECRET_PATTERNS: tuple[tuple["re.Pattern[str]", str], ...] = (
    (re.compile(r"sk-[A-Za-z0-9_\-]{16,}"),        "openai-key"),
    (re.compile(r"ATATT3[A-Za-z0-9_\-=.]{16,}"),   "atlassian-token"),
    (re.compile(r"gh[poasu]_[A-Za-z0-9]{20,}"),    "github-token"),
    (re.compile(r"AKIA[0-9A-Z]{16}"),              "aws-access-key"),
)


def scrub_secrets(text: str) -> str:
    """Redact known secret token shapes from `text`. Idempotent."""
    if not text:
        return text
    for pat, kind in _SECRET_PATTERNS:
        text = pat.sub(f"«redacted:{kind}»", text)
    return text


def preview(text: object, cap: int = _PREVIEW_CAP) -> str:
    """Scrub secrets, then cap to `cap` chars with a `…(+N chars)` marker.

    Use for EVERY free-text span attribute set by hand (prompts, outputs,
    compressed session_text). Accepts any object — coerced to str first.
    """
    if text is None:
        return ""
    s = scrub_secrets(str(text))
    if len(s) > cap:
        return f"{s[:cap]}…(+{len(s) - cap} chars)"
    return s


# Attribute keys whose values are free text captured from the model/user and so
# must be scrubbed+capped before export. Covers OpenInference's conventions (which
# vary by version: `input.value`/`output.value` on chain/agent spans, AND
# `llm.input`/`llm.output`/`llm.input_messages.*` on the LLM span) plus our own
# `*_preview` keys (already scrubbed at source — re-scrub is harmless).
_SCRUB_KEY_EXACT = frozenset({"input.value", "output.value"})
# Prefixes that mark FREE-TEXT prompt/output content. Note `llm.input`/`llm.output`
# are caught here, while `llm.token_count.*` / `llm.model_name` /
# `llm.invocation_parameters` are NOT (they don't start with these) — so numbers
# and config stay intact.
_SCRUB_KEY_PREFIX = ("llm.input", "llm.output", "llm.prompt")
_SCRUB_KEY_SUFFIX = ("_preview",)


def _scrub_span_attributes(span) -> None:
    """In-place scrub+cap of free-text attributes on a ReadableSpan before export.

    Mutates the span's backing attribute map. This is the only safe interception
    point for agno's auto-captured prompt/output values (they're set deep inside
    OpenInference, never through `preview()`).
    """
    attrs = getattr(span, "_attributes", None)
    if not attrs:
        return
    try:
        for key in list(attrs.keys()):
            val = attrs[key]
            if not isinstance(val, str):
                continue
            if (
                key in _SCRUB_KEY_EXACT
                or key.startswith(_SCRUB_KEY_PREFIX)
                or key.endswith(_SCRUB_KEY_SUFFIX)
                or key.endswith(".value")
                or ".message.content" in key
            ):
                attrs[key] = preview(val)
    except Exception:  # noqa: BLE001 — scrubbing must never break export
        pass


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
            # Scrub+cap free-text attributes (incl. agno's auto-captured prompts
            # / outputs) before they're serialised and shipped to OpenObserve.
            for _s in spans:
                _scrub_span_attributes(_s)
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


def current_traceparent() -> Optional[str]:
    """W3C `traceparent` of the currently-active span, or `None` if none/invalid.

    Used to hand a parent span's identity to an out-of-band continuation (e.g.
    the loopback stage HTTP calls under /worklog_hour, so they nest under the
    root span rather than under the original Rust caller).
    """
    ctx = trace.get_current_span().get_span_context()
    if not ctx.is_valid:
        return None
    return (
        f"00-{ctx.trace_id:032x}-{ctx.span_id:016x}-{int(ctx.trace_flags):02x}"
    )


_AGNO_INSTRUMENTED = False


# agno builds three FLAT (non-dotted, so non-hierarchical) loggers, each with its
# own console RichHandler and `propagate=False` — so by default agno's run/step
# logs never reach Meridian's root handlers (file / stdout / the OTLP spool →
# OpenObserve). They must be adopted explicitly to be queryable in OO.
_AGNO_LOGGERS = ("agno", "agno-team", "agno-workflow")


def _route_agno_logs_to_root() -> None:
    """Re-parent agno's loggers onto Meridian's root handlers.

    Drops agno's console-only RichHandler and enables propagation, so agno's
    Agent/Workflow log records flow through the same file + stdout + OTLP-spool
    handlers as the rest of the service — landing in OpenObserve with the active
    span's trace_id/span_id attached (via LoggingInstrumentor). Idempotent.
    """
    for name in _AGNO_LOGGERS:
        lg = logging.getLogger(name)
        lg.handlers.clear()
        lg.propagate = True


_AGNO_DB_ATTACHED = False


def _agno_trace_db_file() -> str:
    """Path for agno's native traces DB (`agno_traces`/`agno_spans` tables).

    `AGNO_TRACE_DB` overrides; otherwise a dedicated sibling of meridian.db
    (NOT meridian.db itself — agno's docs warn against mixing observability into
    the app DB).
    """
    explicit = os.environ.get("AGNO_TRACE_DB")
    if explicit:
        return str(Path(explicit).expanduser())
    base = os.environ.get("MERIDIAN_DB") or "~/.meridian/meridian.db"
    return str(Path(base).expanduser().parent / "agno_traces.db")


def _attach_agno_db_exporter() -> bool:
    """Add agno's `DatabaseSpanExporter` as a SECOND sink on our existing
    provider, so agno's Agent/Model/tool spans land in the native
    `agno_traces`/`agno_spans` tables in addition to OpenObserve.

    A second processor on the same provider — NOT agno's `setup_tracing()`, which
    would create its own bare provider and (via its `isinstance` guard /
    `set_tracer_provider`) either no-op or clobber ours, killing the OpenObserve
    export. This sink is local-only and independent of the OTLP toggle. Set
    `AGNO_TRACE_DB=` (empty) — handled by the caller — to skip. Best-effort:
    a missing agno DB module just logs a warning and is skipped.

    NOTE: this local DB is written UNSCRUBBED (full prompts/outputs), matching the
    trust level of meridian.db which already holds raw transcripts on the same
    disk. Only the OpenObserve sink is secret-scrubbed (data leaving the host).
    """
    global _AGNO_DB_ATTACHED
    if _AGNO_DB_ATTACHED:
        return True
    if os.environ.get("AGNO_TRACE_DB") == "":  # explicit opt-out
        return False
    provider = trace.get_tracer_provider()
    if not isinstance(provider, TracerProvider):
        logging.getLogger(__name__).warning(
            "agno trace DB sink skipped — no SDK TracerProvider (call setup() first)"
        )
        return False
    try:
        from agno.db.sqlite import SqliteDb
        from agno.tracing.exporter import DatabaseSpanExporter
    except Exception as exc:  # noqa: BLE001
        logging.getLogger(__name__).warning(
            "agno trace DB sink unavailable", extra={"error": str(exc)}
        )
        return False
    try:
        db_file = _agno_trace_db_file()
        Path(db_file).parent.mkdir(parents=True, exist_ok=True)
        exporter = DatabaseSpanExporter(db=SqliteDb(db_file=db_file))
        provider.add_span_processor(BatchSpanProcessor(exporter))
        _AGNO_DB_ATTACHED = True
        logging.getLogger(__name__).info(
            "agno trace DB sink attached", extra={"db_file": db_file}
        )
        return True
    except Exception as exc:  # noqa: BLE001
        logging.getLogger(__name__).warning(
            "agno trace DB sink failed", extra={"error": str(exc)}
        )
        return False


def instrument_agno() -> bool:
    """Auto-instrument agno (Agent/Workflow runs) onto the global TracerProvider.

    Routes agno's OpenInference spans — rendered prompt, raw output, token
    counts, step path — through the same `SpoolSpanExporter` → OpenObserve as the
    rest of Meridian's telemetry, attaches agno's native `DatabaseSpanExporter` as
    a second local sink (see `_attach_agno_db_exporter`), AND re-parents agno's own
    loggers so its logs reach OO too (see `_route_agno_logs_to_root`). Idempotent
    and best-effort: if the optional `openinference-instrumentation-agno` package
    is absent, logs a warning and returns False so the server still boots — but
    log routing + the DB sink are applied regardless, since they're independent of
    the tracing package. Free-text values are scrubbed+capped on export to
    OpenObserve by `_scrub_span_attributes`.
    """
    global _AGNO_INSTRUMENTED
    # Always route agno logs to OO + attach the local DB sink, even if the
    # OpenInference tracing package is unavailable.
    _route_agno_logs_to_root()
    _attach_agno_db_exporter()
    if _AGNO_INSTRUMENTED:
        return True
    try:
        from openinference.instrumentation.agno import AgnoInstrumentor
    except Exception as exc:  # noqa: BLE001
        logging.getLogger(__name__).warning(
            "agno tracing disabled — openinference-instrumentation-agno missing",
            extra={"error": str(exc)},
        )
        return False
    try:
        AgnoInstrumentor().instrument(tracer_provider=trace.get_tracer_provider())
        _AGNO_INSTRUMENTED = True
        logging.getLogger(__name__).info("agno OpenTelemetry instrumentation enabled")
        return True
    except Exception as exc:  # noqa: BLE001
        logging.getLogger(__name__).warning(
            "agno instrumentation failed", extra={"error": str(exc)}
        )
        return False


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

    # Split streams by level so the launchd plist can route them to separate
    # files: INFO/DEBUG → stdout, WARNING+ → stderr.
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
        # reach stderr.
        _otlp_excluded = ("httpx", "httpcore", "urllib3", "grpc", "opentelemetry")
        otlp_log_h.addFilter(lambda r: not r.name.startswith(_otlp_excluded))
        root.addHandler(otlp_log_h)
    root.setLevel(level)

    for noisy in _NOISY_LOGGERS:
        logging.getLogger(noisy).setLevel(logging.WARNING)


__all__ = [
    "setup",
    "extract_parent_context",
    "current_traceparent",
    "instrument_agno",
    "scrub_secrets",
    "preview",
]
