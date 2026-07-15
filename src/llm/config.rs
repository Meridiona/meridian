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
    /// Per-call timeout for the on-device model.
    pub local_timeout_s: u64,
    pub mlx_host: String,
    pub mlx_port: u16,
}

fn env_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

impl LlmConfig {
    /// Build from the user's settings. The provider itself is read by the resolver; this
    /// carries the rest.
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
            local_timeout_s: env_or("LLM_LOCAL_TIMEOUT_S", 180),
            mlx_host: std::env::var("MLX_SERVER_HOST").unwrap_or_else(|_| "127.0.0.1".into()),
            mlx_port: env_or("MLX_SERVER_PORT", 7823u16),
        }
    }
}

impl From<&RuntimeSettings> for LlmConfig {
    fn from(s: &RuntimeSettings) -> Self {
        Self::from_settings(s)
    }
}
