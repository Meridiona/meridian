// meridian — normalises screenpipe activity into structured app sessions

pub mod backends;
pub mod prompt;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct PmTaskRef {
    pub key: String,
    pub title: String,
}

#[derive(Debug)]
pub struct ClassifyRequest {
    pub app_name: String,
    pub duration_s: i64,
    pub windows: Vec<String>,
    pub ocr_snippet: String,
    pub tasks: Vec<PmTaskRef>,
    pub valid_keys: HashSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassifyResponse {
    pub task_key: Option<String>,
    pub method: String,
}

pub enum LlmBackend {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    Foundation(backends::foundation::FoundationBackend),
    OpenAiCompat(backends::openai_compat::OpenAiCompatBackend),
    Claude(backends::claude::ClaudeBackend),
    Disabled,
}

impl LlmBackend {
    pub fn name(&self) -> &'static str {
        match self {
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            Self::Foundation(_) => "foundation_models",
            Self::OpenAiCompat(_) => "openai_compat",
            Self::Claude(_) => "claude",
            Self::Disabled => "disabled",
        }
    }

    pub fn is_foundation_models(&self) -> bool {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            matches!(self, Self::Foundation(_))
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            false
        }
    }

    pub async fn raw_generate(&self, system: &str, user: &str) -> Result<String> {
        match self {
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            Self::Foundation(b) => b.raw_generate(system, user).await,
            Self::OpenAiCompat(b) => b.raw_generate(system, user).await,
            Self::Claude(b) => b.raw_generate(system, user).await,
            Self::Disabled => anyhow::bail!("LLM backend is disabled"),
        }
    }

    pub async fn generate_category(&self, system: &str, user: &str) -> Result<(String, String)> {
        match self {
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            Self::Foundation(b) => b.generate_category(system, user).await,
            _ => anyhow::bail!(
                "generate_category is only available on the Foundation Models backend"
            ),
        }
    }

    pub async fn classify(&self, req: &ClassifyRequest) -> Result<ClassifyResponse> {
        match self {
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            Self::Foundation(b) => b.classify(req).await,
            Self::OpenAiCompat(b) => b.classify(req).await,
            Self::Claude(b) => b.classify(req).await,
            Self::Disabled => Ok(ClassifyResponse {
                task_key: None,
                method: "disabled".to_string(),
            }),
        }
    }
}
