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

    pub async fn raw_generate(&self, system: &str, user: &str) -> Result<String> {
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
        Ok(data["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    pub async fn classify(&self, req: &ClassifyRequest) -> Result<ClassifyResponse> {
        let (system, user) = prompt::build_prompts(req);

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
        let task_key = prompt::extract_key(&content, &req.valid_keys);

        Ok(ClassifyResponse {
            task_key,
            method: format!("openai_compat({})", self.model),
        })
    }
}
