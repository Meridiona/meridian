//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Making a shared schema acceptable to a provider that wants OpenAI's strict dialect.
//!
//! # Why this exists
//! Meridian's schemas ([`crate::llm::prompts`]) are provider-agnostic and lean on optional
//! keys — a placement omits `id` to mean "new task". Claude's tool use and the local
//! model's guided generation accept that; OpenAI does not, and rejects the request with a
//! 400 (`invalid_json_schema`) BEFORE the model runs. So the strictness is applied on the
//! way OUT, per provider, rather than by bending the shared schemas to one vendor's
//! dialect — they also feed the production-default local backend, which needs none of it.
//!
//! # Why it is shared rather than per-backend
//! Both cloud paths need the identical rewrite: `codex exec --output-schema` and the
//! OpenAI-compatible `response_format: json_schema`. Measured (2026-07-17): OpenAI REJECTS
//! the un-rewritten workstream schema ("Missing \'id\'"), while Gemini\'s compat endpoint
//! ACCEPTS both forms — so the rewrite is *required* by one vendor and *harmless* to
//! another, which is what makes one shared pass safe for every OpenAI-compatible endpoint.
//!
//! # Who calls this
//! [`crate::llm::codex`] (writes the schema file for `--output-schema`) and
//! [`crate::llm::openai_compat`] (inlines it into `response_format`).

use serde_json::Value;

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
/// Only `required` completeness is rewritten. `maxItems` and friends are left alone:
/// measured, `codex exec` (0.141.0 / gpt-5.5) and Gemini's compat endpoint both accept
/// `maxItems: 6` inside a strict schema. Note the codex evidence is via a CLI that may
/// sanitize before the API sees it, so if a *direct* OpenAI endpoint ever rejects an
/// unsupported keyword, this is the place to strip it — the probe
/// ([`crate::llm::detect`]) will surface it as a failed rung rather than a broken fold.
pub(crate) fn strictify(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut out: serde_json::Map<String, Value> = map
                .iter()
                .map(|(k, val)| (k.clone(), strictify(val)))
                .collect();

            // Whether THIS node is an object-type schema at all - checked on `type`,
            // not on "does it have a `properties` key". `properties` itself is a bag
            // of field-name -> schema mappings, not a schema node, and recursion walks
            // into it too (see the `.iter().map` above); testing `type` instead of
            // `properties`' mere presence is what keeps this from misfiring on that
            // bag, while still catching a `{"type":"object"}` with no `properties` at
            // all, or a `["object","null"]` union - both of which the old
            // `out.get("properties")`-gated early return skipped past silently,
            // leaving them without `additionalProperties:false` (see below).
            if !declares_object_type(&out) {
                return Value::Object(out);
            }

            let keys = out
                .get("properties")
                .and_then(Value::as_object)
                .map(|p| p.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default();

            if !keys.is_empty() {
                let required: Vec<String> = out
                    .get("required")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();

                // A key the schema didn't require kept its "absent" meaning in the
                // answer; it can only keep it under strict mode by being nullable.
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
            }
            // The other half of OpenAI's strict-mode contract, and the one Meridian's
            // shared schemas didn't all carry by hand: every object-type node needs
            // `additionalProperties: false`, whether or not it declares `properties` -
            // an empty object (`{"type":"object"}`, no `properties` at all) and an
            // `["object","null"]` union both still need it. `placements.items` had it
            // (authored explicitly in prompts.rs) so the existing test above only ever
            // proved it *survives* the rewrite; every OTHER object node in every shared
            // schema silently lacked it. Groq's endpoint validates this directly
            // (unlike `codex exec --output-schema`, which appears to inject it before
            // the API ever sees the request - see this fn's own doc on why a
            // CLI-observed rung can hide a gap a direct API exposes), and rejects the
            // whole request with a 400 before the model runs: "invalid JSON schema for
            // response_format: 'answer': additionalProperties:false must be set on every
            // object" - observed live across dozens of accounts, the single most common
            // Groq-path failure in production telemetry (2026-08-19). Setting it here,
            // unconditionally for every object-type node, closes the gap for every
            // current and future shared schema at the one place that already owns
            // strict-mode compliance, rather than relying on each hand-authored schema
            // to remember it on every nested object.
            out.insert("additionalProperties".into(), Value::Bool(false));
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(strictify).collect()),
        _ => v.clone(),
    }
}

/// Whether a schema node's `type` names (or includes, in a union) `"object"`.
fn declares_object_type(map: &serde_json::Map<String, Value>) -> bool {
    match map.get("type") {
        Some(Value::String(t)) => t == "object",
        Some(Value::Array(types)) => types.iter().any(|t| t.as_str() == Some("object")),
        _ => false,
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
            // The other half of the contract - see this test's own comment for why
            // `strictify_preserves_other_keywords` alone didn't catch this being absent
            // everywhere ELSE it was needed.
            assert_eq!(
                v["additionalProperties"],
                json!(false),
                "an object node with properties is missing additionalProperties:false"
            );
        } else if let Value::Object(m) = v {
            if declares_object_type(m) {
                assert_eq!(
                    v["additionalProperties"],
                    json!(false),
                    "an object-type node WITHOUT properties is missing additionalProperties:false"
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

    /// An object schema with no `properties` at all - previously skipped entirely by the
    /// old `out.get("properties")`-gated early return, so it never picked up
    /// `additionalProperties:false` and Groq's endpoint would 400 on it.
    #[test]
    fn strictify_sets_additional_properties_false_on_an_empty_object() {
        let out = strictify(&json!({"type": "object"}));
        assert_eq!(out["additionalProperties"], json!(false));
    }

    /// Same gap, via the union form real schemas actually use (see `propose` in
    /// `worklog_generate_schema`) — an `["object","null"]` node with no `properties`.
    #[test]
    fn strictify_sets_additional_properties_false_on_an_object_null_union_with_no_properties() {
        let out = strictify(&json!({"type": ["object", "null"]}));
        assert_eq!(out["additionalProperties"], json!(false));
    }

    /// A non-object node (a bare string schema) must never gain
    /// `additionalProperties` - the field means nothing outside an object type.
    #[test]
    fn strictify_never_adds_additional_properties_to_a_non_object_node() {
        let out = strictify(&json!({"type": "string"}));
        assert!(out.get("additionalProperties").is_none());
    }

    /// The `properties` bag itself is a map of field-name -> schema, walked by the same
    /// recursion as every other node - it must never be mistaken for an object-type
    /// schema and gain a spurious `additionalProperties`/`required` key of its own,
    /// which would show up as a bogus field named "additionalProperties" in the schema.
    #[test]
    fn strictify_does_not_stamp_the_properties_bag_itself() {
        let out = strictify(&json!({
            "type": "object",
            "properties": {"x": {"type": "string"}},
            "required": ["x"]
        }));
        assert!(out["properties"].get("additionalProperties").is_none());
        assert!(out["properties"].get("required").is_none());
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
}
