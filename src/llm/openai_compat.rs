//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Custom cloud backend — an OpenAI-compatible `/chat/completions` endpoint the user
//! configured themselves (OpenAI, Gemini's compat endpoint, OpenRouter, Groq, …).
//!
//! # What makes this one different
//! Every other backend is either a CLI on the user's own flat-rate subscription or the
//! on-device model. This one is a direct HTTP call on the user's own API key, and it is the
//! only provider that spends **metered money per call** — so a runaway retry here has a
//! bill attached, not just a wasted minute.
//!
//! # One backend, many vendors
//! The endpoint's identity (base URL, key, model) comes from a
//! [`meridian_core::CustomLlmProvider`] registry row, not from this type or the
//! [`LlmProvider`] enum — there can be several configured at once. The request shape is
//! `local.rs`'s, which already speaks this protocol to the MLX server; the deltas are the
//! base URL, `Authorization: Bearer`, and [`crate::llm::schema::strictify`] on the way out.
//!
//! # Structured output is measured, never assumed
//! "OpenAI-compatible" endpoints disagree about schemas — measured 2026-07-17, OpenAI
//! rejects a schema whose `required` omits an optional key while Gemini's compat endpoint
//! accepts it. What each endpoint actually honours is probed once when it is added and
//! recorded on its row as a [`meridian_core::SchemaRung`]; this backend just sends the
//! recorded rung's shape. See [`crate::llm::detect`] for the probe and the gate.
//!
//! # Who calls this
//! [`crate::llm::resolver::backend_for`] for [`LlmProvider::Custom`] — the production
//! provider when the user selected one, and the LLM Lab for a `custom:<id>` variant.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{LlmBackend, LlmConfig, LlmError, LlmOutput, LlmProvider, PromptRequest};
use meridian_core::SchemaRung;

/// The resolved endpoint a call runs against — the registry row's fields, minus everything
/// this backend has no business knowing (the display name, the vendor label).
///
/// Deliberately NOT `meridian_core::CustomLlmProvider`: that is the storage form and its
/// `api_key` must not spread further than it has to.
#[derive(Debug, Clone)]
pub struct CustomEndpoint {
    /// The registry id — the one safe thing to LOG. Never log the key or the URL.
    pub id: String,
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    /// What this endpoint was measured to honour for the schema being sent.
    pub rung: SchemaRung,
}

impl CustomEndpoint {
    /// `<base_url>/chat/completions`, tolerating a trailing slash on the stored base.
    fn chat_url(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }
}

pub struct OpenAiCompatBackend {
    pub cfg: LlmConfig,
}

#[async_trait]
impl LlmBackend for OpenAiCompatBackend {
    fn provider(&self) -> LlmProvider {
        LlmProvider::Custom
    }

    async fn complete(&self, req: &PromptRequest) -> Result<LlmOutput, LlmError> {
        let t0 = std::time::Instant::now();

        // Construction is infallible (`backend_for` returns a Box, not a Result), so an
        // unconfigured provider can only be reported HERE. It must fail loudly: the setting
        // says "custom" and silently answering from some other model would put a provider
        // the user did not choose on their timeline — with their real hours in it.
        let ep = self.cfg.custom.as_ref().ok_or_else(|| {
            LlmError::Failed(
                "custom provider is selected but not configured - add an endpoint in Settings, \
                 or pick another provider"
                    .to_string(),
            )
        })?;

        // NO llm_gate permit: the gate exists to serialise the single Metal device, and this
        // call runs on someone else's hardware. Taking it would throttle a cloud endpoint
        // behind the local model for no reason ("gate the GPU, don't gate the subscription").
        let url = ep.chat_url();
        let mut body = json!({
            "model": ep.model,
            "max_tokens": req.max_tokens,
            "messages": [
                {"role": "system", "content": req.system},
                {"role": "user",   "content": req.user},
            ],
        });
        apply_schema(&mut body, req, ep);

        // The endpoint id, not the URL or the key: a base URL can carry a key in a query
        // string, and this line goes to the telemetry spool which ships in a diagnostics
        // bundle. The id is enough to know WHICH endpoint answered.
        tracing::debug!(
            endpoint_id = %ep.id,
            model = %ep.model,
            rung = ?ep.rung,
            label = %req.label,
            "custom provider: sending"
        );

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(self.cfg.cli_timeout_s))
            .build()
            .map_err(|e| LlmError::Failed(format!("custom provider client: {e}")))?;

        let resp = client
            .post(&url)
            .bearer_auth(&ep.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                // `e` can carry the URL; it cannot carry the key (reqwest redacts the auth
                // header), but keep the message about the endpoint rather than the request.
                tracing::warn!(endpoint_id = %ep.id, error = %e, "custom provider unreachable");
                LlmError::Failed(format!("custom provider unreachable: {e}"))
            })?;

        let status = resp.status();
        if !status.is_success() {
            let detail = resp.text().await.unwrap_or_default();
            return Err(classify_error(status, &detail, &ep.id));
        }

        let payload: Value = resp
            .json()
            .await
            .map_err(|e| LlmError::Failed(format!("custom provider response not JSON: {e}")))?;

        let text = payload["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string();
        if text.is_empty() {
            return Err(LlmError::Failed(
                "custom provider returned an empty answer".into(),
            ));
        }

        let usage = &payload["usage"];
        Ok(LlmOutput {
            text,
            input_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0) as u32,
            output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0) as u32,
            elapsed_s: t0.elapsed().as_secs_f64(),
        })
    }
}

/// Ask for structured output in the strongest form this endpoint was MEASURED to honour.
///
/// Sending a mode the endpoint rejects costs a 400 and a wasted paid round trip, so the
/// rung is read from the probe rather than attempted optimistically per call. At
/// [`SchemaRung::Prompt`] the contract rides in the prompt and the answer is parsed
/// tolerantly — the same bargain `cursor.rs`/`copilot.rs` already ship.
fn apply_schema(body: &mut Value, req: &PromptRequest, ep: &CustomEndpoint) {
    let Some(schema) = &req.schema else {
        return;
    };
    match ep.rung {
        SchemaRung::Strict | SchemaRung::JsonSchema => {
            // strictify() unconditionally: required by OpenAI, harmless to the endpoints
            // that don't need it (measured) — so one shape serves every vendor.
            body["response_format"] = json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "answer",
                    "strict": ep.rung == SchemaRung::Strict,
                    "schema": super::schema::strictify(schema),
                },
            });
        }
        SchemaRung::JsonObject => {
            body["response_format"] = json!({"type": "json_object"});
            append_schema_to_prompt(body, schema);
        }
        // Never probed, or nothing enforced: the prompt is the only lever left.
        SchemaRung::Prompt | SchemaRung::None => append_schema_to_prompt(body, schema),
    }
}

/// Put the JSON contract in the user message — the fallback `cursor.rs` and `copilot.rs`
/// use, reusing their exact instruction so an unenforced answer parses the same way.
fn append_schema_to_prompt(body: &mut Value, schema: &Value) {
    let instruction = super::prompts::schema_instruction(schema);
    if let Some(user) = body["messages"][1]["content"].as_str() {
        let joined = format!("{user}{instruction}");
        body["messages"][1]["content"] = Value::String(joined);
    }
}

/// Turn a non-2xx into the right [`LlmError`] — the rate-limit distinction is load-bearing:
/// `resolver` backs off and falls back on `RateLimited`, but treats `Failed` as a real
/// error. A metered endpoint that 429s must not be hammered — that costs money.
fn classify_error(status: reqwest::StatusCode, detail: &str, endpoint_id: &str) -> LlmError {
    let head: String = detail.chars().take(300).collect();
    tracing::warn!(
        endpoint_id = %endpoint_id,
        status = %status,
        detail = %head,
        "custom provider returned an error"
    );
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return LlmError::RateLimited(if head.is_empty() {
            format!("custom provider rate-limited ({status})")
        } else {
            head
        });
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return LlmError::Failed(format!(
            "custom provider rejected the API key ({status}) - check it in Settings"
        ));
    }
    LlmError::Failed(format!("custom provider {status}: {head}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(rung: SchemaRung) -> CustomEndpoint {
        CustomEndpoint {
            id: "g1".into(),
            base_url: "https://example.test/v1beta/openai".into(),
            model: "gemini-flash-latest".into(),
            api_key: "secret".into(),
            rung,
        }
    }

    fn req_with_schema() -> PromptRequest {
        PromptRequest {
            system: "sys",
            user: "hour report".into(),
            schema: Some(crate::llm::prompts::workstream_schema()),
            max_tokens: 2048,
            label: "test".into(),
        }
    }

    fn body_for(rung: SchemaRung) -> Value {
        let mut body = json!({"messages": [{"role":"system","content":"sys"},
                                           {"role":"user","content":"hour report"}]});
        apply_schema(&mut body, &req_with_schema(), &ep(rung));
        body
    }

    #[test]
    fn base_url_trailing_slash_does_not_double_the_path() {
        let mut e = ep(SchemaRung::Strict);
        e.base_url = "https://example.test/v1/".into();
        assert_eq!(e.chat_url(), "https://example.test/v1/chat/completions");
    }

    /// The strict rung sends the STRICTIFIED schema — the raw one is what OpenAI 400s on.
    #[test]
    fn strict_rung_sends_a_strictified_schema() {
        let body = body_for(SchemaRung::Strict);
        let rf = &body["response_format"];
        assert_eq!(rf["type"], "json_schema");
        assert_eq!(rf["json_schema"]["strict"], json!(true));
        let item = &rf["json_schema"]["schema"]["properties"]["placements"]["items"];
        assert_eq!(
            item["required"],
            json!(["id", "segments", "summary", "title"])
        );
    }

    /// The enforcing-but-not-strict rung still gets the rewrite (harmless where unneeded),
    /// but must not claim `strict` — the flag is what some endpoints reject.
    #[test]
    fn json_schema_rung_rewrites_but_does_not_claim_strict() {
        let body = body_for(SchemaRung::JsonSchema);
        assert_eq!(
            body["response_format"]["json_schema"]["strict"],
            json!(false)
        );
        assert_eq!(body["response_format"]["type"], "json_schema");
    }

    /// Below the enforcing rungs the contract has to reach the model some other way, or the
    /// answer is free-form prose and the fold silently loses the hour.
    #[test]
    fn unenforced_rungs_put_the_contract_in_the_prompt() {
        for rung in [SchemaRung::Prompt, SchemaRung::None] {
            let body = body_for(rung);
            assert!(
                body["response_format"].is_null(),
                "{rung:?} must not ask for a format"
            );
            let user = body["messages"][1]["content"].as_str().unwrap();
            assert!(
                user.starts_with("hour report"),
                "{rung:?} must keep the original prompt"
            );
            assert!(
                user.len() > "hour report".len(),
                "{rung:?} must append the contract"
            );
        }
    }

    /// json_object guarantees JSON but not shape, so it needs BOTH levers.
    #[test]
    fn json_object_rung_asks_for_json_and_describes_the_shape() {
        let body = body_for(SchemaRung::JsonObject);
        assert_eq!(body["response_format"], json!({"type": "json_object"}));
        assert!(body["messages"][1]["content"].as_str().unwrap().len() > "hour report".len());
    }

    /// A schema-less request must not gain a response_format at any rung.
    #[test]
    fn no_schema_means_no_response_format() {
        let mut body = json!({"messages": [{"role":"system","content":"s"},
                                           {"role":"user","content":"u"}]});
        let req = PromptRequest {
            system: "s",
            user: "u".into(),
            schema: None,
            max_tokens: 512,
            label: "t".into(),
        };
        apply_schema(&mut body, &req, &ep(SchemaRung::Strict));
        assert!(body["response_format"].is_null());
        assert_eq!(body["messages"][1]["content"], "u");
    }

    /// 429 must be `RateLimited`, not `Failed` — the resolver backs off on one and retries
    /// the other, and retrying a metered endpoint that is rate-limiting costs real money.
    #[test]
    fn rate_limit_is_classified_apart_from_a_plain_failure() {
        let e = classify_error(reqwest::StatusCode::TOO_MANY_REQUESTS, "slow down", "g1");
        assert!(matches!(e, LlmError::RateLimited(_)));
        assert!(e.is_rate_limited());

        let e = classify_error(reqwest::StatusCode::BAD_REQUEST, "bad schema", "g1");
        assert!(matches!(e, LlmError::Failed(_)));
        assert!(!e.is_rate_limited());
    }

    /// A bad key is the most common setup failure — it must name itself, not read as an
    /// outage the user should wait out.
    #[test]
    fn an_auth_failure_names_the_key() {
        let e = classify_error(reqwest::StatusCode::UNAUTHORIZED, "invalid key", "g1");
        match e {
            LlmError::Failed(m) => assert!(m.contains("API key"), "{m}"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    /// Selected-but-unconfigured must be a loud, actionable error — the one failure mode
    /// this backend's infallible construction defers to call time.
    #[tokio::test]
    async fn an_unconfigured_custom_provider_fails_with_an_actionable_error() {
        let cfg = LlmConfig {
            model: String::new(),
            meridian_home: std::path::PathBuf::from("/tmp"),
            cli_timeout_s: 5,
            local_timeout_s: 5,
            mlx_host: "127.0.0.1".into(),
            mlx_port: 7823,
            custom: None,
        };
        let err = OpenAiCompatBackend { cfg }
            .complete(&req_with_schema())
            .await
            .expect_err("an unconfigured endpoint cannot answer");
        match err {
            LlmError::Failed(m) => {
                assert!(m.contains("not configured"), "{m}");
                // Must not read as a rate limit, or the resolver would back off and retry.
                assert!(!m.contains("rate"), "{m}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    /// The only test that proves this backend can actually TALK to an OpenAI-compatible
    /// endpoint — the unit tests above prove the request's shape, not that a real vendor
    /// accepts it, parses back, or reports tokens.
    ///
    /// Ignored by default: it spends one real, metered request. Run it against any endpoint:
    ///
    /// ```text
    /// MERIDIAN_TEST_LLM_BASE=https://generativelanguage.googleapis.com/v1beta/openai \
    /// MERIDIAN_TEST_LLM_MODEL=gemini-flash-latest \
    /// MERIDIAN_TEST_LLM_KEY=<key> \
    ///   cargo test --lib llm::openai_compat -- --ignored --nocapture
    /// ```
    ///
    /// It sends the REAL `workstream_schema` at the strict rung, because a toy schema is
    /// exactly what hides a strict-mode rejection (measured: `{"answer":"string"}` passes
    /// on an endpoint where the real schema 400s).
    #[tokio::test]
    #[ignore = "spends one real metered request; needs MERIDIAN_TEST_LLM_{BASE,MODEL,KEY}"]
    async fn live_endpoint_answers_the_real_workstream_schema() {
        let (Ok(base), Ok(model), Ok(key)) = (
            std::env::var("MERIDIAN_TEST_LLM_BASE"),
            std::env::var("MERIDIAN_TEST_LLM_MODEL"),
            std::env::var("MERIDIAN_TEST_LLM_KEY"),
        ) else {
            panic!("set MERIDIAN_TEST_LLM_{{BASE,MODEL,KEY}} — see this test's docs");
        };

        let cfg = LlmConfig {
            model: String::new(),
            meridian_home: std::path::PathBuf::from("/tmp"),
            cli_timeout_s: 90,
            local_timeout_s: 90,
            mlx_host: "127.0.0.1".into(),
            mlx_port: 7823,
            custom: Some(CustomEndpoint {
                id: "live".into(),
                base_url: base,
                model,
                api_key: key,
                rung: SchemaRung::Strict,
            }),
        };

        let req = PromptRequest {
            system: crate::llm::prompts::WORKSTREAM,
            user: "=== CURRENT TASKS ===\n{\"tasks\":[]}\n\n\
                   === NEW ACTIVITY - HOUR 2026-07-17T09 (place this hour's work only) ===\n\
                   09:40-09:58  16 min  Synced the working branch and started the dev \
                   environment, hit a database upgrade conflict that crashed the app, applied \
                   the missing change and got it running cleanly again."
                .into(),
            schema: Some(crate::llm::prompts::workstream_schema()),
            max_tokens: 2048,
            label: "live workstream".into(),
        };

        let out = OpenAiCompatBackend { cfg }
            .complete(&req)
            .await
            .expect("the live endpoint should answer");
        println!("live answer: {}", out.text);

        // It must parse through the PRODUCTION reader, not just be valid JSON — that is the
        // contract the fold depends on.
        let placements = crate::worklog_pipeline::workstream_parse::parse_placements(&out.text)
            .expect("the answer must parse as placements");
        assert!(
            !placements.is_empty(),
            "an hour of work should place somewhere"
        );
        assert!(
            placements.iter().any(|p| !p.segments.is_empty()),
            "a placement must carry the hour's time, or the day loses it"
        );
        assert!(out.elapsed_s > 0.0);
    }
}
