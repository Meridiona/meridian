//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//
// Observability bootstrap.
//
// One call to `init(service_name)` builds a `tracing` subscriber whose
// canonical, persisted sink is the local OpenTelemetry telemetry spool
// (`~/.meridian/telemetry/pending/`, both traces and logs — log events carry
// trace_id/span_id so they correlate with traces). There is no separate
// JSON-Lines log file — every `tracing::*!` call is captured there, so there
// is exactly one STORED log/trace representation to export/import, not
// several. `meridian logs` (see `telemetry_spool::render`) decodes this same
// spool back to human-readable text on demand rather than reading a parallel
// plain-text file.
//
// In a debug build (`cargo run`/`cargo watch` — i.e. every normal dev
// workflow, `dev-start.sh` included) a compact stdout/stderr mirror is ALSO
// installed, purely for live terminal visibility — it writes nothing to
// disk, so it isn't a second persisted log store, just an ephemeral view of
// the same events (the same pattern as `kubectl logs -f` alongside a
// structured backend). Gated on `cfg!(debug_assertions)`, NOT on install type
// (`is_canonical_install()`/`Dev`/`Bare`/`Canonical`): those answer a
// different question (may this process ship to OpenObserve?) and using it
// here would misfire on a dev machine that also has the packaged app
// installed (`~/.meridian/.env` would exist, silently killing terminal
// output during normal dev work). A `--release` build — what actually ships
// in the DMG — never gets this mirror, matching production exactly.
//
// The one thing NOT covered by this pipeline is a hard crash (panic before
// `init` runs, segfault, OOM kill) — for that, launchd's own stdout/stderr
// redirect (`~/.meridian/logs/<service>.log` / `<service>-error.log`, set in
// each service's `com.meridiona.*.plist`) is the OS-level safety net; it's
// unrelated to `tracing` and stays in place, size-capped by
// `telemetry_spool::shipper` and folded into diagnostics export bundles.
//
// Capture (writing spans/logs to `~/.meridian/telemetry/pending/`) is
// unconditional — it's a local disk write, not a network call — so every
// install always has full structured traces available for export, regardless
// of shipping configuration. Only *delivery* (the background shipper POSTing
// spooled files to OpenObserve) is gated, and only for a Dev/Bare install with
// `otlp_enabled` + credentials; a Canonical (packaged/shipped) install never
// attempts network delivery. See `install_mode::is_canonical_install()` and
// `telemetry_spool::shipper`.
//
// Environment variables read at init time:
//   MERIDIAN_OTLP_ENDPOINT     — OTLP/HTTP traces endpoint override
//                                 (default: http://localhost:5080/api/default/v1/traces)
//   MERIDIAN_LOG_DIR           — launchd raw-log directory (default: ~/.meridian/logs)
//   MERIDIAN_TELEMETRY_DISABLED — hard kill switch: skip OTel capture entirely
//                                 (no tracing sink at all beyond the default panic hook)
//   RUST_LOG                   — standard env-filter; default
//                                 "meridian=info,meridian::etl=debug,sqlx=warn"
//
// OpenObserve credentials (for shipping, dev/bare installs only) come from
// settings.json (oo_email/oo_password, set in the dashboard Settings). The
// MERIDIAN_OO_AUTH env var is DEPRECATED and ignored; shipping is skipped when
// settings carry no credentials or the install is Canonical.
//
// `init` returns an `ObservabilityGuard`. Call
// `ObservabilityGuard::shutdown().await` before tearing down the tokio
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
use tracing_subscriber::{layer::SubscriberExt, reload, util::SubscriberInitExt, EnvFilter};

mod filter;
mod install_mode;
mod otlp_target;
/// Tests only - it reads the workspace's own source rather than calling
/// anything, so there is nothing here to compile into a release build.
#[cfg(test)]
mod span_status_guard;
pub use filter::reload_log_level;
use filter::{build_default_filter, FILTER_HANDLE};
use install_mode::capture_disabled;
pub use install_mode::{is_canonical_install, staged_daemon_path};
use otlp_target::DEFAULT_OTLP_ENDPOINT;
pub use otlp_target::{
    is_otlp_configured, resolve_otlp_endpoint, resolve_otlp_target, AuthCredential, OtlpTarget,
};

/// RAII guard returned from [`init`]. Holds (when OTel is enabled) the logger
/// provider for graceful shutdown.
///
/// Call [`ObservabilityGuard::shutdown`] explicitly before the tokio runtime
/// is torn down — the BatchSpanProcessor's shutdown is blocking, and a Drop
/// inside an async context panics with "Cannot drop a runtime in a context
/// where blocking is not allowed".
pub struct ObservabilityGuard {
    tracer_provider: Option<TracerProvider>,
    logger_provider: Option<LoggerProvider>,
    otel_enabled: bool,
}

impl ObservabilityGuard {
    /// Flush and shut down both OTel exporters (traces + logs). Must be
    /// `await`ed while the tokio runtime is still alive.
    ///
    /// We hold the concrete `TracerProvider`/`LoggerProvider` and `force_flush`
    /// each one BEFORE shutting it down, rather than relying on
    /// `global::shutdown_tracer_provider()`. On a long-lived daemon it makes no
    /// difference (the batch processor's timer already drains spans every few
    /// seconds), but on a **short-lived one-shot** (`meridian worklog-hour`,
    /// `coding-agent-*`) the final batch — the parent `worklog.hour`/`.report`
    /// spans that only close as the process ends — was being lost: the global
    /// shutdown returned before that last batch reached the spool. An explicit
    /// `force_flush` pushes it to `~/.meridian/telemetry/pending/` first, so a
    /// manual run produces the same complete trace the daemon does.
    pub async fn shutdown(self) {
        if !self.otel_enabled {
            return;
        }
        if let Some(tp) = self.tracer_provider {
            let _ = tokio::task::spawn_blocking(move || {
                for r in tp.force_flush() {
                    if let Err(e) = r {
                        eprintln!("observability: span force_flush error: {e:?}");
                    }
                }
                let _ = tp.shutdown();
            })
            .await;
        }
        if let Some(lp) = self.logger_provider {
            let _ = tokio::task::spawn_blocking(move || {
                // See `force_flush` for why this reports to stderr rather than
                // through `tracing`.
                for r in lp.force_flush() {
                    if let Err(e) = r {
                        eprintln!("observability: log force_flush error: {e:?}");
                    }
                }
                let _ = lp.shutdown();
            })
            .await;
        }
    }
}

/// Provider handles kept for [`force_flush`]. Set once by [`init`].
///
/// Separate from [`ObservabilityGuard`] because the guard is owned by whoever
/// called `init` — in the tray that is a local in `run()` — while the code that
/// needs to flush is somewhere else entirely (an exit handler in a spawned
/// task). Both providers are `Arc`-backed, so this holds clones rather than
/// taking anything away from the guard.
static FLUSH_HANDLES: std::sync::OnceLock<FlushHandles> = std::sync::OnceLock::new();

struct FlushHandles {
    tracer_provider: Option<TracerProvider>,
    logger_provider: Option<LoggerProvider>,
}

/// Push everything currently batched in memory out to the telemetry spool,
/// WITHOUT shutting the providers down.
///
/// # Why this is needed at all
///
/// Spans and logs do not reach `~/.meridian/telemetry/pending/` the moment they
/// are emitted: the batch processors hold them and drain on a timer. A process
/// that calls `std::process::exit` — which is what `tauri::AppHandle::exit`
/// does — takes that batch with it. Destructors do not run, so holding an RAII
/// guard is no protection either.
///
/// The practical cost was that the LAST thing a process does is the thing least
/// likely to be recorded, and for the tray the last thing it does is stop the
/// daemon on quit. Every `daemon stopped for quit` / `could not stop the daemon
/// on quit` / `exceeded its budget` line was emitted and then discarded
/// microseconds later, so the outcome of the operation that most needs
/// explaining after a corruption report was systematically the one missing from
/// the spool.
///
/// [`ObservabilityGuard::shutdown`] already does this and more, but it consumes
/// the guard and shuts the providers down; this is the "flush and keep going"
/// half, callable from anywhere and safe to call more than once.
///
/// A no-op when capture is disabled (`MERIDIAN_TELEMETRY_DISABLED`) or before
/// [`init`] has run, so callers never need to check.
///
/// # Who calls this
/// - `meridian_tray_lib::run`'s `RunEvent::ExitRequested` handler, immediately
///   before `handle.exit(0)`.
pub async fn force_flush() {
    let Some(handles) = FLUSH_HANDLES.get() else {
        return;
    };
    if let Some(tp) = handles.tracer_provider.clone() {
        let _ = tokio::task::spawn_blocking(move || {
            for r in tp.force_flush() {
                if let Err(e) = r {
                    eprintln!("observability: span force_flush error: {e:?}");
                }
            }
        })
        .await;
    }
    if let Some(lp) = handles.logger_provider.clone() {
        let _ = tokio::task::spawn_blocking(move || {
            // Reported, not discarded - the tracer arm above already prints its
            // error and the logger arm silently swallowed one.
            //
            // `eprintln!` rather than `tracing::error!` DELIBERATELY: this is
            // the code that flushes the telemetry pipeline, so routing its own
            // failure back into that pipeline is circular. On the quit path the
            // process exits microseconds later, so a `tracing::error!` here
            // would land in the very batch that just failed to flush and be
            // lost - reporting the failure by the one mechanism the failure
            // proves is broken. stderr is the only sink that does not depend on
            // what is failing.
            for r in lp.force_flush() {
                if let Err(e) = r {
                    eprintln!("observability: log force_flush error: {e:?}");
                }
            }
        })
        .await;
    }
}

/// Initialise the layered tracing subscriber.
///
/// `service_name` becomes the OTel `service.name` resource attribute. The
/// OTel spool (traces + logs) is the sole PERSISTED sink; a debug build also
/// gets a compact stdout/stderr mirror for live terminal visibility (see the
/// module doc comment above for why this is gated on `cfg!(debug_assertions)`
/// rather than install type).
pub fn init(service_name: &str) -> Result<ObservabilityGuard> {
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

    // Debug-build-only terminal mirror. `Option<Layer>` itself implements
    // `Layer` (tracing-subscriber's blanket impl), so `.with(fmt_stdout)`
    // below is a no-op layer in a release build — no runtime cost, no output.
    //
    // ONE layer, with the writer choosing the stream by level — NOT two layers.
    // It used to be a stdout layer taking every level plus a second stderr
    // layer filtered to WARN, which meant every warning and error was printed
    // TWICE in a dev run (once per layer) while INFO printed once. Two layers
    // are two subscribers: each one formats and writes the event independently,
    // so a level filter on the second narrows WHICH events it duplicates, never
    // whether it duplicates them.
    //
    // `with_max_level(WARN)` reads by SEVERITY, not verbosity: it admits WARN
    // and ERROR (the levels at or above WARN in severity) and rejects
    // INFO/DEBUG/TRACE. `or_else` then catches exactly what stderr declined, so
    // the two streams partition the events rather than overlapping — every
    // event lands on exactly one stream, once.
    use tracing_subscriber::fmt::writer::MakeWriterExt as _;
    let fmt_stdout = cfg!(debug_assertions).then(|| {
        tracing_subscriber::fmt::layer()
            .with_target(true)
            .with_writer(
                std::io::stderr
                    .with_max_level(tracing::Level::WARN)
                    .or_else(std::io::stdout),
            )
            .compact()
    });

    // Build OTel providers first (no generic subscriber type involved yet),
    // then construct the layers inline so the subscriber type is concrete at
    // each .with() call — this avoids the Box<dyn Layer<S>> type-erasure issue
    // that arises when chaining two boxed layers with different subscriber types.
    let (otel_enabled, tracer_provider, logger_provider) =
        match try_build_otel_providers(service_name) {
            Ok(Some((tracer, tp, lp))) => {
                let trace_layer = tracing_opentelemetry::layer()
                    .with_tracer(tracer)
                    .with_tracked_inactivity(false);
                let log_layer = OpenTelemetryTracingBridge::new(&lp);

                tracing_subscriber::registry()
                    .with(rl.take().unwrap())
                    .with(fmt_stdout)
                    .with(trace_layer)
                    .with(log_layer)
                    .init();

                (true, Some(tp), Some(lp))
            }
            Ok(None) => {
                // Capture disabled (MERIDIAN_TELEMETRY_DISABLED) — no persisted
                // sink beyond the filter layer (still gets the debug-build
                // terminal mirror, if any). Rust's default panic hook still
                // prints to stderr regardless of any tracing subscriber, so a
                // hard failure is never silent even here.
                tracing_subscriber::registry()
                    .with(rl.take().unwrap())
                    .with(fmt_stdout)
                    .init();
                (false, None, None)
            }
            Err(err) => {
                eprintln!("observability: OTLP exporter init failed: {err:#}");
                tracing_subscriber::registry()
                    .with(rl.take().unwrap())
                    .with(fmt_stdout)
                    .init();
                (false, None, None)
            }
        };

    // W3C trace-context propagator so we can inject/extract `traceparent` strings
    // across process boundaries via the meridian SQLite handoff.
    global::set_text_map_propagator(TraceContextPropagator::new());

    if otel_enabled {
        tracing::info!(
            service.name = service_name,
            otel = "enabled",
            "observability initialised"
        );
    } else {
        tracing::info!(
            service.name = service_name,
            otel = "disabled",
            "observability initialised (no OTLP exporter)"
        );
    }

    // Clones for `force_flush`, which runs far from whoever owns the guard.
    // `set` rather than `get_or_init`: a second `init` in one process is a bug,
    // and silently keeping the first set of handles is the safer failure.
    let _ = FLUSH_HANDLES.set(FlushHandles {
        tracer_provider: tracer_provider.clone(),
        logger_provider: logger_provider.clone(),
    });

    Ok(ObservabilityGuard {
        tracer_provider,
        logger_provider,
        otel_enabled,
    })
}

/// Builds the OTel tracer and logger providers.
///
/// The exporters are always wired to the [`SpoolClient`], which writes every
/// OTLP batch atomically to `~/.meridian/telemetry/pending/` and returns a
/// synthetic HTTP 200 — this is a local disk write, not a network call, so
/// capture happens unconditionally (every install, dev or packaged). The
/// background shipper task separately decides whether it's allowed to forward
/// spooled files to OpenObserve (Dev/Bare install + `otlp_enabled` +
/// credentials); a Canonical install never ships. This keeps capture and
/// delivery fully decoupled: no telemetry is lost during OO downtime, and a
/// packaged install still has full local traces to export by hand.
///
/// Only `MERIDIAN_TELEMETRY_DISABLED` skips capture entirely (an explicit
/// escape hatch, not tied to shipping config).
fn try_build_otel_providers(
    service_name: &str,
) -> Result<Option<(Tracer, TracerProvider, LoggerProvider)>> {
    if capture_disabled() {
        return Ok(None);
    }
    let settings = crate::config::load_runtime_settings();

    // Derive placeholder endpoints — SpoolClient ignores them (it writes to
    // disk), but the SDK requires non-empty strings.
    let trace_endpoint = settings
        .otlp_endpoint
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_OTLP_ENDPOINT)
        .to_string();
    let log_endpoint = trace_endpoint.replace("/v1/traces", "/v1/logs");

    // `host.name` is a "semconv_experimental"-gated constant in
    // opentelemetry-semantic-conventions 0.27 — not worth enabling that
    // feature flag for one stable, well-known attribute name.
    use opentelemetry_semantic_conventions::resource::SERVICE_VERSION;
    let mut resource_kvs = vec![
        KeyValue::new("service.name", service_name.to_string()),
        KeyValue::new(SERVICE_VERSION, env!("CARGO_PKG_VERSION")),
        // Kept RAW here on purpose. This resource set feeds the local spool,
        // which `meridian logs` renders at full fidelity — a developer reading
        // their own machine's logs should see their own hostname.
        //
        // The ship leg does not hash this value, it REPLACES it with
        // `telemetry_spool::redact::local_host_pseudonym()` — a hash of a
        // stable hardware id, not of whatever the hostname happened to be at
        // capture time. On macOS the kernel hostname is network-derived
        // (see `telemetry_spool::machine_id`), so hashing it would give the
        // same machine a new identity on every network change. Nothing
        // identifying reaches the central backend either way.
        KeyValue::new(
            "host.name",
            gethostname::gethostname().to_string_lossy().into_owned(),
        ),
        // Which release channel produced this build, so staging test traffic
        // and real user errors are separable in the central backend — they
        // share one endpoint and one Sentry project, and without this
        // attribute there is no way to filter one out of the other. Baked at
        // compile time from the release workflow's job-level `MERIDIAN_CHANNEL`
        // (set for both the macOS and Windows build jobs); a source build has
        // it unset and reports "dev".
        KeyValue::new(
            "deployment.environment",
            option_env!("MERIDIAN_CHANNEL").unwrap_or("dev"),
        ),
        // Platform shape. Without these, a Windows error and a macOS error are
        // indistinguishable in the central backend — every attribute above is
        // OS-agnostic, and the SDK adds nothing of its own (`Resource::new`
        // runs no detectors, unlike `Resource::default`). All three are already
        // on `redact::SAFE_STRING_KEYS`, so populating them needs no change to
        // the redaction boundary: they are enum-like build facts that
        // structurally cannot carry user content.
        //
        // Deliberately the RAW Rust constants ("macos"/"windows", "aarch64"/
        // "x86_64") rather than the OTel semconv spellings ("darwin", "arm64").
        // Nothing downstream parses these as semconv enums, and a translation
        // layer is one more place to introduce a silent mismatch between what
        // the code says and what the dashboards filter on.
        KeyValue::new("os.type", std::env::consts::OS),
        KeyValue::new("host.arch", std::env::consts::ARCH),
        // NOT setting `app.install_mode` here, deliberately — it is on
        // `redact::SAFE_STRING_KEYS` and looks tempting. The only available
        // signal is `is_canonical_install()`, which compares `current_exe()`
        // against `~/.meridian/bin/meridian`. That is a DAEMON-shaped test, and
        // this function also runs in the tray (`lib.rs` calls
        // `observability::init("meridian-tray")` unconditionally), whose exe
        // lives in the `.app` bundle — so every tray row on a fully packaged
        // install would be labelled "source". A wrong platform attribute is
        // worse than an absent one; it gets trusted in a dashboard filter.
        // `deployment.environment` already separates source builds ("dev") from
        // released ones, which is the distinction that was actually wanted.
    ];

    // ALPHA TESTING ONLY (expires with
    // `redact::ALPHA_ACCOUNT_OVERRIDE_EXPIRES_UNIX`) — attach the RAW
    // signed-in email so support can identify which user and which machine an
    // error came from, alongside (not instead of) the per-machine/per-account
    // pseudonym that already seeds `host.name` above. This is the one
    // deliberate exception to this resource set otherwise carrying no PII;
    // see `redact::alpha_account_email_if_active`'s doc for the full
    // rationale and `redact::ACCOUNT_EMAIL_KEY`'s doc for why it's safe on
    // the ship-leg allowlist. Runs in BOTH the daemon and the tray (this
    // function is called from both), so either process shipping an error
    // while the tester is signed in carries the same value.
    if let Some(email) = crate::telemetry_spool::redact::alpha_account_email_if_active(
        settings.account_email.as_deref(),
    ) {
        resource_kvs.push(KeyValue::new(
            crate::telemetry_spool::redact::ACCOUNT_EMAIL_KEY,
            email,
        ));
    }
    let resource = Resource::new(resource_kvs);

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
    // Clone into the global (context propagation) but keep the original so the
    // guard can force_flush + shutdown it explicitly on exit — a clone is a
    // cheap Arc bump and both handles drive the same batch processor.
    global::set_tracer_provider(tracer_provider.clone());

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

    Ok(Some((tracer, tracer_provider, logger_provider)))
}

/// `~/.meridian/logs/` — where launchd redirects each service's raw
/// stdout/stderr (`daemon.log`, `tray.log`, etc. — see each service's
/// `com.meridiona.*.plist`). No longer written to by `tracing`/`logging`
/// directly (the OTel spool is the sole application-log sink now); this
/// directory holds only the crash-safety-net text launchd captures. Public so
/// the telemetry shipper's raw-log size cap (`telemetry_spool::shipper`) and
/// the diagnostics export bundle can find it.
pub fn resolve_log_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var("MERIDIAN_LOG_DIR") {
        return Ok(PathBuf::from(shellexpand::tilde(&dir).into_owned()));
    }
    let meridian_dir =
        meridian_core::paths::meridian_dir().context("cannot resolve the home directory")?;
    Ok(meridian_dir.join("logs"))
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
