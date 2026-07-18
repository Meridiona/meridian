//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Config for the LLM backends — resolved from settings.json, with env for the knobs.
//!
//! The *provider* comes from settings (the user's choice, see [`meridian_core::settings`]);
//! the timeouts and the MLX address come from env, matching how the summariser already
//! configures itself. There is no env override for the provider on purpose: settings.json
//! is the single source of truth, or we are back to two.

use std::path::PathBuf;

use meridian_core::settings::RuntimeSettings;

/// Everything a backend needs beyond the prompt itself.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// Optional model override within the provider (`--model` / `-m`). Empty = default.
    pub model: String,
    /// Working directory for CLI subprocesses. `~/.meridian`.
    pub meridian_home: PathBuf,
    /// Per-call timeout for a CLI backend. Generous: a cloud model doing a real hour of
    /// screen text can take a while, and the cost of a false timeout is a lost hour.
    pub cli_timeout_s: u64,
    /// The resolved custom endpoint, when the user's selected provider is
    /// [`meridian_core::LlmProvider::Custom`] and its id names a live registry row.
    ///
    /// Resolving it HERE is what keeps `backend_for(provider, cfg) -> Box<dyn LlmBackend>`
    /// infallible: the fallible id→row lookup happens while building the config, and a
    /// `None` that reaches the backend is reported as "selected but not configured" at call
    /// time rather than silently becoming a different provider.
    ///
    /// Carries the API KEY — never log or serialise this field (see
    /// [`crate::llm::openai_compat::CustomEndpoint`]).
    pub custom: Option<crate::llm::openai_compat::CustomEndpoint>,
}

fn env_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

impl LlmConfig {
    /// Build from the user's settings. The provider itself is read by the resolver; this
    /// carries the rest — including the ACTIVE custom endpoint, if that is what they chose.
    ///
    /// For a `custom:<id>` LLM-Lab variant (which addresses an endpoint the user has not
    /// necessarily selected for production), build this and then override [`Self::custom`]
    /// via [`Self::with_custom`].
    pub fn from_settings(s: &RuntimeSettings) -> Self {
        let home = std::env::var("MERIDIAN_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let h = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
                PathBuf::from(h).join(".meridian")
            });
        Self {
            model: s.llm_provider_model.clone().unwrap_or_default(),
            meridian_home: home,
            cli_timeout_s: env_or("LLM_CLI_TIMEOUT_S", 300),
            custom: s.active_custom_provider().map(endpoint_from_row),
        }
    }

    /// Point this config at a specific custom endpoint, whatever the user's saved selection
    /// is — the LLM Lab runs a `custom:<id>` variant against an endpoint that is merely
    /// configured, not necessarily chosen.
    pub fn with_custom(mut self, row: &meridian_core::CustomLlmProvider) -> Self {
        self.custom = Some(endpoint_from_row(row));
        self
    }
}

/// Registry row → the endpoint the backend actually needs.
///
/// The rung sent to the backend is [`meridian_core::CustomLlmProvider::effective_rung`] —
/// the WEAKEST across the probed schemas, not the best. A per-schema lookup would be more
/// generous, but it would let a call ask for a mode this endpoint only honours for *some*
/// schemas; the weakest rung is the one it can hold for all of them.
fn endpoint_from_row(
    row: &meridian_core::CustomLlmProvider,
) -> crate::llm::openai_compat::CustomEndpoint {
    crate::llm::openai_compat::CustomEndpoint {
        id: row.id.clone(),
        base_url: row.base_url.clone(),
        model: row.model.clone(),
        api_key: row.api_key.clone(),
        rung: row.effective_rung(),
    }
}

impl From<&RuntimeSettings> for LlmConfig {
    fn from(s: &RuntimeSettings) -> Self {
        Self::from_settings(s)
    }
}
