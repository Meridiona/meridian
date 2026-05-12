// meridian — normalises screenpipe activity into structured app sessions

use anyhow::{Context, Result};
use tracing::debug;

use crate::intelligence::classifier::prompt;
use crate::intelligence::classifier::{ClassifyRequest, ClassifyResponse};

pub struct OpenAiCompatBackend {
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl OpenAiCompatBackend {
    pub fn new(base_url: String, model: String) -> Self {
        Self {
            base_url,
            model,
            client: reqwest::Client::new(),
        }
    }

    #[tracing::instrument(
        skip(self, system, user),
        fields(
            backend = "openai_compat",
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
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ]
        });
        let url = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("POST /v1/chat/completions")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI-compat {} → {}: {}", url, status, text);
        }
        let data: serde_json::Value = resp.json().await.context("parsing response")?;
        let text = data["choices"][0]["message"]["content"]
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
            backend = "openai_compat",
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
            "max_tokens": 32,
            "response_format": {"type": "json_object"},
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ]
        });

        let url = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("POST /v1/chat/completions")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("OpenAI-compat API {} → {}: {}", url, status, text);
        }

        let data: serde_json::Value = resp.json().await.context("parsing response")?;
        let content = data["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        debug!(raw = %content, "OpenAI-compat raw response");
        if std::env::var("MERIDIAN_LOG_PROMPTS").is_ok() {
            tracing::trace!(response = %content, "llm response");
        }
        tracing::Span::current().record("latency_ms", start.elapsed().as_millis() as i64);
        let task_key = prompt::extract_key(&content, &req.valid_keys);
        tracing::Span::current().record("decision", task_key.as_deref().unwrap_or("none"));

        Ok(ClassifyResponse {
            task_key,
            method: format!("openai_compat({})", self.model),
        })
    }
}
