//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The single factory — settings in, live backend out — plus the retry and backoff rules.
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
//! # What happens when a call fails ([`complete`])
//!
//! | what happened | what we do |
//! |---|---|
//! | `Failed` (crashed, not installed, timed out) | retry once, then return the error |
//! | `RateLimited` (quota exhausted) | return **immediately** — retrying a quota is pointless — and stop calling that provider until it resets |
//! | a call arrives while the backoff is live | return the rate-limit error without dialling out at all |
//!
//! In every case the caller leaves the hour pending and retries on the next tick. That is
//! the whole error contract now.
//!
//! # There is no on-device fallback (deliberately removed)
//!
//! This used to substitute the on-device MLX model whenever the chosen provider failed or
//! was backing off. MLX — and the whole Python/services stack it ran in — has been removed
//! entirely, so that substitution is gone along with the "Local" provider it substituted:
//! generation now runs only through a third-party CLI or a custom endpoint.
//!
//! Losing it costs less than it looks. The substitution was already refused whenever the
//! local weights were not on disk (the old `llm_local_chat_model_ready` gate), so
//! "propagate the error, leave the hour pending, retry next tick" was an established path
//! that every caller already handled — it is now simply the only path. And an hour is not
//! lost by being deferred: the work is durable, the next tick picks it up, and by then the
//! backoff has usually expired. What the fallback really bought was *sooner*, at the price
//! of a materially weaker answer from a 2B model attributed to a provider the user never
//! picked.
//!
//! The backoff still lives **in memory here, not in settings.json**. The user's *choice* is
//! sacred: being rate-limited degrades the *timing*, it does not rewrite what they asked
//! for. Writing it to settings would mean a quota blip silently and permanently changed
//! their provider. That distinction is what keeps this from becoming Dayflow's
//! three-settings mess.
//!
//! # How long the backoff actually lasts
//!
//! Three sources, most authoritative first:
//!
//! 1. **A `Retry-After` / `x-ratelimit-reset-*` header**, parsed by
//!    [`super::openai_compat`] and carried on `LlmError::RateLimited::retry_after`. Custom
//!    endpoints only — a CLI subprocess has no headers.
//! 2. **The provider's own message**, via [`super::reset_time::parse_backoff`] — Claude Code
//!    and Codex both print their real reset hint ("resets 3pm", "try again in 5 hours"), so
//!    a call resumes the moment the provider says it will.
//! 3. **A flat default**, per transport ([`default_backoff`]). [`RATE_LIMIT_BACKOFF`] (30
//!    min) for a subscription; [`CUSTOM_RATE_LIMIT_BACKOFF`] (60 s) for a metered endpoint,
//!    whose binding limit is normally per-MINUTE and would recover long before the
//!    subscription figure expired.
//!
//! # Reactive here, proactive next door
//!
//! Everything above is after-the-fact: a 429 must land before anything slows down. The
//! complement is [`super::rate_limit`], which paces a custom endpoint's requests so the 429
//! does not happen in the first place. [`complete_inner`] calls it before each attempt, and
//! the two share a key derivation ([`provider_key`] / [`super::rate_limit::custom_key`]) so
//! an endpoint cannot be paced under one identity and parked under another.

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

/// Same idea as [`RATE_LIMIT_BACKOFF`] but for a metered endpoint, where the assumption
/// behind the 30-minute figure does not hold.
///
/// That constant is justified by SUBSCRIPTION semantics: a Claude/Codex plan that just
/// refused us is out for a 5-hour window, so half an hour is a cheap, safe guess. A custom
/// endpoint's binding limit is usually requests-per-MINUTE, which refills in under 60
/// seconds — parking it for 30 minutes on one blip would strand ~30 hours of pipeline work
/// for a quota that came back before the next poll tick.
///
/// Only reached when the endpoint sent no usable header AND its message was unparseable;
/// a well-behaved provider never gets here.
pub const CUSTOM_RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(60);

/// How long a production call will wait for a paced slot before giving up on pacing and
/// sending anyway.
///
/// Some of these callers are user-driven (drafting a task, synthesising a worklog, with
/// someone watching a spinner), so an unbounded wait is not acceptable here the way it is
/// for the probe. Proceeding unpaced is safe: the reactive backoff still catches the 429 if
/// one lands. Sized to cover a full window at the free tiers this targets — 30s is ~7 slots
/// at 15 rpm — so it only trips when genuinely many calls are queued at once.
const PACING_MAX_WAIT: Duration = Duration::from_secs(30);

/// The flat backoff to use when a provider gave us nothing to go on — keyed by transport,
/// because a flat-rate subscription and a per-minute metered quota recover on completely
/// different timescales.
fn default_backoff(provider: LlmProvider) -> Duration {
    match provider {
        LlmProvider::Custom => CUSTOM_RATE_LIMIT_BACKOFF,
        _ => RATE_LIMIT_BACKOFF,
    }
}

/// When each rate-limited backend may be used again, keyed by backend IDENTITY (see
/// [`provider_key`]) — NOT the bare [`LlmProvider`], because two custom endpoints share the
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
        SummariserError::RateLimited(m) => LlmError::rate_limited(m),
        SummariserError::Failed(m) => LlmError::Failed(m),
    }
}

/// The identity key for a resolved backend — used both as the in-memory backoff-map key
/// here, and as the [`super::runtime_health`] file-record key (`record_success`,
/// `record_failure`, `record_probe_success`, `latest_for` all take this, never a bare
/// [`LlmProvider`]). CLI providers key by name; a custom endpoint keys by its own id
/// (`custom:<id>`, via [`super::rate_limit::custom_key`]) so two endpoints - both the
/// `Custom` variant - are tracked independently rather than sharing the literal string
/// `"custom"`. Sharing it was a real bug: switching the active custom endpoint left the
/// backoff map harmless (it is in-memory and per-process, see [`RATE_LIMITED_UNTIL`]'s own
/// docs) but silently carried the PREVIOUS endpoint's persisted health/test verdict onto
/// the newly-active one, which only became visible once a second curated preset (Ollama,
/// alongside Groq) existed for a real user to switch between.
pub fn provider_key(provider: LlmProvider, custom_id: Option<&str>) -> String {
    match provider {
        LlmProvider::Custom => super::rate_limit::custom_key(custom_id.unwrap_or("")),
        _ => provider.as_str().to_string(),
    }
}

/// Groq is discontinued — this is an unconditional refusal, not a measured-failure verdict.
///
/// Shared by [`complete_inner`] (the production funnel: hourly folds, worklog drafts,
/// classification) AND [`super::detect::test_provider`] (manual "Test Connection"), so a
/// user can never see a Groq tile report a healthy test while the pipeline that actually
/// matters silently refuses every real call — the two would otherwise be produced by code
/// paths that had never been introduced to each other, exactly the trap `complete_inner`'s
/// health gate already exists to avoid for a *measured* failure. Groq's free-tier TPM
/// ceiling is too tight for Meridian's hourly prompt sizes; this is an ongoing production
/// failure, not a transient one worth spending a network call to re-verify.
pub fn groq_blocked(vendor: Option<&str>) -> Option<LlmError> {
    if vendor != Some("groq") {
        return None;
    }
    Some(LlmError::Failed(
        "Groq is discontinued - its free tier can't reliably serve Meridian's hourly \
         summaries. Open Settings → Intelligence and add a free Ollama key to resume."
            .to_string(),
    ))
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

/// How often a provider the health gate has refused is allowed one real call to find out
/// whether the verdict still holds.
///
/// The trade is between a wasted call and a silent outage. Too long and the exemption stops
/// being a recovery path - the point is that a provider fixed at 10:00 is working again by
/// about 10:15, not at 16:00 when a runtime observation would have aged out on its own.
/// Too short and a genuinely broken provider is dialled repeatedly, which on a metered
/// endpoint costs money to keep confirming bad news. Fifteen minutes sits under the hourly
/// fold cadence - the main thing an outage delays - so at most one hour's work is deferred.
const HEALTH_PROBE_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// When each provider was last allowed a probe past the health gate.
///
/// In-process and deliberately not persisted: a restart re-earning one probe is the right
/// behaviour, since a restart is the single most likely thing to have FIXED the provider
/// (new PATH, fresh credentials, a CLI installed while the daemon was down).
static LAST_HEALTH_PROBE: Mutex<Option<BTreeMap<String, Instant>>> = Mutex::new(None);

/// Whether the backend identified by `key` may spend one call re-checking a refusal,
/// stamping the attempt if so.
///
/// Takes the same [`provider_key`] identity every other per-backend map in this file keys
/// on, NOT a bare [`LlmProvider`] - see that fn's doc for why: two custom endpoints (e.g.
/// Groq and Ollama, both `LlmProvider::Custom`) would otherwise share one probe allowance,
/// so re-checking one endpoint's refusal could silently spend the other's exemption.
///
/// Check and stamp are one operation under one lock on purpose: several pipeline stages can
/// reach the gate concurrently on the same tick, and a check that did not immediately record
/// the grant would let all of them through together - turning "one call per interval" into a
/// small burst against a provider already believed to be failing.
fn health_probe_is_due(key: &str) -> bool {
    let mut guard = LAST_HEALTH_PROBE.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(BTreeMap::new);
    let now = Instant::now();
    match map.get(key) {
        Some(last) if now.duration_since(*last) < HEALTH_PROBE_INTERVAL => false,
        _ => {
            map.insert(key.to_string(), now);
            true
        }
    }
}

/// Forget every recorded probe, so the next refusal is exempted immediately. Test-only.
#[cfg(test)]
pub fn clear_health_probes() {
    let mut guard = LAST_HEALTH_PROBE.lock().unwrap_or_else(|e| e.into_inner());
    guard.get_or_insert_with(BTreeMap::new).clear();
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
/// hand-edited file or a downgrade.
///
/// Purely a factory: it returns the user's provider whatever its backoff state, because a
/// `Box<dyn LlmBackend>` has no way to say "backing off, don't call". Rate limits are
/// handled by [`complete`], which is what callers actually use.
pub fn resolve() -> Box<dyn LlmBackend> {
    let s = load_runtime_settings();
    let cfg = LlmConfig::from_settings(&s);
    backend_for(chosen_provider(), cfg)
}

/// WHICH provider the user has chosen, without building a backend.
///
/// The single place that reads the stored choice, so callers that only need to *reason*
/// about the provider (rather than call it) do not re-implement the parse-and-default
/// dance. Used by the coding-agent summariser's fallback to answer "is my global provider
/// the same CLI that just failed?" before spending a call to find out.
pub fn chosen_provider() -> LlmProvider {
    LlmProvider::from_wire(&load_runtime_settings().llm_provider).unwrap_or_default()
}

/// Run one prose call through the user's provider, with retry and rate-limit handling.
///
/// This — not [`resolve`] — is what callers should use. It owns the retry and the backoff,
/// so no caller has to reimplement them (and get them subtly different, which is how a
/// shared rule becomes a silent data-loss bug).
///
/// Returns the output and the provider that answered. That is always the provider the user
/// chose — nothing substitutes for it any more — but the value is still returned so callers
/// can record it on their span, and so the signature survives a future second provider.
///
/// Every logical call is one `llm.call` span with THREE sub-spans, so the trace waterfall
/// reads left-to-right as request → think → response:
/// * `llm.request` — the EXACT prompt (`llm_prompt`, the system prompt) and input
///   (`llm_input`, the user message) sent, FTS-indexed in OpenObserve.
/// * `llm.infer` — the actual backend call; its duration is the model's real think time,
///   and it carries the provider that answered, token counts and elapsed. The retry and the
///   rate-limit handling both live inside this one sub-span.
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
        // Wraps the actual backend call (its retry and rate-limit handling all
        // live inside this one span); its duration is the model's
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

/// Whether the recorded provider health should stop a call before it is made, and
/// what to say if so. Pure, so the decision is unit-testable without a settings
/// file, a `MERIDIAN_HOME`, or a CLI on the machine.
///
/// `Failed`, never `RateLimited`: the caller must leave its work pending and retry
/// on a later tick, not sit in a quota backoff for a problem that was never a
/// quota. And the message names the fix — "something went wrong" on a signed-out
/// CLI is the least actionable sentence this product can produce, and it is the
/// one the user reads at exactly the moment they can still do something about it.
fn health_refusal(health: &super::detect::InUseProviderHealth) -> Option<LlmError> {
    // A rate limit is a wait, not a refusal — the backoff below already handles it,
    // and blocking here would turn a self-clearing pause into a dead product.
    if health.ok {
        return None;
    }
    let detail = health.detail.as_deref().unwrap_or("not available");
    Some(LlmError::Failed(format!(
        "{} is not available ({detail}). Open Settings → Intelligence to sign in or pick another provider.",
        health.name,
    )))
}

/// The resolution + retry body, wrapped by [`complete`]'s per-call span.
///
/// No on-device fallback: a backing-off or failing provider returns an error and the
/// caller leaves the work pending for the next cycle.
async fn complete_inner(req: &PromptRequest) -> Result<(LlmOutput, LlmProvider), LlmError> {
    let s = load_runtime_settings();
    let cfg = LlmConfig::from_settings(&s);
    let chosen = LlmProvider::from_wire(&s.llm_provider).unwrap_or_default();
    // The ONE identity this whole call is tracked under — backoff, and every
    // `runtime_health` read/write below. Computed once so it can't drift between call
    // sites the way two independent `provider.as_str()`/`backoff_key(...)` calls could.
    let key = provider_key(chosen, cfg.custom.as_ref().map(|c| c.id.as_str()));

    if let Some(refusal) = groq_blocked(s.active_custom_provider().map(|c| c.vendor.as_str())) {
        tracing::warn!(
            label = %req.label,
            "llm: refusing the call - groq is discontinued, switch to ollama in Settings → Intelligence"
        );
        return Err(refusal);
    }

    // ── The provider is known to be unavailable ──────────────────────────────
    //
    // THE GATE THAT WAS MISSING. Until this existed, `in_use_provider_health` had
    // exactly one consumer in the entire codebase - the dashboard banner - so the
    // app could tell the user "Claude Code isn't available. Hourly summaries are
    // paused." across the top of the screen while, four inches below it, Generate
    // Worklog spawned that same provider and drafted an update. The two statements
    // were produced by code paths that had never been introduced to each other,
    // and the user is entitled to conclude one of them is lying.
    //
    // Putting it HERE and nowhere else is the whole point. This function is the
    // single funnel every prose call passes through - hourly folds, day summaries,
    // worklog drafts, classification - so one check covers all of them and cannot
    // be forgotten by the next feature that needs a model. A check bolted onto the
    // worklog command instead would have left the other four callers lying.
    //
    // ONLY on `ok == false`, never on `rate_limited`: a rate limit clears on its
    // own and is already handled below by the backoff, which is a wait rather than
    // a refusal. And "paused" is the truthful word for what happens next - the
    // caller leaves its unit of work pending and retries on the following tick, so
    // nothing is lost by refusing early; the same work is drafted the moment the
    // provider is working again.
    // ONE EXEMPTION, and the gate does not work without it. Everything above is a refusal
    // based on a recorded verdict, and the only thing that can overturn that verdict is a
    // successful call - which this gate is refusing to make. Left as a plain `return Err`
    // the check is self-latching: a runtime failure blacks out every AI feature for the
    // full six hours until the observation ages out, and a failed manual Test Connection,
    // which carries no expiry at all, blacks them out until the user happens to press that
    // button a second time. Neither is announced; the features simply stop.
    //
    // So a `probeable` verdict (see `InUseProviderHealth::probeable` - a MEASURED failure,
    // not a missing install and not an assertion) buys one real call per
    // `HEALTH_PROBE_INTERVAL` to find out whether it is still true. If that call succeeds
    // the verdict is cleared on the spot and normal service resumes; if it fails, the
    // refusal stands and costs one call every fifteen minutes to keep checking.
    let health = super::detect::in_use_provider_health(&s).await;
    let mut probing = false;
    if let Some(refusal) = health_refusal(&health) {
        if !(health.probeable && health_probe_is_due(&key)) {
            tracing::warn!(
                provider = chosen.as_str(),
                label = %req.label,
                error = %refusal,
                probeable = health.probeable,
                "llm: refusing the call - the chosen provider is not available"
            );
            return Err(refusal);
        }
        probing = true;
        tracing::info!(
            provider = chosen.as_str(),
            label = %req.label,
            error = %refusal,
            "llm: provider is marked unavailable - letting one call through to re-check"
        );
    }

    // Still inside an earlier rate limit's window. Refuse without dialling out: the quota
    // has not refilled, so the call would spend a round-trip to earn the same 429 and, on a
    // metered endpoint, could count against the very quota we are waiting on. Keyed
    // per-backend, so a limit on a provider the user has since switched away from does not
    // block this one.
    if is_backing_off(&key) {
        tracing::debug!(
            provider = chosen.as_str(),
            label = %req.label,
            "llm: provider is rate-limited; skipping until the backoff expires"
        );
        // No `retry_after`: this is our own bookkeeping talking, not the provider. The
        // real reset time is already held in the backoff map, and inventing a duration
        // here would let a caller mistake it for something the endpoint actually said.
        let err = LlmError::rate_limited(format!(
            "{} is rate-limited; waiting for the quota to reset",
            chosen.as_str()
        ));
        // Re-stamp the runtime record so the banner stays up for the whole backoff window.
        // Without this the observation would age out mid-outage, because every call inside
        // the window returns here and never reaches the recording site below.
        super::runtime_health::record_failure(&key, &err);
        return Err(err);
    }

    let backend = backend_for(chosen, cfg.clone());
    let mut last: LlmError = LlmError::Failed("no attempt made".into());

    // A `Failed` may be a blip, so it earns one retry. A `RateLimited` never does —
    // the quota will not refill in the next two seconds.
    for attempt in 1..=2u32 {
        // Pace BEFORE the request, not after a failure — the whole point is that the 429
        // never happens. No-op unless this is a custom endpoint with an `rpm` configured.
        if chosen == LlmProvider::Custom {
            if let Some(ep) = cfg.custom.as_ref() {
                super::rate_limit::acquire(&key, ep.rpm, Some(PACING_MAX_WAIT)).await;
            }
        }
        match backend.complete(req).await {
            Ok(out) => {
                // The provider answered: clear any runtime health flag it was carrying, so a
                // recovered provider drops the banner without waiting on a manual re-test.
                //
                // A probe takes the unconditional path. `record_success` decides whether
                // there is anything to clear by looking at the RUNTIME record only, and the
                // verdict a probe was sent to disprove is usually in the manual-test cache,
                // which that check cannot see - so it would write nothing and the next call
                // would be refused again by the verdict this one just disproved.
                if probing {
                    super::runtime_health::record_probe_success(&key);
                } else {
                    super::runtime_health::record_success(&key);
                }
                return Ok((out, chosen));
            }
            Err(LlmError::RateLimited {
                message,
                retry_after,
            }) => {
                // Most authoritative first: a header the endpoint actually sent, then the
                // provider's own prose, then a flat per-transport default. The header wins
                // because it is the only one of the three that is machine-generated.
                let backoff = retry_after
                    .or_else(|| reset_time::parse_backoff(&message, chrono::Local::now()))
                    .unwrap_or_else(|| default_backoff(chosen));
                tracing::warn!(
                    provider = chosen.as_str(),
                    label = %req.label,
                    error = %message,
                    backoff_s = backoff.as_secs(),
                    backoff_source = if retry_after.is_some() { "header" } else { "message-or-default" },
                    "llm: provider rate-limited — skipping it until the quota resets"
                );
                start_backoff(&key, backoff);
                last = LlmError::RateLimited {
                    message,
                    retry_after,
                };
                break;
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

    // Out of options: no provider answers on the user's behalf. The caller leaves its unit
    // of work pending and retries next tick, by which time a rate-limit window has usually
    // expired and a transient failure has usually cleared.
    tracing::error!(
        provider = chosen.as_str(),
        label = %req.label,
        error = %last,
        "llm: provider call did not succeed — leaving the work pending for the next tick"
    );
    // The one place every exhausted call converges, so the dashboard banner learns about a
    // provider that is installed and tested-fine but failing every real call — the state that
    // silently leaves worklog hours pending and the day timeline empty.
    super::runtime_health::record_failure(&key, &last);
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_env::ScopedSettings;

    /// `MERIDIAN_SETTINGS_PATH` is process-global, so the lock lives in
    /// [`crate::test_env`] rather than here. A module-local one used to guard
    /// these tests, and `telemetry_spool::redact` had its own - two locks over
    /// one variable, which is the same as none. See that module's docs.
    fn settings_for(tag: &str, provider: &str) -> ScopedSettings {
        ScopedSettings::install(tag, &format!(r#"{{"llm_provider":"{provider}"}}"#))
    }

    #[test]
    fn groq_blocked_refuses_only_groq() {
        assert!(groq_blocked(Some("groq")).is_some());
        assert!(groq_blocked(Some("ollama")).is_none());
        assert!(groq_blocked(None).is_none());
    }

    /// THE test for this design. The resolver re-reads settings on every call, so flipping
    /// the provider takes effect immediately — no restart, no cache invalidation, no
    /// reload hook. If someone "optimises" this with a OnceLock, this fails.
    #[test]
    fn resolve_rereads_settings_every_call() {
        // Guard FIRST: `clear_backoff` mutates a process-global map the
        // sibling tests also write, so it must happen under the lock.
        let settings = settings_for("resolver-rereads", "claude");
        clear_backoff();
        assert_eq!(resolve().provider(), LlmProvider::Claude);

        // Same process, no restart — just a different settings file.
        settings.rewrite(r#"{"llm_provider":"codex"}"#);
        assert_eq!(resolve().provider(), LlmProvider::Codex);

        settings.rewrite(r#"{"llm_provider":"cursor"}"#);
        assert_eq!(resolve().provider(), LlmProvider::Cursor);
    }

    /// THE bug this gate exists for.
    ///
    /// Before it, `in_use_provider_health` had exactly one consumer in the whole
    /// codebase - the dashboard banner - so the app displayed "Claude Code isn't
    /// available. Hourly summaries are paused." across the top of the screen while
    /// Generate Worklog, four inches below it, cheerfully spawned that same
    /// provider and drafted an update. Two code paths that had never been
    /// introduced to each other, each confidently contradicting the other.
    ///
    /// The check lives in `complete` and nowhere else on purpose: it is the single
    /// funnel every prose call goes through, so one gate covers hourly folds, day
    /// summaries, worklog drafts and classification alike, and the next feature
    /// that needs a model inherits it without having to know it exists.
    #[test]
    fn an_unavailable_provider_refuses_the_call_and_says_how_to_fix_it() {
        let health =
            |ok, rate_limited, detail: Option<&str>| crate::llm::detect::InUseProviderHealth {
                ok,
                rate_limited,
                name: "Claude Code".to_string(),
                detail: detail.map(str::to_string),
                // Irrelevant here: `health_refusal` decides WHETHER a verdict refuses, and
                // `probeable` only decides whether `complete_inner` spends a call testing
                // one that already does. Pinned false so this stays a test of the ladder.
                probeable: false,
            };

        // Unavailable → refuse, carrying the reason AND the fix.
        let err = health_refusal(&health(false, false, Some("not signed in")))
            .expect("an unavailable provider must not reach the backend");
        assert!(matches!(err, LlmError::Failed(_)), "got {err:?}");
        let msg = err.to_string();
        assert!(msg.contains("not signed in"), "keeps the reason: {msg}");
        assert!(
            msg.contains("Settings") && msg.contains("Intelligence"),
            "points at the fix: {msg}"
        );

        // Unavailable with no recorded detail still refuses — a missing message is
        // not evidence that the provider works.
        assert!(health_refusal(&health(false, false, None)).is_some());

        // RATE-LIMITED IS NOT A REFUSAL. It clears on its own and the backoff below
        // already handles it as a wait; gating here would turn a self-healing pause
        // into a product that stops working until someone intervenes.
        assert!(health_refusal(&health(true, true, Some("usage limit"))).is_none());

        // Healthy → nothing in the way.
        assert!(health_refusal(&health(true, false, None)).is_none());
    }

    /// The exemption that keeps the health gate from latching shut.
    ///
    /// The gate refuses on a recorded verdict, and only a successful call can replace
    /// that verdict — so without a periodic exemption the refusal is its own evidence.
    /// A failed manual test never expires, which makes the blackout permanent rather
    /// than merely long, and nothing tells the user any of it.
    #[test]
    fn a_refused_provider_earns_one_probe_per_interval() {
        // `LAST_HEALTH_PROBE` is process-global, like the backoff map -
        // `two_custom_endpoints_earn_independent_probe_allowances` also calls
        // `clear_health_probes()`, and without this lock the two can interleave and wipe
        // each other's in-progress state. See `a_backoff_on_one_provider_does_not_stall_another`'s
        // comment for the same race on the sibling map.
        let _lock = crate::test_env::lock_settings_env();
        clear_health_probes();

        // First refusal after the interval: one call goes through to re-check.
        assert!(
            health_probe_is_due(&provider_key(LlmProvider::Claude, None)),
            "a refused provider must get a chance to prove the verdict wrong"
        );

        // Everything else in the window is still refused. This is the half that keeps
        // the exemption from becoming "ignore the gate": several pipeline stages hit
        // this on the same tick, and letting them all through would dial a provider
        // believed to be broken once per stage.
        for _ in 0..5 {
            assert!(
                !health_probe_is_due(&provider_key(LlmProvider::Claude, None)),
                "only ONE call per interval may pass the gate"
            );
        }

        // Per provider, not global: a broken Claude must not consume Codex's probe.
        assert!(
            health_probe_is_due(&provider_key(LlmProvider::Codex, None)),
            "the allowance is keyed per provider"
        );
    }

    /// The whole point of keying on `provider_key` rather than the bare `LlmProvider`: two
    /// distinct custom endpoints (both `LlmProvider::Custom`) must earn independent probe
    /// allowances, exactly like the health/test-cache contamination bug this mirrors (see
    /// `provider_key`'s own doc).
    #[test]
    fn two_custom_endpoints_earn_independent_probe_allowances() {
        // See `a_refused_provider_earns_one_probe_per_interval`'s comment - same
        // process-global map, same need to serialize against it.
        let _lock = crate::test_env::lock_settings_env();
        clear_health_probes();
        let groq = provider_key(LlmProvider::Custom, Some("groq-endpoint"));
        let ollama = provider_key(LlmProvider::Custom, Some("ollama-endpoint"));

        assert!(
            health_probe_is_due(&groq),
            "the first custom endpoint gets its probe"
        );
        assert!(
            !health_probe_is_due(&groq),
            "that SAME endpoint is exempt again within the interval"
        );
        assert!(
            health_probe_is_due(&ollama),
            "a DIFFERENT custom endpoint must not have spent its allowance too"
        );
    }

    #[test]
    fn an_unknown_provider_resolves_to_the_default() {
        let _settings = settings_for("resolver-unknown", "gemini");
        clear_backoff();
        assert_eq!(resolve().provider(), LlmProvider::default());
    }

    /// A rate limit degrades the TIMING, never the user's choice. With the on-device
    /// fallback gone there is nothing to route *to*, so what has to hold is narrower and
    /// more important: the backoff is recorded, the stored setting is untouched, and
    /// [`resolve`] keeps handing back the provider they picked.
    #[test]
    fn a_rate_limit_backoff_never_rewrites_the_users_choice() {
        let settings = settings_for("resolver-backoff", "claude");
        clear_backoff();

        start_backoff("claude", RATE_LIMIT_BACKOFF);
        assert!(
            is_backing_off("claude"),
            "the limit is recorded so `complete` can refuse without dialling out"
        );
        assert_eq!(
            resolve().provider(),
            LlmProvider::Claude,
            "the factory still reports the user's provider — nothing substitutes for it"
        );
        let on_disk = std::fs::read_to_string(settings.path()).unwrap();
        assert!(
            on_disk.contains("claude"),
            "and the stored choice is untouched: {on_disk}"
        );

        clear_backoff();
        assert!(!is_backing_off("claude"), "calls resume once it expires");
    }

    /// The fix for the cross-provider backoff bug: a rate limit on ONE provider must not
    /// stall a DIFFERENT provider the user switches to. Per-backend keying makes the switch
    /// escape the backoff with no clear-on-write signal (which couldn't cross the
    /// tray↔daemon process boundary anyway).
    #[test]
    fn a_backoff_on_one_provider_does_not_stall_another() {
        // Takes the lock despite reading no settings. The backoff map is ALSO
        // process-global, and every sibling test here calls `clear_backoff()` -
        // so one of them landing between this test's `start_backoff` and its
        // assertion wipes the entry and fails it. The old module-local
        // `ENV_LOCK` was quietly doing double duty as the backoff lock too;
        // dropping it here (this test touches no settings) reintroduced the
        // race, at roughly 1 run in 15.
        let _lock = crate::test_env::lock_settings_env();
        clear_backoff();

        start_backoff("claude", RATE_LIMIT_BACKOFF);
        assert!(is_backing_off("claude"));
        assert!(
            !is_backing_off("codex"),
            "switching to a provider that isn't rate-limited escapes the backoff"
        );

        clear_backoff();
    }

    #[test]
    fn every_provider_maps_to_a_backend_that_reports_itself() {
        let cfg = LlmConfig::from_settings(&Default::default());
        for p in LlmProvider::all() {
            assert_eq!(backend_for(p, cfg.clone()).provider(), p, "{p:?}");
        }
    }
}
