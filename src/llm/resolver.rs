//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The single factory — settings in, live backend out — plus the fallback chain.
//!
//! # Re-read on every call, never cached
//!
//! [`resolve`] loads `settings.json` fresh each time it is called. That is the whole
//! trick (Dayflow's `LLMService.shared` does the same): with no cached provider there is
//! nothing to invalidate, so changing the provider in Settings takes effect on the next
//! call with **zero** plumbing — no reload hook, no restart, no event. The read is a ~1 KB
//! file parse; the call it gates takes 5-120 seconds. The cost is unmeasurable.
//!
//! # [`backend_for`] is the ONLY match on `LlmProvider`
//!
//! If a second `match` on the provider appears anywhere in the codebase, the choice has
//! fragmented and we have reinvented Dayflow's bug (it ended up with three independent
//! provider settings and a generator that bypassed its own factory). Route everything
//! through here.
//!
//! # The fallback chain ([`complete`])
//!
//! | what happened | what we do |
//! |---|---|
//! | `Failed` (crashed, not installed, timed out) | retry once, then give up |
//! | `RateLimited` (subscription exhausted) | give up **immediately** — retrying a quota is pointless — and stop routing to that provider until it resets |
//! | provider gives up | return the error; the caller leaves the work pending and retries next tick |
//!
//! There is **no on-device fallback** — generation runs only through the user's chosen
//! third-party provider. When it's unavailable, the LLM-driven step simply idles and the
//! hourly ledger re-runs it next cycle. (A dedicated auto-retry/backoff-aware scheduler is
//! a planned follow-up.)
//!
//! The backoff lives **in memory here, not in settings.json**. The user's *choice* is
//! sacred: being rate-limited pauses the *routing* for that provider, it does not rewrite
//! what they asked for. Writing it to settings would mean a quota blip silently and
//! permanently changed their provider. That distinction is what keeps this from becoming
//! Dayflow's three-settings mess.
//!
//! # How long the backoff actually lasts
//!
//! [`start_backoff`] tries [`super::reset_time::parse_backoff`] against the provider's own
//! error message first — Claude Code and Codex both print their real reset hint ("resets
//! 3pm", "try again in 5 hours"), so a call resumes the moment the provider says it will,
//! not after an arbitrary wait. [`RATE_LIMIT_BACKOFF`] is only the fallback for a message
//! shape we don't recognise (e.g. Codex's weekly-limit date format).

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use meridian_core::settings::load_runtime_settings;

use crate::coding_agent_session_ingest::summariser::SummariserError;

use tracing::Instrument;

use super::{
    claude::ClaudeBackend, codex::CodexBackend, copilot::CopilotBackend, cursor::CursorBackend,
    openai_compat::OpenAiCompatBackend, reset_time, LlmBackend, LlmConfig, LlmError, LlmOutput,
    LlmProvider, PromptRequest,
};

/// Fallback for a rate-limit message [`reset_time::parse_backoff`] couldn't read. Matches
/// the summariser's own backoff: a quota that just refused us will still refuse us half an
/// hour later, and this is a safe default while we wait to actually hear otherwise.
pub const RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(30 * 60);

/// When each rate-limited backend may be used again, keyed by backend IDENTITY (see
/// [`backoff_key`]) — NOT the bare [`LlmProvider`], because two custom endpoints share the
/// `Custom` variant but must back off independently.
///
/// Per-backend keying is what makes "switch providers to escape a rate limit" work with no
/// cross-process signal: the backoff lives in the DAEMON's memory while `settings.json` is
/// written by the TRAY, so a clear-on-write could never reach here. Instead, [`resolve`] /
/// [`complete_inner`] re-read the selected provider every call and only ever consult the key
/// for THAT backend — a stale entry for the provider the user just left is simply never
/// looked at, and expires on its own. There is therefore nothing to clear on a switch.
///
/// Transient, in-memory, deliberately not persisted — see the module docs.
static RATE_LIMITED_UNTIL: Mutex<BTreeMap<String, Instant>> = Mutex::new(BTreeMap::new());

/// Bridge the summariser's error type (raised by the shared `run_capture`) into ours.
pub(super) fn from_summariser_error(e: SummariserError) -> LlmError {
    match e {
        SummariserError::RateLimited(m) => LlmError::RateLimited(m),
        SummariserError::Failed(m) => LlmError::Failed(m),
    }
}

/// The backoff-map key for a resolved backend. CLI providers key by name; a custom endpoint
/// keys by its id so two endpoints (both the `Custom` variant) back off independently.
fn backoff_key(provider: LlmProvider, cfg: &LlmConfig) -> String {
    match provider {
        LlmProvider::Custom => format!(
            "custom:{}",
            cfg.custom.as_ref().map(|c| c.id.as_str()).unwrap_or("")
        ),
        _ => provider.as_str().to_string(),
    }
}

fn is_backing_off(key: &str) -> bool {
    let mut guard = RATE_LIMITED_UNTIL.lock().unwrap_or_else(|e| e.into_inner());
    match guard.get(key).copied() {
        Some(until) if Instant::now() < until => true,
        Some(_) => {
            guard.remove(key); // expired — resume the user's choice for this backend
            false
        }
        None => false,
    }
}

fn start_backoff(key: &str, duration: Duration) {
    let mut guard = RATE_LIMITED_UNTIL.lock().unwrap_or_else(|e| e.into_inner());
    guard.insert(key.to_string(), Instant::now() + duration);
}

/// Clear all backoff state. Test-only: per-backend keying (see [`RATE_LIMITED_UNTIL`]) means
/// a live install never needs an explicit clear — switching providers just consults a
/// different key, and each entry expires on its own.
#[cfg(test)]
pub fn clear_backoff() {
    let mut guard = RATE_LIMITED_UNTIL.lock().unwrap_or_else(|e| e.into_inner());
    guard.clear();
}

/// The one place an [`LlmProvider`] becomes a live backend. Nothing else may match on it.
pub fn backend_for(provider: LlmProvider, cfg: LlmConfig) -> Box<dyn LlmBackend> {
    match provider {
        LlmProvider::Claude => Box::new(ClaudeBackend { cfg }),
        LlmProvider::Codex => Box::new(CodexBackend { cfg }),
        LlmProvider::Cursor => Box::new(CursorBackend { cfg }),
        LlmProvider::Copilot => Box::new(CopilotBackend { cfg }),
        // The endpoint was resolved into `cfg` (see `LlmConfig::from_settings`), because
        // this factory cannot fail. A `cfg.custom` of None — "custom" selected with no live
        // row — surfaces as a clear error on the call, never as a quiet switch to another
        // provider.
        LlmProvider::Custom => Box::new(OpenAiCompatBackend { cfg }),
    }
}

/// The user's chosen backend, read fresh from settings.
///
/// An unrecognised stored value degrades to the default rather than failing —
/// `update_settings` rejects bad values at the door, so this is belt and braces for a
/// hand-edited file or a downgrade. This always returns the chosen provider; backoff is
/// handled in [`complete_inner`] (a backing-off provider errors, it is not re-routed).
pub fn resolve() -> Box<dyn LlmBackend> {
    let s = load_runtime_settings();
    let cfg = LlmConfig::from_settings(&s);
    let chosen = LlmProvider::from_wire(&s.llm_provider).unwrap_or_default();
    backend_for(chosen, cfg)
}

/// Run one prose call through the user's provider, with the full fallback chain.
///
/// This — not [`resolve`] — is what callers should use. It owns the retry, the backoff and
/// the degrade-to-local rule, so no caller has to reimplement them (and get them subtly
/// different, which is how a fallback becomes a silent data-loss bug).
///
/// Returns the output and the provider that actually answered — which may not be the one
/// the user chose. Callers should record it on their span: an hour summarised by the local
/// model when the user picked Claude is not a failure, but it is a fact worth seeing.
///
/// Every logical call is one `llm.call` span with THREE sub-spans, so the trace waterfall
/// reads left-to-right as request → think → response:
/// * `llm.request` — the EXACT prompt (`llm_prompt`, the system prompt) and input
///   (`llm_input`, the user message) sent, FTS-indexed in OpenObserve.
/// * `llm.infer` — the actual backend call; its duration is the model's real think time,
///   and it carries the provider that answered, token counts and elapsed. The retry /
///   rate-limit / on-device fallback all live inside this one sub-span.
/// * `llm.response` — the model's EXACT answer (`llm_output`).
///
/// A parent stage (`worklog.report`, `worklog.workstream.build`) therefore shows one child
/// `llm.call` per call — two for the hourly report (activity-summary + activity-time), one
/// for the workstream fold — each expandable into its request/infer/response sub-spans. The
/// parent `llm.call` also carries a rolled-up summary (provider, char/token counts, elapsed)
/// so the call reads at a glance without expanding.
pub async fn complete(req: &PromptRequest) -> Result<(LlmOutput, LlmProvider), LlmError> {
    let span = tracing::info_span!(
        "llm.call",
        label = %req.label,
        provider = tracing::field::Empty,
        input_chars = req.user.len(),
        prompt_chars = req.system.len(),
        output_chars = tracing::field::Empty,
        input_tokens = tracing::field::Empty,
        output_tokens = tracing::field::Empty,
        elapsed_s = tracing::field::Empty,
    );
    async {
        // ── sub-span 1: the REQUEST ──────────────────────────────────────────
        // A field-carrier span (opens and closes at once) holding the EXACT
        // prompt (system) and input (user) sent, FTS-indexed in OpenObserve.
        tracing::info_span!(
            "llm.request",
            llm_prompt = %req.system,
            llm_input = %req.user,
            prompt_chars = req.system.len(),
            input_chars = req.user.len(),
        )
        .in_scope(|| {});

        // ── sub-span 2: the INFERENCE ────────────────────────────────────────
        // Wraps the actual backend call (its retry / rate-limit / on-device
        // fallback all live inside this one span); its duration is the model's
        // real think time. Provider/tokens/elapsed are recorded from within.
        let infer_span = tracing::info_span!(
            "llm.infer",
            provider = tracing::field::Empty,
            input_tokens = tracing::field::Empty,
            output_tokens = tracing::field::Empty,
            elapsed_s = tracing::field::Empty,
        );
        let result = async {
            let r = complete_inner(req).await;
            let cs = tracing::Span::current();
            match &r {
                Ok((out, provider)) => {
                    cs.record("provider", provider.as_str());
                    cs.record("input_tokens", out.input_tokens);
                    cs.record("output_tokens", out.output_tokens);
                    cs.record("elapsed_s", out.elapsed_s);
                }
                Err(e) => {
                    cs.record("provider", "error");
                    tracing::warn!(error = %e, label = %req.label, "llm.infer failed");
                }
            }
            r
        }
        .instrument(infer_span)
        .await;

        // ── sub-span 3: the RESPONSE ─────────────────────────────────────────
        // A field-carrier span holding the model's EXACT answer. Only on
        // success — a failure has no output to record.
        let parent = tracing::Span::current();
        match &result {
            Ok((out, provider)) => {
                tracing::info_span!(
                    "llm.response",
                    llm_output = %out.text,
                    output_chars = out.text.len(),
                )
                .in_scope(|| {});
                // Roll a compact summary up to the parent `llm.call` so the call
                // reads at a glance without expanding the children.
                parent.record("provider", provider.as_str());
                parent.record("output_chars", out.text.len());
                parent.record("input_tokens", out.input_tokens);
                parent.record("output_tokens", out.output_tokens);
                parent.record("elapsed_s", out.elapsed_s);
            }
            Err(_) => {
                parent.record("provider", "error");
            }
        }
        result
    }
    .instrument(span)
    .await
}

/// The resolution + retry body, wrapped by [`complete`]'s per-call span.
///
/// No on-device fallback: a backing-off or failing provider returns an error and the
/// caller leaves the work pending for the next cycle.
async fn complete_inner(req: &PromptRequest) -> Result<(LlmOutput, LlmProvider), LlmError> {
    let s = load_runtime_settings();
    let cfg = LlmConfig::from_settings(&s);
    let chosen = LlmProvider::from_wire(&s.llm_provider).unwrap_or_default();

    // Already backing off from an earlier rate limit → don't spend an attempt. Keyed
    // per-backend, so a limit on a provider the user has since switched away from doesn't
    // block the one they're on now.
    if is_backing_off(&backoff_key(chosen, &cfg)) {
        tracing::debug!(
            provider = chosen.as_str(),
            "llm: provider is backing off from an earlier rate limit — skipping this call"
        );
        return Err(LlmError::RateLimited(format!(
            "{} is backing off from an earlier rate limit",
            chosen.as_str()
        )));
    }

    let backend = backend_for(chosen, cfg.clone());
    let mut last: LlmError = LlmError::Failed("no attempt made".into());

    // A `Failed` may be a blip, so it earns one retry. A `RateLimited` never does —
    // the quota will not refill in the next two seconds.
    for attempt in 1..=2u32 {
        match backend.complete(req).await {
            Ok(out) => return Ok((out, chosen)),
            Err(LlmError::RateLimited(msg)) => {
                let backoff = reset_time::parse_backoff(&msg, chrono::Local::now())
                    .unwrap_or(RATE_LIMIT_BACKOFF);
                tracing::warn!(
                    provider = chosen.as_str(),
                    label = %req.label,
                    error = %msg,
                    backoff_s = backoff.as_secs(),
                    "llm: provider rate-limited — pausing it until the backoff expires"
                );
                start_backoff(&backoff_key(chosen, &cfg), backoff);
                return Err(LlmError::RateLimited(msg));
            }
            Err(e) => {
                tracing::warn!(
                    provider = chosen.as_str(),
                    label = %req.label,
                    attempt,
                    error = %e,
                    "llm: provider call failed"
                );
                last = e;
            }
        }
    }

    tracing::error!(
        provider = chosen.as_str(),
        label = %req.label,
        error = %last,
        "llm: provider failed with no fallback — leaving the work pending for next cycle"
    );
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `MERIDIAN_SETTINGS_PATH` is process-global and cargo runs tests in parallel.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn write_settings(dir: &std::path::Path, provider: &str) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, format!(r#"{{"llm_provider":"{provider}"}}"#)).unwrap();
        std::env::set_var("MERIDIAN_SETTINGS_PATH", &path);
        path
    }

    /// THE test for this design. The resolver re-reads settings on every call, so flipping
    /// the provider takes effect immediately — no restart, no cache invalidation, no
    /// reload hook. If someone "optimises" this with a OnceLock, this fails.
    #[test]
    fn resolve_rereads_settings_every_call() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_backoff();
        let dir = std::env::temp_dir().join(format!("meridian-resolver-{}", std::process::id()));

        write_settings(&dir, "claude");
        assert_eq!(resolve().provider(), LlmProvider::Claude);

        // Same process, no restart — just a different settings file.
        write_settings(&dir, "codex");
        assert_eq!(resolve().provider(), LlmProvider::Codex);

        write_settings(&dir, "cursor");
        assert_eq!(resolve().provider(), LlmProvider::Cursor);

        std::env::remove_var("MERIDIAN_SETTINGS_PATH");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unknown_provider_resolves_to_the_default() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_backoff();
        let dir =
            std::env::temp_dir().join(format!("meridian-resolver-unknown-{}", std::process::id()));
        write_settings(&dir, "gemini");
        assert_eq!(resolve().provider(), LlmProvider::default());
        std::env::remove_var("MERIDIAN_SETTINGS_PATH");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A backoff pauses the ROUTING for that provider but never rewrites the user's choice:
    /// `resolve()` still returns the chosen provider (backoff is enforced in
    /// `complete_inner`, which errors), and the stored setting is untouched.
    #[test]
    fn a_backoff_does_not_rewrite_the_users_choice() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_backoff();
        let dir =
            std::env::temp_dir().join(format!("meridian-resolver-backoff-{}", std::process::id()));
        let path = write_settings(&dir, "claude");

        start_backoff("claude", RATE_LIMIT_BACKOFF);
        assert_eq!(
            resolve().provider(),
            LlmProvider::Claude,
            "resolve still returns the chosen provider; the setting is sacred"
        );
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(
            on_disk.contains("claude"),
            "stored choice untouched: {on_disk}"
        );

        clear_backoff();
        std::env::remove_var("MERIDIAN_SETTINGS_PATH");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Per-backend keying: a rate limit on ONE provider must not block a DIFFERENT provider
    /// the user switches to. This is the invariant `complete_inner` relies on to skip a
    /// backing-off provider without touching the one now selected.
    #[test]
    fn backoff_is_keyed_per_provider() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_backoff();
        start_backoff("claude", RATE_LIMIT_BACKOFF);
        assert!(
            is_backing_off("claude"),
            "the limited provider is backing off"
        );
        assert!(!is_backing_off("codex"), "a different provider is not");
        clear_backoff();
        assert!(!is_backing_off("claude"), "and it clears");
    }

    #[test]
    fn every_provider_maps_to_a_backend_that_reports_itself() {
        let cfg = LlmConfig::from_settings(&Default::default());
        for p in LlmProvider::all() {
            assert_eq!(backend_for(p, cfg.clone()).provider(), p, "{p:?}");
        }
    }
}
