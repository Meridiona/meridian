// meridian — normalises screenpipe activity into structured app sessions

use anyhow::{Context, Result};
use tracing::debug;

use crate::intelligence::classifier::prompt;
use crate::intelligence::classifier::{ClassifyRequest, ClassifyResponse};

const CLAUDE_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct ClaudeBackend {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl ClaudeBackend {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            client: reqwest::Client::new(),
        }
    }

    #[tracing::instrument(
        skip(self, system, user),
        fields(
            backend = "claude",
            model = %self.model,
            prompt_len = user.len(),
            latency_ms = tracing::field::Empty,
        )
    )]
    pub async fn raw_generate(&self, system: &str, user: &str) -> Result<String> {
        if std::env::var("MERIDIAN_LOG_PROMPTS").is_ok() {
            tracing::trace!(prompt = %user, "llm prompt");
        }
        let start = std::time::Instant::now();
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 64,
            "system": system,
            "messages": [{"role": "user", "content": user}]
        });
        let resp = self
            .client
            .post(CLAUDE_API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .context("POST /v1/messages")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Claude API {} → {}: {}", CLAUDE_API_URL, status, text);
        }
        let data: serde_json::Value = resp.json().await.context("parsing Claude response")?;
        let text = data["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();
        tracing::Span::current().record("latency_ms", start.elapsed().as_millis() as i64);
        if std::env::var("MERIDIAN_LOG_PROMPTS").is_ok() {
            tracing::trace!(response = %text, "llm response");
        }
        Ok(text)
    }

    #[tracing::instrument(
        skip(self, req),
        fields(
            backend = "claude",
            model = %self.model,
            app_name = %req.app_name,
            latency_ms = tracing::field::Empty,
            decision = tracing::field::Empty,
        )
    )]
    pub async fn classify(&self, req: &ClassifyRequest) -> Result<ClassifyResponse> {
        let (system, user) = prompt::build_prompts(req);
        if std::env::var("MERIDIAN_LOG_PROMPTS").is_ok() {
            tracing::trace!(prompt = %user, "llm prompt");
        }
        let start = std::time::Instant::now();

        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 64,
            "system": system,
            "messages": [
                {"role": "user", "content": user}
            ]
        });

        let resp = self
            .client
            .post(CLAUDE_API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .context("POST /v1/messages")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Claude API {} → {}: {}", CLAUDE_API_URL, status, text);
        }

        let data: serde_json::Value = resp.json().await.context("parsing Claude response")?;
        let content = data["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();

        debug!(raw = %content, "Claude raw response");
        if std::env::var("MERIDIAN_LOG_PROMPTS").is_ok() {
            tracing::trace!(response = %content, "llm response");
        }
        tracing::Span::current().record("latency_ms", start.elapsed().as_millis() as i64);
        let task_key = prompt::extract_key(&content, &req.valid_keys);
        tracing::Span::current().record("decision", task_key.as_deref().unwrap_or("none"));

        Ok(ClassifyResponse {
            task_key,
            method: format!("claude({})", self.model),
        })
    }
}
