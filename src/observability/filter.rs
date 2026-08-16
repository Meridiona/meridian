//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Which targets get captured at all — the `EnvFilter` the subscriber is built
//! with, and the hot-reload handle that swaps it at runtime.
//!
//! Split out of `observability/mod.rs` (over the repo's 500-line cap once the
//! capture directives landed), the same way `install_mode.rs` and
//! `otlp_target.rs` were: "what may be captured" is a self-contained question,
//! independent of the subscriber/exporter wiring the parent module owns.
//!
//! # Why this is load-bearing
//! This filter runs BEFORE the spool, before the severity filter, and before
//! redaction. Anything it drops does not exist anywhere: not shipped, not in
//! `meridian logs`, not in an export bundle. That is not a theoretical concern
//! — see [`CAPTURE_TARGETS`] for the ~48 OCR/accessibility error sites it was
//! silently discarding.
//!
//! # Who calls this
//! - `observability::init` — builds the initial filter from `RUST_LOG`, else
//!   [`build_default_filter`] over `settings.log_level`.
//! - The daemon poll loop — [`reload_log_level`] when the setting changes.

use std::sync::OnceLock;
use tracing_subscriber::{reload, EnvFilter};

/// Type alias for the hot-reload handle. The `S = Registry` parameter reflects
/// that the reload layer is installed directly on `tracing_subscriber::Registry`
/// (it is the first layer added, before any OTel or fmt layers).
pub(super) type FilterHandle = reload::Handle<EnvFilter, tracing_subscriber::Registry>;

/// Global handle for hot-reloading the `EnvFilter` without restarting the daemon.
/// Set once during `init()`; accessed from the poll loop via [`reload_log_level`].
pub(super) static FILTER_HANDLE: OnceLock<FilterHandle> = OnceLock::new();

/// Third-party crate targets that must be captured explicitly, because nothing
/// else in the filter reaches them.
///
/// # Why these need naming at all
/// `EnvFilter` matches a directive against a target by string prefix, so the
/// `meridian` directive happens to cover `meridian_core::*`,
/// `meridian_tray_lib::*` and `meridian_oauth::*` too — every first-party
/// target starts with those bytes. The in-process capture crates do NOT
/// (`screenpipe_screen::*`, `screenpipe_a11y::*`), so every one of their ~48
/// `warn!`/`error!` sites was discarded at the subscriber. OCR and
/// accessibility failures — screen-capture bail-outs, Apple Vision handler
/// creation, `AXObserver`/`CGEventTap` registration, monitor enumeration — were
/// structurally invisible. Since capture is the daemon's only data source, a
/// silent failure there looks downstream like "the user did nothing today".
///
/// # Privacy
/// Audited every `warn!`/`error!` site in both crates before enabling: all are
/// infrastructure failures (monitor ids, image dimensions, word counts, error
/// `Display`s). None interpolates a window title, app name, URL, or OCR text.
/// **Re-audit if the fork's revision is bumped** — this is exactly the class of
/// change that could start shipping captured content.
///
/// `tauri_plugin_clerk` was added alongside its `vendor/tauri-plugin-clerk`
/// session-persistence fix (`tray/src-tauri/vendor/tauri-plugin-clerk`) so the
/// new `tracing::warn!` sites that replaced a swallowed deserialize error are
/// actually visible in `meridian logs`/OpenObserve instead of only the local
/// terminal. Audited every event site in that crate, not only the ones this
/// change added — the `warn!`/`error!` sites interpolate only
/// `serde_json::Error`'s and the Tauri emit error's `Display` (schema-mismatch
/// messages — field names, not field values), and the two `DEBUG` sites that
/// upstream wrote as `{payload:?}` were rewritten to log presence booleans.
/// That rewrite is load-bearing, not tidying: `ClientSession` carries
/// `last_active_token.jwt`, so a `Debug`-rendered payload is a live bearer
/// token, and naming the crate here is exactly what would route it into the
/// full-fidelity local spool. Nothing in this crate may interpolate the Clerk
/// payload (email, JWT, name). **Re-audit alongside the vendored crate** — on
/// any upstream re-sync, and if it's ever un-vendored back to a crates.io
/// release (see that crate's module doc for the un-vendor plan).
///
/// # Adding to this list
/// Add the bare target (crate) name here and every log level picks it up
/// automatically. It is deliberately a list of names rather than a formatted
/// directive string: the level varies per branch in [`build_default_filter`],
/// and hardcoding the pairs in each branch is how a new entry silently ends up
/// with no coverage at one level.
const CAPTURE_TARGETS: &[&str] = &["screenpipe_screen", "screenpipe_a11y", "tauri_plugin_clerk"];

/// Render [`CAPTURE_TARGETS`] as `EnvFilter` directives at `level`.
fn capture_directives(level: &str) -> String {
    CAPTURE_TARGETS
        .iter()
        .map(|t| format!("{t}={level}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Map the settings.json `log_level` value (DEBUG/INFO/WARNING/ERROR) to a
/// tracing `EnvFilter` string. Used at startup and on hot-reload, when
/// `RUST_LOG` is not set.
///
/// The capture stack is pinned at `warn` regardless of the first-party level:
/// these are third-party crates written for a CLI that printed to a terminal,
/// so their INFO/DEBUG volume is unaudited and runs at frame cadence — `warn`
/// takes the failures without the chatter. The one exception is `ERROR`, where
/// the user has asked for errors only and that is honoured everywhere.
///
/// Note `RUST_LOG` REPLACES this wholesale rather than extending it, so an
/// engineer who sets it also opts out of [`CAPTURE_TARGETS`] and must re-add
/// them by hand to see capture failures.
pub(super) fn build_default_filter(log_level: &str) -> String {
    let (base, capture_level) = match log_level.to_uppercase().as_str() {
        "DEBUG" => ("meridian=debug,sqlx=warn", "warn"),
        "WARNING" | "WARN" => ("meridian=warn,sqlx=warn", "warn"),
        "ERROR" => ("meridian=error,sqlx=error", "error"),
        // INFO or anything else: keep the previous fixed default with module-level overrides.
        // `embedder=debug` surfaces the model-load/batch spans (embedder/mod.rs) at the
        // production default — the same treatment etl/intelligence already get — since
        // the embedder is on the critical path for every hour's distillation and its
        // timing is exactly what a `DISTILLER_EMBED_TIMEOUT_SECS` investigation needs.
        _ => (
            "meridian=info,meridian::etl=debug,meridian::intelligence=debug,meridian::embedder=debug,sqlx=warn",
            "warn",
        ),
    };
    format!("{base},{}", capture_directives(capture_level))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::{layer::SubscriberExt, Layer};

    /// Records the target of every event the filter lets through.
    struct Recorder(Arc<Mutex<Vec<String>>>);

    impl<S: tracing::Subscriber> Layer<S> for Recorder {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            self.0
                .lock()
                .unwrap()
                .push(event.metadata().target().to_string());
        }
    }

    /// Run `f` under `filter` and return the targets that survived.
    fn targets_passing(filter: &str, f: impl FnOnce()) -> Vec<String> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new(filter))
            .with(Recorder(Arc::clone(&seen)));
        tracing::subscriber::with_default(subscriber, f);
        let out = seen.lock().unwrap().clone();
        out
    }

    fn emit_all() {
        tracing::warn!(target: "screenpipe_screen::core", "ocr failed");
        tracing::warn!(target: "screenpipe_a11y::observer", "axobserver failed");
        tracing::info!(target: "meridian::etl::runner", "etl ok");
        tracing::info!(target: "meridian_core::readers::tasks", "read ok");
        tracing::info!(target: "meridian_tray_lib::commands::health", "health ok");
        tracing::warn!(target: "reqwest::connect", "noisy third party");
    }

    /// The bug this guards: `EnvFilter` matches directives against targets by
    /// string PREFIX, so `meridian` silently covers `meridian_core` and
    /// `meridian_tray_lib` — but nothing covered `screenpipe_*`, so every OCR
    /// and accessibility failure was dropped at the subscriber. Asserts the
    /// negative on the old filter and the positive on the new one, since the
    /// whole finding rests on that asymmetry.
    #[test]
    fn capture_targets_pass_only_with_the_capture_directives() {
        let old = "meridian=info,sqlx=warn";
        let passed = targets_passing(old, emit_all);
        assert!(
            !passed.iter().any(|t| t.starts_with("screenpipe")),
            "regression check is meaningless if the old filter already passed capture: {passed:?}"
        );
        // ...while first-party targets DID pass on prefix alone.
        assert!(passed.iter().any(|t| t.starts_with("meridian_core")));
        assert!(passed.iter().any(|t| t.starts_with("meridian_tray_lib")));

        let passed = targets_passing(&build_default_filter("INFO"), emit_all);
        assert!(
            passed.iter().any(|t| t == "screenpipe_screen::core"),
            "OCR failures still dropped at the subscriber: {passed:?}"
        );
        assert!(
            passed.iter().any(|t| t == "screenpipe_a11y::observer"),
            "accessibility failures still dropped at the subscriber: {passed:?}"
        );
        // Unrelated third-party crates must stay out — this widens the filter
        // deliberately and narrowly, not globally.
        assert!(
            !passed.iter().any(|t| t.starts_with("reqwest")),
            "filter widened beyond the capture stack: {passed:?}"
        );
    }

    /// A throttled site's "quieter repeat" must be INFO, not DEBUG.
    ///
    /// This pins a coupling nothing else expresses, and getting it wrong is
    /// invisible at the call site. `pm_worklog::post` throttles a permanent
    /// warning and emits the suppressed repeats at a lower level so the
    /// condition stays readable in `meridian logs` without egressing (only
    /// WARN+ ships). That was first written as `debug!` — and at the production
    /// default this filter grants debug ONLY to `etl`, `intelligence` and
    /// `embedder`, so `meridian::pm_worklog::post` fell under `meridian=info`
    /// and the repeats were dropped **before the spool**. Not quieter: gone.
    ///
    /// So: INFO from that target must survive the default filter, and DEBUG
    /// must not. If someone adds a debug directive covering `pm_worklog` this
    /// fails and they can drop the `debug!`-is-invisible workaround; if someone
    /// removes `meridian=info` it fails too.
    #[test]
    fn a_throttled_repeat_at_info_survives_the_default_filter_but_debug_does_not() {
        const TARGET: &str = "meridian::pm_worklog::post";
        let filter = build_default_filter("INFO");

        let passed = targets_passing(&filter, || {
            tracing::info!(target: "meridian::pm_worklog::post", "throttled repeat");
        });
        assert!(
            passed.iter().any(|t| t == TARGET),
            "INFO from {TARGET} was dropped by the default filter, so a throttled \
             repeat would be invisible even locally: {passed:?}"
        );

        let passed = targets_passing(&filter, || {
            tracing::debug!(target: "meridian::pm_worklog::post", "throttled repeat");
        });
        assert!(
            !passed.iter().any(|t| t == TARGET),
            "DEBUG from {TARGET} now passes the default filter. That is fine, but \
             `post.rs` emits throttled repeats at INFO specifically BECAUSE debug \
             was being dropped here — revisit that comment: {passed:?}"
        );
    }

    /// EVERY entry in `CAPTURE_TARGETS` must be covered at EVERY log level.
    ///
    /// Deliberately iterates the constant rather than naming crates: an earlier
    /// revision hardcoded the pairs in the `ERROR` branch, so a target added to
    /// the list would have had no coverage at that one level while working
    /// everywhere else — and a test written against literal names stays green
    /// through exactly that drift.
    #[test]
    fn every_capture_target_is_covered_at_every_log_level() {
        for level in ["DEBUG", "INFO", "WARNING", "ERROR", "nonsense"] {
            let f = build_default_filter(level);
            for target in CAPTURE_TARGETS {
                assert!(
                    f.contains(&format!("{target}=")),
                    "{level} filter has no directive for {target}: {f}"
                );
            }
            // A directive can be present and still not admit an ERROR (e.g. set
            // to `off`), so also assert on behaviour, not just on the string.
            //
            // These two targets are spelled out because `tracing::error!`
            // requires a literal target — the loop above is the part that
            // actually scales with `CAPTURE_TARGETS`, and is what catches a
            // newly added entry losing coverage at one level.
            let passed = targets_passing(&f, || {
                tracing::error!(target: "screenpipe_screen::core", "hard failure");
                tracing::error!(target: "screenpipe_a11y::observer", "hard failure");
            });
            assert_eq!(passed.len(), 2, "{level} dropped a capture ERROR: {f}");
        }
    }

    /// At ERROR the user asked for errors only, and that must apply to the
    /// capture stack too rather than forcing its warnings through.
    #[test]
    fn error_level_suppresses_capture_warnings() {
        let passed = targets_passing(&build_default_filter("ERROR"), || {
            tracing::warn!(target: "screenpipe_screen::core", "noisy per-frame warn");
        });
        assert!(
            passed.is_empty(),
            "ERROR level let a WARN through: {passed:?}"
        );
    }
}
