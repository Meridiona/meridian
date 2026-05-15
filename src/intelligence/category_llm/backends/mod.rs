// meridian — normalises screenpipe activity into structured app sessions

pub mod claude;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub mod foundation;
pub mod openai_compat;

use super::LlmBackend;
use crate::config::LlmBackendConfig;

pub fn build_backend(config: &LlmBackendConfig) -> LlmBackend {
    match config {
        LlmBackendConfig::Disabled => LlmBackend::Disabled,

        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        LlmBackendConfig::FoundationModels => LlmBackend::Foundation(foundation::FoundationBackend),

        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        LlmBackendConfig::FoundationModels => {
            eprintln!(
                "warning: Foundation Models requested but not on macOS aarch64 — using Disabled"
            );
            LlmBackend::Disabled
        }

        LlmBackendConfig::OpenAiCompat { base_url, model } => LlmBackend::OpenAiCompat(
            openai_compat::OpenAiCompatBackend::new(base_url.to_owned(), model.to_owned()),
        ),

        LlmBackendConfig::Claude { api_key, model } => LlmBackend::Claude(
            claude::ClaudeBackend::new(api_key.to_owned(), model.to_owned()),
        ),
    }
}
