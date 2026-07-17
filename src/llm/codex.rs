//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Codex backend — `codex exec`, on the user's own ChatGPT subscription.
//!
//! Structured output is real: `--output-schema` is validated, and the final message is
//! written to a file (`-o`) rather than stdout, so we read it from there. Both the schema
//! and the output file live in a temp dir removed on drop, even on a panic or timeout.
//!
//! `-s read-only` + `--skip-git-repo-check` + `--ephemeral`: this is a summarisation call,
//! not an agent session. It must not touch the filesystem or leave a rollout behind.
//!
//! The shared schema is rewritten to OpenAI's strict dialect on the way out — see
//! [`strictify`], without which no schema-bearing call reaches the model at all.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use serde_json::Value;

use crate::coding_agent_session_ingest::summariser::prompts as sp;
use crate::coding_agent_session_ingest::summariser::run_capture;

use super::{LlmBackend, LlmConfig, LlmError, LlmOutput, LlmProvider, PromptRequest};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Removes the temp dir on drop — including when the call times out or panics.
struct TempDirGuard(PathBuf);
impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Rewrite a shared schema into OpenAI's **strict** dialect, which `--output-schema`
/// enforces: every key in an object's `properties` must also appear in `required`,
/// or the request is rejected with a 400 (`invalid_json_schema`) *before the model
/// runs* — costing a wasted round trip and, until the error extraction below, an
/// unreadable failure.
///
/// Meridian's schemas ([`crate::llm::prompts`]) are provider-agnostic and lean on
/// optional keys — a placement omits `id` to mean "new task" — which Claude's tool
/// use and the local model's guided generation both accept. So the strictness is
/// applied HERE, on the way out to Codex only, rather than by bending the shared
/// schemas to one provider's dialect (they also feed the production-default local
/// backend, which has no such requirement).
///
/// **Meaning is preserved, not forced**: a key that was optional becomes required
/// but *nullable* (`"string"` → `["string","null"]`), so the model can still answer
/// "absent" by emitting `null`. Every reader of these answers already treats a null
/// exactly as a missing key (`workstream_parse::str_field` and `parse_segments` both
/// fall back to empty), so nothing downstream sees a new shape.
///
/// `maxItems` and friends are left alone — the API tolerates them (verified against
/// codex-cli 0.141.0 / gpt-5.5); only `required` completeness is enforced.
fn strictify(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut out: serde_json::Map<String, Value> = map
                .iter()
                .map(|(k, val)| (k.clone(), strictify(val)))
                .collect();

            // Only an object node with `properties` carries the rule; `properties`'
            // own children are schemas in their own right and were handled above.
            let Some(keys) = out
                .get("properties")
                .and_then(Value::as_object)
                .map(|p| p.keys().cloned().collect::<Vec<_>>())
            else {
                return Value::Object(out);
            };

            let required: Vec<String> = out
                .get("required")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();

            // A key the schema didn't require kept its "absent" meaning in the answer;
            // it can only keep it under strict mode by being nullable.
            if let Some(props) = out.get_mut("properties").and_then(Value::as_object_mut) {
                for k in keys.iter().filter(|k| !required.contains(k)) {
                    if let Some(p) = props.get_mut(k) {
                        make_nullable(p);
                    }
                }
            }
            out.insert(
                "required".into(),
                Value::Array(keys.into_iter().map(Value::String).collect()),
            );
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(strictify).collect()),
        _ => v.clone(),
    }
}

/// Widen a schema node's `type` to admit `null`. A node with no `type` (a `$ref`,
/// a composed `anyOf`) is left untouched — guessing at its shape would be worse
/// than leaving a schema the API will judge for itself.
fn make_nullable(node: &mut Value) {
    let Some(obj) = node.as_object_mut() else {
        return;
    };
    match obj.get("type") {
        Some(Value::String(t)) => {
            let t = t.clone();
            obj.insert("type".into(), serde_json::json!([t, "null"]));
        }
        Some(Value::Array(types)) => {
            if !types.iter().any(|t| t == "null") {
                let mut types = types.clone();
                types.push(Value::String("null".into()));
                obj.insert("type".into(), Value::Array(types));
            }
        }
        _ => {}
    }
}

/// The real reason a `codex exec` call failed, out of its stderr.
///
/// Codex writes an informational banner first — `Reading additional input from
/// stdin...`, the model/sandbox header — so the FIRST line is never the error, and
/// reporting it (as this backend once did) surfaced the stdin notice as the cause of
/// every failure, which is how a 400 `invalid_json_schema` masqueraded as a stdin
/// problem. The real failure is an `ERROR:` line followed by the API's JSON body, so
/// prefer that body's `message`; fall back to the raw `ERROR:` text, and only then to
/// the first line (a crash with no ERROR block at all).
fn codex_error(stderr: &str) -> String {
    let Some(rest) = stderr.split("ERROR:").nth(1) else {
        return sp::first_line(stderr);
    };
    // The body is pretty-printed JSON, so it spans lines — let serde find where it ends
    // rather than guessing, and fall back to the text if it isn't JSON at all.
    if let Some(Ok(v)) = serde_json::Deserializer::from_str(rest.trim())
        .into_iter::<Value>()
        .next()
    {
        if let Some(msg) = v
            .pointer("/error/message")
            .or_else(|| v.pointer("/message"))
            .and_then(Value::as_str)
        {
            return msg.chars().take(300).collect();
        }
    }
    sp::first_line(rest)
}

pub struct CodexBackend {
    pub cfg: LlmConfig,
}

#[async_trait]
impl LlmBackend for CodexBackend {
    fn provider(&self) -> LlmProvider {
        LlmProvider::Codex
    }

    async fn complete(&self, req: &PromptRequest) -> Result<LlmOutput, LlmError> {
        let t0 = std::time::Instant::now();

        let td = std::env::temp_dir().join(format!(
            "meridian-llm-codex-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&td)
            .map_err(|e| LlmError::Failed(format!("codex: temp dir: {e}")))?;
        let _guard = TempDirGuard(td.clone());

        let out_path = td.join("last_message.txt");
        let mut args: Vec<String> = vec![
            "exec".into(),
            req.system.to_string(),
            "-s".into(),
            "read-only".into(),
            "--skip-git-repo-check".into(),
            "--ephemeral".into(),
            // Privacy: disable Codex's usage/analytics collection for this call. This is
            // telemetry only — opting your prompts out of model *training* is the ChatGPT
            // account's "Improve the model for everyone" setting (Data Controls), which a
            // subprocess cannot toggle. See super::DO_NOT_TRACK.
            "--config".into(),
            "analytics.enabled=false".into(),
            "-o".into(),
            out_path.display().to_string(),
            "-C".into(),
            self.cfg.meridian_home.display().to_string(),
        ];
        if let Some(schema) = &req.schema {
            let schema_path = td.join("schema.json");
            std::fs::write(&schema_path, strictify(schema).to_string())
                .map_err(|e| LlmError::Failed(format!("codex: write schema: {e}")))?;
            args.push("--output-schema".into());
            args.push(schema_path.display().to_string());
        }
        if !self.cfg.model.is_empty() {
            args.push("-m".into());
            args.push(self.cfg.model.clone());
        }

        let cap = run_capture(
            "codex",
            &args,
            &req.user, // codex exec reads the input from stdin
            &self.cfg.meridian_home,
            self.cfg.cli_timeout_s,
            &[("MERIDIAN_SUMMARISER", "1"), super::DO_NOT_TRACK],
            &[],
        )
        .await
        .map_err(super::resolver::from_summariser_error)?;

        if !cap.success {
            let blob = format!("{}\n{}", cap.stderr, cap.stdout);
            if sp::looks_rate_limited(&blob) {
                let msg =
                    sp::rate_limited_line(&blob).unwrap_or_else(|| sp::first_line(&cap.stderr));
                return Err(LlmError::RateLimited(if msg.is_empty() {
                    "rate/usage limit".into()
                } else {
                    msg
                }));
            }
            // The summary line is bounded, so keep the whole of stderr on the span: a
            // rejected request answers WHY only in the API's JSON body, and that body is
            // the one thing worth having when this fails.
            tracing::error!(
                code = ?cap.code,
                stderr = %cap.stderr,
                "codex exec failed"
            );
            return Err(LlmError::Failed(format!(
                "codex exited {:?}: {}",
                cap.code,
                codex_error(&cap.stderr)
            )));
        }

        let text = std::fs::read_to_string(&out_path)
            .map_err(|e| LlmError::Failed(format!("codex: no output file ({e})")))?;
        if text.trim().is_empty() {
            return Err(LlmError::Failed("codex returned an empty answer".into()));
        }

        Ok(LlmOutput {
            text: text.trim().to_string(),
            input_tokens: 0,
            output_tokens: 0,
            elapsed_s: t0.elapsed().as_secs_f64(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Every key of every object node must be required — the rule the API rejected the
    /// real workstream schema over. Walked recursively so a nested `items` object (where
    /// the live 400 actually pointed) is covered, not just the root.
    fn assert_strict(v: &Value) {
        if let Some(props) = v.get("properties").and_then(Value::as_object) {
            let required: Vec<&str> = v["required"]
                .as_array()
                .expect("an object with properties must carry required")
                .iter()
                .map(|x| x.as_str().unwrap())
                .collect();
            for k in props.keys() {
                assert!(
                    required.contains(&k.as_str()),
                    "key {k} missing from required"
                );
            }
        }
        match v {
            Value::Object(m) => m.values().for_each(assert_strict),
            Value::Array(a) => a.iter().for_each(assert_strict),
            _ => {}
        }
    }

    /// The live failure: `placements.items` required only `summary`/`segments`, so the
    /// API rejected the call before the model ran ("Missing 'id'").
    #[test]
    fn strictify_requires_every_key_of_the_real_workstream_schema() {
        let out = strictify(&crate::llm::prompts::workstream_schema());
        assert_strict(&out);
        // Sorted, because serde_json keys a Map by BTreeMap — `required` is a set to the
        // API, so the order is not part of the contract.
        let item = &out["properties"]["placements"]["items"];
        assert_eq!(
            item["required"],
            json!(["id", "segments", "summary", "title"])
        );
    }

    /// Optional keys must stay expressible as "absent", which under strict mode is a
    /// `null` — the parsers read null and missing identically.
    #[test]
    fn strictify_makes_formerly_optional_keys_nullable() {
        let out = strictify(&crate::llm::prompts::workstream_schema());
        let props = &out["properties"]["placements"]["items"]["properties"];
        assert_eq!(props["id"]["type"], json!(["string", "null"]));
        assert_eq!(props["title"]["type"], json!(["string", "null"]));
        // Already-required keys keep their exact type — nothing gains a null it didn't need.
        assert_eq!(props["summary"]["type"], json!("array"));
        assert_eq!(props["segments"]["type"], json!("array"));
    }

    /// Unrelated keywords survive: `maxItems` is tolerated by the API, and the 6-bullet
    /// cap is a contract the prompt relies on.
    #[test]
    fn strictify_preserves_other_keywords() {
        let out = strictify(&crate::llm::prompts::workstream_schema());
        assert_eq!(
            out["properties"]["placements"]["items"]["properties"]["summary"]["maxItems"],
            json!(6)
        );
        assert_eq!(
            out["properties"]["placements"]["items"]["additionalProperties"],
            json!(false)
        );
    }

    /// An already-nullable branch key must not collect a second "null".
    #[test]
    fn strictify_does_not_double_null_a_union_type() {
        let out = strictify(&crate::llm::prompts::worklog_generate_schema());
        assert_strict(&out);
        assert_eq!(
            out["properties"]["propose"]["type"],
            json!(["object", "null"])
        );
    }

    /// The other shared schemas must survive the same walk — Codex can carry any of them.
    #[test]
    fn strictify_makes_every_shared_schema_strict() {
        for s in [
            crate::llm::prompts::activity_report_schema(),
            crate::llm::prompts::worklog_generate_schema(),
            crate::llm::prompts::plan_task_draft_schema(),
        ] {
            assert_strict(&strictify(&s));
        }
    }

    /// Verbatim stderr from the live failure (codex-cli 0.141.0). The banner's first line
    /// is what this backend used to report as the cause.
    const REAL_STDERR: &str = "Reading additional input from stdin...\nOpenAI Codex v0.141.0\n--------\nworkdir: /Users/x/.meridian\nmodel: gpt-5.5\n--------\nERROR: {\n  \"type\": \"error\",\n  \"error\": {\n    \"type\": \"invalid_request_error\",\n    \"code\": \"invalid_json_schema\",\n    \"message\": \"Invalid schema for response_format 'codex_output_schema': In context=('properties', 'placements', 'items'), 'required' is required to be supplied and to be an array including every key in properties. Missing 'id'.\",\n    \"param\": \"text.format.schema\"\n  },\n  \"status\": 400\n}\n";

    #[test]
    fn codex_error_reports_the_api_message_not_the_stdin_banner() {
        let msg = codex_error(REAL_STDERR);
        assert!(
            msg.starts_with("Invalid schema for response_format"),
            "got: {msg}"
        );
        assert!(
            !msg.contains("stdin"),
            "the banner must never be the reported cause: {msg}"
        );
    }

    /// No ERROR block (a crash / a usage message) still has to say something.
    #[test]
    fn codex_error_falls_back_to_the_first_line() {
        assert_eq!(
            codex_error("codex: command failed\n\n"),
            "codex: command failed"
        );
        assert_eq!(codex_error(""), "");
    }

    /// An ERROR block that isn't JSON (a panic, a plain-text failure) reports its text.
    #[test]
    fn codex_error_handles_a_non_json_error_block() {
        assert_eq!(
            codex_error("banner\nERROR: stream disconnected\n"),
            "stream disconnected"
        );
    }
}
