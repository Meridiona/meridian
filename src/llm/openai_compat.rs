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
//! the standard OpenAI chat-completions protocol; the deltas from a bare call are the
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
    /// Requests-per-minute ceiling, `0` = unpaced. Carried down from the registry row so
    /// [`super::resolver`] can pace WITHOUT re-reading settings on every call.
    pub rpm: u32,
    /// What this endpoint was measured to honour for the schema being sent.
    pub rung: SchemaRung,
}

impl CustomEndpoint {
    /// `<base_url>/chat/completions`, tolerating a trailing slash on the stored base.
    fn chat_url(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }
}

/// Pull the model ids out of a `/models` response body.
///
/// Split out from [`list_models`] so the envelope handling is unit-testable without a live
/// endpoint — the shapes below are the whole reason this function is fallible.
///
/// OpenAI answers `{"data":[{"id":…}]}`. Some compatible servers answer `{"models":[…]}`, and
/// a few return a bare array; entries are usually objects but are sometimes bare strings. All
/// are accepted, because the alternative is making a user hand-type a model purely because
/// their vendor picked a different envelope.
///
/// Returns an empty vec — NOT an error — for a well-formed response listing nothing. That is
/// a real answer ("this endpoint serves no models it will admit to"), and callers already
/// treat empty as "fall back to free text".
fn parse_models_body(body: &str) -> Result<Vec<String>, LlmError> {
    let parsed: Value = serde_json::from_str(body)
        .map_err(|e| LlmError::Failed(format!("custom provider models response: {e}")))?;

    let items = parsed
        .get("data")
        .or_else(|| parsed.get("models"))
        .unwrap_or(&parsed);
    // A non-array envelope is a FAILURE, not an empty list. Some endpoints answer 200 with
    // an error object (`{"error":"invalid key"}`); treating that as "listed no models" would
    // report a working endpoint serving nothing and hide the real reason from the user.
    let items = items.as_array().ok_or_else(|| {
        LlmError::Failed("custom provider models response was not a list".to_string())
    })?;
    let mut ids: Vec<String> = items
        .iter()
        .filter_map(|m| {
            // An entry is either {"id": "…"} or, on some servers, a bare string.
            m.get("id")
                .and_then(Value::as_str)
                .or_else(|| m.as_str())
                .map(str::to_string)
        })
        .filter(|s| !s.is_empty())
        .collect();
    // Sorted and deduped so the picker's order doesn't depend on the server's, and a vendor
    // that lists the same id twice doesn't render it twice.
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// How long to wait on a models listing. Deliberately short and NOT `cli_timeout_s`: this
/// runs while a user watches a dropdown, and a slow endpoint should fall back to free text
/// quickly rather than freeze the picker.
const MODELS_TIMEOUT_S: u64 = 10;

/// Ceiling on a `/models` body. Generous for the use case — the longest real listing is a
/// few hundred entries, well under 100 KB — while still bounded.
const MODELS_MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Read a response body with a hard byte ceiling.
///
/// `resp.text()` buffers without limit, and the URL here is **user-supplied**: a faulty or
/// hostile endpoint could stream indefinitely and exhaust tray memory, which the request
/// timeout does not prevent (a steady trickle never times out). Chunks are accumulated and
/// the read is abandoned the moment it exceeds the cap.
async fn read_capped_body(mut resp: reqwest::Response) -> Result<String, LlmError> {
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| LlmError::Failed(format!("custom provider models response: {e}")))?
    {
        if buf.len() + chunk.len() > MODELS_MAX_BODY_BYTES {
            return Err(LlmError::Failed(format!(
                "custom provider models response exceeded {} KB",
                MODELS_MAX_BODY_BYTES / 1024
            )));
        }
        buf.extend_from_slice(&chunk);
    }
    // Lossy rather than strict: a body that is almost-JSON with one bad byte should reach
    // the parser and fail there with a useful message, not die as an encoding error.
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Ask an OpenAI-compatible endpoint what models it serves — `GET <base_url>/models`.
///
/// Takes the base URL and key rather than a [`CustomEndpoint`] because the main caller is the
/// ADD-endpoint form, where no model has been chosen yet and so no endpoint exists to build.
///
/// # Why `<base_url>/models` and not `<base_url>/v1/models`
///
/// The stored `base_url` already carries the version segment (`https://…/v1`,
/// `https://…/v1beta/openai`) — the same reason [`CustomEndpoint::chat_url`] appends a bare
/// `/chat/completions`. Appending `/v1` here would 404 every configured endpoint.
///
/// # Who calls this
///
/// [`crate::llm`] exposes it to the tray's `list_custom_llm_provider_models` command, which
/// serves the custom-endpoint model picker.
///
/// # Errors
///
/// Reuses [`classify_error`], so a 429 comes back as [`LlmError::RateLimited`] and a 401/403
/// as a key-specific message. Callers are expected to DEGRADE on any error — every field this
/// populates must stay usable as free text, since plenty of OpenAI-compatible servers don't
/// implement `/models` at all.
pub async fn list_models(
    base_url: &str,
    api_key: &str,
    endpoint_id: &str,
) -> Result<Vec<String>, LlmError> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(MODELS_TIMEOUT_S))
        // Same reasoning as the chat call: a 3xx to another origin would forward the
        // Authorization header (and the key) to a host the user never configured.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| LlmError::Failed(format!("custom provider client: {e}")))?;

    let resp = client
        .get(&url)
        .bearer_auth(api_key)
        .send()
        .await
        // As in `complete`: `e` can carry the URL but never the key (reqwest redacts auth).
        .map_err(|e| LlmError::Failed(format!("custom provider models request failed: {e}")))?;

    let status = resp.status();
    let headers = resp.headers().clone();
    let body = read_capped_body(resp).await?;
    if !status.is_success() {
        return Err(classify_error(status, &headers, &body, endpoint_id));
    }

    let ids = parse_models_body(&body)?;

    tracing::info!(
        endpoint_id = %endpoint_id,
        models = ids.len(),
        "custom provider: listed models"
    );
    Ok(ids)
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
        apply_reasoning_effort(&mut body, &ep.model);
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
            // The Bearer key goes to a user-supplied host, so never follow a redirect:
            // a 3xx to another origin would forward the Authorization header (and the
            // key) somewhere the user never configured.
            .redirect(reqwest::redirect::Policy::none())
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
            // Headers must be lifted BEFORE `text()`, which consumes the response. They carry
            // the only machine-readable reset signal a metered endpoint gives us.
            let headers = resp.headers().clone();
            let detail = resp.text().await.unwrap_or_default();
            return Err(classify_error(status, &headers, &detail, &ep.id));
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
            // WHY it was empty, when the endpoint told us. A REASONING model charges its
            // hidden reasoning tokens against the same completion budget as the visible
            // answer, so a budget that is merely small comes back `finish_reason: "length"`
            // with no content at all - and the bare "returned an empty answer" then reads as
            // "your key is broken" over a key that answered perfectly. (Measured: Groq's
            // gpt-oss-120b spends 14 reasoning tokens before writing a single character of
            // "OK"; see `detect::PROBE_MAX_TOKENS`.)
            let truncated = payload["choices"][0]["finish_reason"].as_str() == Some("length");
            tracing::warn!(
                endpoint_id = %ep.id,
                truncated,
                "custom provider returned no content"
            );
            return Err(LlmError::Failed(if truncated {
                "custom provider used its whole token budget before answering - \
                 the model is likely a reasoning model that needs a larger budget"
                    .into()
            } else {
                "custom provider returned an empty answer".to_string()
            }));
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

/// Cap a gpt-oss (Harmony template) model's hidden reasoning budget.
///
/// gpt-oss spends its hidden reasoning against the same completion budget as the visible
/// answer, so it can burn `max_tokens` entirely on reasoning and return empty `content` with
/// `finish_reason: "length"` (see the empty-content branch in [`OpenAiCompatBackend::complete`]
/// — measured on both Groq's and Ollama's gpt-oss-120b/20b). `reasoning_effort` is the
/// model's own harmony parameter for capping that hidden budget; `"low"` was verified live
/// against Ollama's Cloud API to eliminate the empty answer without a quality regression on
/// trivial prompts. Gated on the MODEL name, not the vendor, because the failure is a
/// property of the gpt-oss template itself, not of any one endpoint.
fn apply_reasoning_effort(body: &mut Value, model: &str) {
    if model.contains("gpt-oss") {
        body["reasoning_effort"] = json!("low");
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

/// How long a 429 says to wait, read from the response headers — the only machine-readable
/// reset signal a metered endpoint offers, and strictly better than guessing.
///
/// Three sources, most-authoritative first:
/// 1. `Retry-After` — the RFC-9110 standard. Either delta-seconds (`23`) or an HTTP-date
///    (`Wed, 21 Oct 2026 07:28:00 GMT`); both are spec-legal and both are seen in the wild,
///    so both are handled. A date in the past clamps to zero rather than underflowing.
/// 2. `x-ratelimit-reset-requests` — the OpenAI-compatible convention for the REQUEST
///    window, e.g. `1s`, `6m0s`, `500ms`.
/// 3. `x-ratelimit-reset-tokens` — the same for the TOKEN window. Read even though we do not
///    yet PACE tokens: when a 429 was TPM-caused rather than RPM-caused, this is the only
///    header that reports the right window, so honouring it closes the TPM gap reactively
///    for free. See `CustomLlmProvider::rpm`.
///
/// `None` means the endpoint told us nothing usable; the caller falls back to the message
/// text and then to a flat default.
fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let get = |k: &str| headers.get(k).and_then(|v| v.to_str().ok()).map(str::trim);

    if let Some(raw) = get("retry-after") {
        if let Ok(secs) = raw.parse::<u64>() {
            return Some(Duration::from_secs(secs));
        }
        // HTTP-date form. Past dates clamp to zero — `to_std` errors on a negative span.
        if let Ok(when) = chrono::DateTime::parse_from_rfc2822(raw) {
            let delta = when.with_timezone(&chrono::Utc) - chrono::Utc::now();
            return Some(delta.to_std().unwrap_or(Duration::ZERO));
        }
    }

    get("x-ratelimit-reset-requests")
        .and_then(parse_duration_suffix)
        .or_else(|| get("x-ratelimit-reset-tokens").and_then(parse_duration_suffix))
}

/// Parse the `6m0s` / `1.5s` / `500ms` duration form these headers use.
///
/// Deliberately NOT [`super::reset_time::parse_backoff`]: that reads English prose off a CLI
/// subprocess's stderr and has no seconds unit, because no CLI emits one. This grammar is
/// machine-generated and sub-minute. Two channels, two parsers, on purpose.
fn parse_duration_suffix(s: &str) -> Option<Duration> {
    let (mut total, mut num) = (0f64, String::new());
    let mut chars = s.chars().peekable();
    let mut saw_unit = false;
    while let Some(c) = chars.next() {
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
            continue;
        }
        let value: f64 = num.parse().ok()?;
        num.clear();
        // `ms` must be tested before `m`, or milliseconds parse as minutes — a 500ms wait
        // read as 500 minutes would park a working endpoint for eight hours.
        let mult = match c {
            'm' if chars.peek() == Some(&'s') => {
                chars.next();
                0.001
            }
            'h' => 3600.0,
            'm' => 60.0,
            's' => 1.0,
            _ => return None,
        };
        total += value * mult;
        saw_unit = true;
    }
    // A bare number with no unit is ambiguous; refuse rather than guess at the scale.
    (saw_unit && total > 0.0).then(|| Duration::from_secs_f64(total))
}

/// Turn a non-2xx into the right [`LlmError`] — the rate-limit distinction is load-bearing:
/// `resolver` backs off and falls back on `RateLimited`, but treats `Failed` as a real
/// error. A metered endpoint that 429s must not be hammered — that costs money.
fn classify_error(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    detail: &str,
    endpoint_id: &str,
) -> LlmError {
    let head: String = detail.chars().take(300).collect();
    tracing::warn!(
        endpoint_id = %endpoint_id,
        status = %status,
        detail = %head,
        "custom provider returned an error"
    );
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry_after = parse_retry_after(headers);
        tracing::warn!(
            endpoint_id = %endpoint_id,
            retry_after_s = retry_after.map(|d| d.as_secs()),
            "custom provider rate-limited"
        );
        return LlmError::RateLimited {
            message: if head.is_empty() {
                format!("custom provider rate-limited ({status})")
            } else {
                head
            },
            retry_after,
        };
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return LlmError::Failed(format!(
            "custom provider rejected the API key ({status}) - check it in Settings"
        ));
    }
    LlmError::Failed(format!("custom provider {status}: {head}"))
}

#[cfg(test)]
mod models_listing_tests {
    use super::*;

    /// The OpenAI shape, which Groq/OpenRouter/Gemini's compat layer all follow.
    #[test]
    fn reads_the_openai_data_envelope() {
        let body = r#"{"object":"list","data":[
            {"id":"gpt-5.1","object":"model"},
            {"id":"gpt-5.5","object":"model"}
        ]}"#;
        assert_eq!(parse_models_body(body).unwrap(), vec!["gpt-5.1", "gpt-5.5"]);
    }

    /// Some compatible servers use `models` instead of `data`.
    #[test]
    fn reads_the_models_envelope() {
        let body = r#"{"models":[{"id":"llama-3.3-70b"}]}"#;
        assert_eq!(parse_models_body(body).unwrap(), vec!["llama-3.3-70b"]);
    }

    /// …and a few return the array itself, sometimes as bare strings.
    #[test]
    fn reads_a_bare_array_of_objects_or_strings() {
        assert_eq!(
            parse_models_body(r#"[{"id":"a"},"b"]"#).unwrap(),
            vec!["a", "b"]
        );
    }

    /// Order comes from us, not the server, and a duplicate id renders once.
    #[test]
    fn sorts_and_dedupes() {
        let body = r#"{"data":[{"id":"z"},{"id":"a"},{"id":"z"}]}"#;
        assert_eq!(parse_models_body(body).unwrap(), vec!["a", "z"]);
    }

    /// A well-formed response listing nothing is an ANSWER, not a failure - the caller
    /// falls back to free text either way, but an Err would surface a scary message for
    /// what is simply an endpoint with nothing to declare.
    #[test]
    fn empty_list_is_ok_not_an_error() {
        assert_eq!(
            parse_models_body(r#"{"data":[]}"#).unwrap(),
            Vec::<String>::new()
        );
    }

    /// Entries we can't read a usable id from are skipped rather than poisoning the list
    /// with empty options.
    #[test]
    fn skips_entries_with_no_usable_id() {
        let body = r#"{"data":[{"id":"good"},{"object":"model"},{"id":""}]}"#;
        assert_eq!(parse_models_body(body).unwrap(), vec!["good"]);
    }

    /// A non-JSON body (an HTML error page from a proxy, say) must fail rather than be
    /// reported as "no models", which would look like a working endpoint serving nothing.
    #[test]
    fn rejects_a_non_json_body() {
        assert!(parse_models_body("<html>502 Bad Gateway</html>").is_err());
    }

    /// Valid JSON that isn't a list must fail for the same reason. Some endpoints answer
    /// **200** with an error object, and reporting that as "no models" would blame the
    /// endpoint for serving nothing while hiding the actual cause (a bad key, usually).
    #[test]
    fn rejects_a_non_array_envelope() {
        assert!(parse_models_body(r#"{"error":"invalid key"}"#).is_err());
        assert!(parse_models_body(r#"{"data":{"id":"a"}}"#).is_err());
    }
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
            rpm: 0,
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
            interactive: false,
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

    #[test]
    fn reasoning_effort_is_capped_for_gpt_oss_models_only() {
        for model in [
            "gpt-oss:20b",
            "gpt-oss:20b-cloud",
            "gpt-oss-120b",
            "openai/gpt-oss-120b",
        ] {
            let mut body = json!({});
            apply_reasoning_effort(&mut body, model);
            assert_eq!(body["reasoning_effort"], json!("low"), "model {model}");
        }
        for model in ["gemini-flash-latest", "llama-3.3-70b", ""] {
            let mut body = json!({});
            apply_reasoning_effort(&mut body, model);
            assert!(body["reasoning_effort"].is_null(), "model {model}");
        }
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
            interactive: false,
        };
        apply_schema(&mut body, &req, &ep(SchemaRung::Strict));
        assert!(body["response_format"].is_null());
        assert_eq!(body["messages"][1]["content"], "u");
    }

    /// 429 must be `RateLimited`, not `Failed` — the resolver backs off on one and retries
    /// the other, and retrying a metered endpoint that is rate-limiting costs real money.
    #[test]
    fn rate_limit_is_classified_apart_from_a_plain_failure() {
        let e = classify_error(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &Default::default(),
            "slow down",
            "g1",
        );
        assert!(matches!(e, LlmError::RateLimited { .. }));
        assert!(e.is_rate_limited());

        let e = classify_error(
            reqwest::StatusCode::BAD_REQUEST,
            &Default::default(),
            "bad schema",
            "g1",
        );
        assert!(matches!(e, LlmError::Failed(_)));
        assert!(!e.is_rate_limited());
    }

    fn headers(pairs: &[(&str, &str)]) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                reqwest::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    /// The common case: `Retry-After` as plain delta-seconds.
    #[test]
    fn retry_after_seconds_is_read() {
        let h = headers(&[("retry-after", "23")]);
        assert_eq!(parse_retry_after(&h), Some(Duration::from_secs(23)));
    }

    /// `Retry-After` is equally legal as an HTTP-date, and real providers send it. A date
    /// already in the past must clamp to zero rather than underflow into an absurd wait.
    #[test]
    fn retry_after_http_date_is_read_and_past_dates_clamp() {
        let future = (chrono::Utc::now() + chrono::Duration::seconds(120)).to_rfc2822();
        let got = parse_retry_after(&headers(&[("retry-after", &future)])).unwrap();
        assert!(
            got > Duration::from_secs(90) && got <= Duration::from_secs(120),
            "expected ~120s, got {got:?}"
        );

        let past = (chrono::Utc::now() - chrono::Duration::seconds(300)).to_rfc2822();
        assert_eq!(
            parse_retry_after(&headers(&[("retry-after", &past)])),
            Some(Duration::ZERO)
        );
    }

    /// The OpenAI-compatible reset headers, including the one that would be catastrophic to
    /// misparse: `500ms` read as 500 MINUTES would park a healthy endpoint for eight hours.
    #[test]
    fn ratelimit_reset_suffix_forms_parse() {
        assert_eq!(parse_duration_suffix("1s"), Some(Duration::from_secs(1)));
        assert_eq!(
            parse_duration_suffix("6m0s"),
            Some(Duration::from_secs(360))
        );
        assert_eq!(
            parse_duration_suffix("500ms"),
            Some(Duration::from_millis(500))
        );
        assert_eq!(
            parse_duration_suffix("1h30m"),
            Some(Duration::from_secs(5400))
        );
        // A bare number has no scale — refuse rather than guess.
        assert_eq!(parse_duration_suffix("30"), None);
        assert_eq!(parse_duration_suffix("soon"), None);
    }

    /// `Retry-After` outranks the vendor-specific headers when both are present.
    #[test]
    fn retry_after_wins_over_reset_headers() {
        let h = headers(&[("retry-after", "5"), ("x-ratelimit-reset-requests", "60s")]);
        assert_eq!(parse_retry_after(&h), Some(Duration::from_secs(5)));
    }

    /// TPM is not PACED, but a token-window 429 must still back off for the right duration —
    /// this header is the only one that reports it.
    #[test]
    fn token_reset_is_used_when_it_is_the_only_signal() {
        let h = headers(&[("x-ratelimit-reset-tokens", "45s")]);
        assert_eq!(parse_retry_after(&h), Some(Duration::from_secs(45)));
    }

    /// No usable header means `None`, so the caller falls through to the message text and
    /// then to its flat per-transport default.
    #[test]
    fn absent_headers_yield_no_duration() {
        assert_eq!(parse_retry_after(&Default::default()), None);
        let e = classify_error(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &headers(&[("x-ratelimit-reset-requests", "12s")]),
            "slow down",
            "g1",
        );
        match e {
            LlmError::RateLimited { retry_after, .. } => {
                assert_eq!(retry_after, Some(Duration::from_secs(12)));
            }
            _ => panic!("expected RateLimited"),
        }
    }

    /// A bad key is the most common setup failure — it must name itself, not read as an
    /// outage the user should wait out.
    #[test]
    fn an_auth_failure_names_the_key() {
        let e = classify_error(
            reqwest::StatusCode::UNAUTHORIZED,
            &Default::default(),
            "invalid key",
            "g1",
        );
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
            custom: Some(CustomEndpoint {
                id: "live".into(),
                base_url: base,
                model,
                api_key: key,
                rpm: 0,
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
            interactive: false,
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
