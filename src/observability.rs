//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Observability bootstrap.
//
// One call to `init(service_name)` builds a layered `tracing` subscriber that:
//   1. Pretty-prints to stdout (so `meridian logs` / daemon.log captures it)
//   2. Writes JSON Lines to `~/.meridian/logs/<service>.jsonl` with daily rotation
//   3. Exports OpenTelemetry traces to OpenObserve via OTLP/HTTP
//   4. Exports OpenTelemetry logs  to OpenObserve via OTLP/HTTP
//      (log events carry trace_id/span_id so they correlate with traces in OO)
//
// Environment variables read at init time:
//   MERIDIAN_OTLP_ENDPOINT  — OTLP/HTTP traces endpoint override
//                              (default: http://localhost:5080/api/default/v1/traces)
//   MERIDIAN_LOG_DIR        — log directory (default: ~/.meridian/logs)
//   RUST_LOG                — standard env-filter; default
//                              "meridian=info,meridian::etl=debug,sqlx=warn"
//
// OpenObserve credentials come from settings.json (oo_email/oo_password, set in
// the dashboard Settings). The MERIDIAN_OO_AUTH env var is DEPRECATED and
// ignored; export is skipped when settings carry no credentials.
//
// `init` returns an `ObservabilityGuard` whose `Drop` flushes the file writer.
// Call `ObservabilityGuard::shutdown().await` before tearing down the tokio
// runtime so the batch exporters flush their final payloads.

use anyhow::{Context, Result};
use opentelemetry::{global, trace::TracerProvider as _, KeyValue};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{Protocol, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::{
    logs::LoggerProvider,
    propagation::TraceContextPropagator,
    runtime,
    trace::{RandomIdGenerator, Sampler, Tracer, TracerProvider},
    Resource,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{layer::SubscriberExt, reload, util::SubscriberInitExt, EnvFilter};

const DEFAULT_OTLP_ENDPOINT: &str = "http://localhost:5080/api/default/v1/traces";

/// Type alias for the hot-reload handle. The `S = Registry` parameter reflects
/// that the reload layer is installed directly on `tracing_subscriber::Registry`
/// (it is the first layer added, before any OTel or fmt layers).
type FilterHandle = reload::Handle<EnvFilter, tracing_subscriber::Registry>;

/// Global handle for hot-reloading the `EnvFilter` without restarting the daemon.
/// Set once during `init()`; accessed from the poll loop via `reload_log_level()`.
static FILTER_HANDLE: OnceLock<FilterHandle> = OnceLock::new();

/// RAII guard returned from [`init`]. Holds the file-writer worker thread and
/// (when OTel is enabled) the logger provider for graceful shutdown.
///
/// Call [`ObservabilityGuard::shutdown`] explicitly before the tokio runtime
/// is torn down — the BatchSpanProcessor's shutdown is blocking, and a Drop
/// inside an async context panics with "Cannot drop a runtime in a context
/// where blocking is not allowed". Drop here just releases the file writer.
pub struct ObservabilityGuard {
    _file_guard: WorkerGuard,
    logger_provider: Option<LoggerProvider>,
    otel_enabled: bool,
}

impl ObservabilityGuard {
    /// Flush and shut down both OTel exporters (traces + logs). Must be
    /// `await`ed while the tokio runtime is still alive.
    pub async fn shutdown(self) {
        if self.otel_enabled {
            if let Some(lp) = self.logger_provider {
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = lp.shutdown();
                })
                .await;
            }
            let _ = tokio::task::spawn_blocking(global::shutdown_tracer_provider).await;
        }
    }
}

/// Initialise the layered tracing subscriber.
///
/// `service_name` becomes the OTel `service.name` resource attribute and the
/// log file prefix (e.g. "meridian-rust" → `meridian-rust.jsonl`).
pub fn init(service_name: &str) -> Result<ObservabilityGuard> {
    let log_dir = resolve_log_dir()?;
    std::fs::create_dir_all(&log_dir)
        .with_context(|| format!("create log dir {}", log_dir.display()))?;

    let file_appender = tracing_appender::rolling::daily(&log_dir, format!("{service_name}.jsonl"));
    let (file_writer, file_guard) = tracing_appender::non_blocking(file_appender);

    // Build the env filter from RUST_LOG if set; otherwise derive from settings.log_level.
    let settings_log_level = crate::config::load_runtime_settings().log_level;
    let default_filter = build_default_filter(&settings_log_level);
    let initial_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&default_filter));

    // Wrap the filter in a reload layer so the poll loop can update it at runtime
    // via `reload_log_level()` without restarting the daemon.
    let (reload_layer, filter_handle) = reload::Layer::new(initial_filter);
    let _ = FILTER_HANDLE.set(filter_handle);
    // We need to move reload_layer into exactly one subscriber init branch below.
    // Using Option::take() satisfies the borrow checker since only one branch runs.
    let mut rl = Some(reload_layer);

    // stdout: everything (INFO+). This is what `meridian logs` / daemon.log shows.
    let fmt_stdout = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_writer(std::io::stdout)
        .compact();
    // stderr: WARN+ERROR only — a filtered view so `meridian logs daemon-error`
    // surfaces just the problems. Errors still appear in stdout/daemon.log too.
    use tracing_subscriber::filter::LevelFilter;
    use tracing_subscriber::Layer as _;
    let fmt_stderr = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_writer(std::io::stderr)
        .compact()
        .with_filter(LevelFilter::WARN);
    let fmt_file = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_writer(file_writer)
        .json()
        .with_current_span(true)
        .with_span_list(false);

    // Build OTel providers first (no generic subscriber type involved yet),
    // then construct the layers inline so the subscriber type is concrete at
    // each .with() call — this avoids the Box<dyn Layer<S>> type-erasure issue
    // that arises when chaining two boxed layers with different subscriber types.
    let (otel_enabled, logger_provider) = match try_build_otel_providers(service_name) {
        Ok(Some((tracer, lp))) => {
            let trace_layer = tracing_opentelemetry::layer()
                .with_tracer(tracer)
                .with_tracked_inactivity(false);
            let log_layer = OpenTelemetryTracingBridge::new(&lp);

            tracing_subscriber::registry()
                .with(rl.take().unwrap())
                .with(fmt_stdout)
                .with(fmt_stderr)
                .with(fmt_file)
                .with(trace_layer)
                .with(log_layer)
                .init();

            (true, Some(lp))
        }
        Ok(None) => {
            tracing_subscriber::registry()
                .with(rl.take().unwrap())
                .with(fmt_stdout)
                .with(fmt_stderr)
                .with(fmt_file)
                .init();
            (false, None)
        }
        Err(err) => {
            eprintln!("observability: OTLP exporter init failed: {err:#}");
            tracing_subscriber::registry()
                .with(rl.take().unwrap())
                .with(fmt_stdout)
                .with(fmt_stderr)
                .with(fmt_file)
                .init();
            (false, None)
        }
    };

    // W3C trace-context propagator so we can inject/extract `traceparent` strings
    // across process boundaries via the meridian SQLite handoff.
    global::set_text_map_propagator(TraceContextPropagator::new());

    if otel_enabled {
        tracing::info!(
            service.name = service_name,
            log_dir = %log_dir.display(),
            otel = "enabled",
            "observability initialised"
        );
    } else {
        tracing::info!(
            service.name = service_name,
            log_dir = %log_dir.display(),
            otel = "disabled",
            "observability initialised (no OTLP exporter)"
        );
    }

    Ok(ObservabilityGuard {
        _file_guard: file_guard,
        logger_provider,
        otel_enabled,
    })
}

/// Resolved OTLP export target: trace endpoint + Basic-auth credential.
/// `None` means export is disabled (toggle off, or no credentials anywhere).
pub struct OtlpTarget {
    pub endpoint: String,
    pub auth: String,
}

/// Cheap liveness check used by the health probe — does NOT assemble
/// credentials. Returns `true` when OTLP export would be attempted if
/// `resolve_otlp_target()` were called (toggle on + credentials present).
pub fn is_otlp_configured() -> bool {
    let settings = crate::config::load_runtime_settings();
    if !settings.otlp_enabled {
        return false;
    }
    // settings.json is the single source for OO credentials — the old
    // MERIDIAN_OO_AUTH env fallback is deprecated and ignored.
    settings.oo_email.as_deref().is_some_and(|e| !e.is_empty())
        && settings
            .oo_password
            .as_deref()
            .is_some_and(|p| !p.is_empty())
}

/// Resolve the configured OTLP endpoint URL (without assembling credentials).
/// Used by the health check to derive the `/healthz` URL to ping.
pub fn resolve_otlp_endpoint() -> Option<String> {
    let settings = crate::config::load_runtime_settings();
    if !settings.otlp_enabled {
        return None;
    }
    Some(
        settings
            .otlp_endpoint
            .filter(|s| !s.is_empty())
            .or_else(|| {
                std::env::var("MERIDIAN_OTLP_ENDPOINT")
                    .ok()
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| DEFAULT_OTLP_ENDPOINT.to_string()),
    )
}

/// Resolve the full OTLP export target: endpoint + Basic-auth header value.
/// Called only at daemon startup (inside `try_build_otel_providers`). Use
/// `is_otlp_configured()` + `resolve_otlp_endpoint()` for lighter call sites.
pub fn resolve_otlp_target() -> Option<OtlpTarget> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    let settings = crate::config::load_runtime_settings();

    if !settings.otlp_enabled {
        return None;
    }

    // Auth: settings email+password only. The MERIDIAN_OO_AUTH env fallback is
    // DEPRECATED and ignored — a dual credential store (env + settings) meant
    // the UI could show creds while the daemon used different ones (or none).
    // Credentials are set in the dashboard Settings and read from settings.json.
    let auth = match (&settings.oo_email, &settings.oo_password) {
        (Some(email), Some(pass)) if !email.is_empty() && !pass.is_empty() => {
            // Guard against HTTP header injection and malformed user:password splits.
            if email.contains(['\n', '\r']) || pass.contains(['\n', '\r']) || email.contains(':') {
                tracing::warn!(
                    "OTLP credentials contain invalid characters — OTLP export disabled"
                );
                return None;
            }
            STANDARD.encode(format!("{email}:{pass}"))
        }
        _ => {
            if std::env::var("MERIDIAN_OO_AUTH").is_ok_and(|v| !v.is_empty()) {
                tracing::warn!(
                    "MERIDIAN_OO_AUTH is set but deprecated and ignored — \
                     set OpenObserve credentials in the dashboard Settings instead"
                );
            }
            return None;
        }
    };

    let endpoint = settings
        .otlp_endpoint
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::env::var("MERIDIAN_OTLP_ENDPOINT")
                .ok()
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| DEFAULT_OTLP_ENDPOINT.to_string());

    // Validate scheme — only http/https are valid OTLP transports.
    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
        tracing::warn!(
            endpoint = %endpoint,
            "OTLP endpoint has no http/https scheme — OTLP export disabled"
        );
        return None;
    }

    Some(OtlpTarget { endpoint, auth })
}

/// Builds the OTel tracer and logger providers.
///
/// When `otlp_enabled` is true in settings.json the exporters are wired to the
/// [`SpoolClient`], which writes every OTLP batch atomically to
/// `~/.meridian/telemetry/pending/` and returns a synthetic HTTP 200.  The
/// background shipper task then forwards the files to OpenObserve whenever a
/// target is reachable.  This keeps capture and delivery fully decoupled: no
/// telemetry is lost during OO downtime.
///
/// We build the spool providers whenever `otlp_enabled` is true — even when
/// credentials are absent — so durability works before OO is fully configured.
/// The shipper checks `resolve_otlp_target()` separately and skips delivery
/// until credentials are present.
fn try_build_otel_providers(service_name: &str) -> Result<Option<(Tracer, LoggerProvider)>> {
    // Gate on otlp_enabled only — credentials are NOT required for capture.
    let settings = crate::config::load_runtime_settings();
    if !settings.otlp_enabled {
        return Ok(None);
    }

    // Derive placeholder endpoints — SpoolClient ignores them (it writes to
    // disk), but the SDK requires non-empty strings.
    let trace_endpoint = settings
        .otlp_endpoint
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_OTLP_ENDPOINT)
        .to_string();
    let log_endpoint = trace_endpoint.replace("/v1/traces", "/v1/logs");

    let resource = Resource::new(vec![KeyValue::new(
        "service.name",
        service_name.to_string(),
    )]);

    // Build spool clients — one per signal so filenames encode the correct prefix.
    let spool_trace = crate::telemetry_spool::spool_client::SpoolClient::new()
        .context("build spool client for traces")?;
    let spool_logs = crate::telemetry_spool::spool_client::SpoolClient::new()
        .context("build spool client for logs")?;

    // ── Trace pipeline ────────────────────────────────────────────────────
    let span_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_http_client(spool_trace)
        .with_endpoint(&trace_endpoint)
        .with_protocol(Protocol::HttpBinary)
        .build()
        .context("build OTLP span exporter (spool)")?;

    let tracer_provider = TracerProvider::builder()
        .with_batch_exporter(span_exporter, runtime::Tokio)
        .with_sampler(Sampler::AlwaysOn)
        .with_id_generator(RandomIdGenerator::default())
        .with_resource(resource.clone())
        .build();

    let tracer = tracer_provider.tracer(service_name.to_string());
    global::set_tracer_provider(tracer_provider);

    // ── Log pipeline ──────────────────────────────────────────────────────
    let log_exporter = opentelemetry_otlp::LogExporter::builder()
        .with_http()
        .with_http_client(spool_logs)
        .with_endpoint(&log_endpoint)
        .with_protocol(Protocol::HttpBinary)
        .build()
        .context("build OTLP log exporter (spool)")?;

    let logger_provider = LoggerProvider::builder()
        .with_batch_exporter(log_exporter, runtime::Tokio)
        .with_resource(resource)
        .build();

    Ok(Some((tracer, logger_provider)))
}

/// Map the settings.json `log_level` value (DEBUG/INFO/WARNING/ERROR) to a
/// tracing `EnvFilter` string. Used at startup and on hot-reload, when
/// `RUST_LOG` is not set.
fn build_default_filter(log_level: &str) -> String {
    match log_level.to_uppercase().as_str() {
        "DEBUG" => "meridian=debug,sqlx=warn".to_string(),
        "WARNING" | "WARN" => "meridian=warn,sqlx=warn".to_string(),
        "ERROR" => "meridian=error,sqlx=error".to_string(),
        // INFO or anything else: keep the previous fixed default with module-level overrides.
        _ => "meridian=info,meridian::etl=debug,meridian::intelligence=debug,sqlx=warn".to_string(),
    }
}

/// Hot-reload the log level filter without restarting the daemon.
///
/// Called from the poll loop whenever `settings.log_level` changes. Returns
/// `true` if the filter was updated, `false` if RUST_LOG is set (we don't
/// fight explicit env-var overrides) or the handle isn't initialised yet.
pub fn reload_log_level(level: &str) -> bool {
    // Respect explicit RUST_LOG override — don't fight the user's env var.
    if std::env::var("RUST_LOG").is_ok() {
        return false;
    }
    let Some(handle) = FILTER_HANDLE.get() else {
        return false;
    };
    let filter_str = build_default_filter(level);
    match filter_str.parse::<EnvFilter>() {
        Ok(new_filter) => handle.modify(|f| *f = new_filter).is_ok(),
        Err(_) => false,
    }
}

fn resolve_log_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("MERIDIAN_LOG_DIR") {
        return Ok(PathBuf::from(shellexpand::tilde(&dir).into_owned()));
    }
    let home = std::env::var("HOME").context("HOME not set")?;
    Ok(PathBuf::from(home).join(".meridian").join("logs"))
}

/// Inject the current span's W3C `traceparent` into a string suitable for
/// persisting in SQLite. Returns `None` when there is no active span context.
pub fn current_traceparent() -> Option<String> {
    use opentelemetry::propagation::{Injector, TextMapPropagator};
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    struct StringInjector(HashMap<String, String>);
    impl Injector for StringInjector {
        fn set(&mut self, key: &str, value: String) {
            self.0.insert(key.to_string(), value);
        }
    }

    let cx = tracing::Span::current().context();
    let mut carrier = StringInjector(HashMap::new());
    TraceContextPropagator::new().inject_context(&cx, &mut carrier);
    carrier.0.remove("traceparent")
}

/// Parse a stored W3C `traceparent` string back into an OTel [`SpanContext`],
/// suitable for adding as a span Link (e.g. linking a worklog_draft span to the
/// classification / formation traces of its contributing sessions). Returns
/// `None` when the string is empty or not a valid traceparent.
pub fn span_context_from_traceparent(
    traceparent: &str,
) -> Option<opentelemetry::trace::SpanContext> {
    use opentelemetry::propagation::{Extractor, TextMapPropagator};
    use opentelemetry::trace::TraceContextExt;

    if traceparent.is_empty() {
        return None;
    }

    struct StringExtractor<'a>(&'a str);
    impl Extractor for StringExtractor<'_> {
        fn get(&self, key: &str) -> Option<&str> {
            (key == "traceparent").then_some(self.0)
        }
        fn keys(&self) -> Vec<&str> {
            vec!["traceparent"]
        }
    }

    let cx = TraceContextPropagator::new().extract(&StringExtractor(traceparent));
    let sc = cx.span().span_context().clone();
    sc.is_valid().then_some(sc)
}
