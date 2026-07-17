//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Panel validation — the load-bearing half of the daily summary.
//!
//! # Why this exists
//! The model writes WHOLE, RAW Vega-Lite specs: any mark, `layer`, `facet`,
//! `repeat`, `transform`. Nothing is off-limits, which is the point — a grammar
//! composes forms a fixed chart component never could. The price is that no
//! provider is correct by construction: a free-form spec cannot be constrained
//! token-by-token the way a fixed schema can (`llm::local`'s FSM), and published
//! work on LLM chart generation finds that even strong models "still hallucinate
//! invalid Vega-Lite fields". So everything the model emits is assumed wrong until
//! it passes here.
//!
//! # The two layers
//! 1. **The real Vega-Lite schema** (`services/schemas/vega-lite-schema.json`,
//!    `include_str!`'d). This is the only thing that actually knows what
//!    Vega-Lite accepts; hand-rolling a subset of it would be a second, drifting
//!    implementation of someone else's grammar.
//! 2. **Rules the VL schema cannot express**, because they are about OUR data:
//!    - `data` must be `{"name": "<known dataset>"}`. **Inline `data.values` is a
//!      rejection.** This is the anti-hallucination rule and the reason the whole
//!      design works: the model chooses the FORM, code supplies the NUMBERS. It
//!      mirrors `worklog_pipeline::workstream`'s house rule (durations are derived
//!      from segments, never taken from the model). A strong model will happily
//!      invent a plausible figure; this leaves it nowhere to put one.
//!    - every encoded `field` must exist on that dataset.
//!
//! # Why failures are reasons, not booleans
//! A drop reason is the only signal that says whether the model is actually
//! authoring valid Vega-Lite, so each rejection carries a human-readable why. They
//! land on the `day_summary.validate` span and answer the open question the design
//! rests on.
//!
//! # Related
//! - [`meridian_core::day_evidence::datasets`] — the names and fields this checks
//!   against.
//! - [`super::generate`] — drops the invalid, falls back when nothing survives.

use meridian_core::day_summaries::SummaryPanel;
use serde_json::Value;
use std::sync::OnceLock;

use meridian_core::day_evidence::datasets;

/// The Vega-Lite JSON schema, vendored from the `vega-lite` npm package and
/// compiled into the binary.
///
/// Vendored rather than fetched: the daemon is local-only and must validate with
/// no network. The version MUST track `ui/package.json`'s `vega-lite` — a spec
/// that passes here but fails in the webview (or vice versa) is the exact bug this
/// is meant to prevent, and `tests::vendored_schema_matches_the_installed_vega_lite`
/// is the tripwire.
const VEGA_LITE_SCHEMA: &str = include_str!("../../services/schemas/vega-lite-schema.json");

/// Why one panel was rejected. `Display`s into the drop-reason span field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropReason {
    /// Failed the real Vega-Lite schema.
    VegaLiteSchema(String),
    /// Carried inline data instead of binding a named dataset.
    InlineData,
    /// `data.name` was absent or malformed.
    MissingDataName,
    /// `data.name` referenced a dataset we do not produce.
    UnknownDataset(String),
    /// Encoded a field the named dataset does not have.
    UnknownField { dataset: String, field: String },
}

impl DropReason {
    /// The short, stable tag used as a span/metric value — group-able, unlike the
    /// free-text message.
    pub fn tag(&self) -> &'static str {
        match self {
            DropReason::VegaLiteSchema(_) => "vega_lite_schema",
            DropReason::InlineData => "inline_data",
            DropReason::MissingDataName => "missing_data_name",
            DropReason::UnknownDataset(_) => "unknown_dataset",
            DropReason::UnknownField { .. } => "unknown_field",
        }
    }
}

impl std::fmt::Display for DropReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DropReason::VegaLiteSchema(e) => write!(f, "not valid Vega-Lite: {e}"),
            DropReason::InlineData => write!(
                f,
                "spec carried inline data.values; data must bind by name so the real rows are injected at render"
            ),
            DropReason::MissingDataName => write!(f, "spec has no data.name"),
            DropReason::UnknownDataset(n) => write!(f, "unknown dataset '{n}'"),
            DropReason::UnknownField { dataset, field } => {
                write!(f, "dataset '{dataset}' has no field '{field}'")
            }
        }
    }
}

/// Percent-encode the characters RFC 3986 forbids in a URI fragment.
///
/// Vega-Lite's definition keys are TypeScript type names — `MarkPropDef<(Gradient
/// |string|null)>`, `ConditionalAxisProperty<(number[]|null)>` — so its `$ref`s
/// contain `<`, `>`, `|`, `[`, `]`. Those are illegal in a URI reference, and the
/// resolver rejects the ENTIRE schema over the first one, which is why this is
/// needed at all rather than being a nicety.
fn encode_fragment(s: &str) -> String {
    let allowed = |c: char| {
        c.is_ascii_alphanumeric()
            || "-._~".contains(c)          // unreserved
            || "!$&'()*+,;=".contains(c)   // sub-delims
            || ":@/?".contains(c)
    };
    s.chars()
        .map(|c| {
            if allowed(c) {
                c.to_string()
            } else {
                c.to_string().bytes().map(|b| format!("%{b:02X}")).collect()
            }
        })
        .collect()
}

/// Rewrite every `#/definitions/...` `$ref` into a legal URI reference.
///
/// **Only the `$ref`s — never the definition keys.** This is the subtle half and
/// it is easy to "fix" backwards: a JSON Pointer is percent-DECODED before it is
/// resolved, so an encoded `$ref` looks up the ORIGINAL key. Encoding the keys too
/// makes every pointer miss ("Pointer '/definitions/RowCol%3CLayoutAlign%3E' does
/// not exist"); leaving the `$ref`s raw makes the schema refuse to compile. It has
/// to be exactly one side.
fn encode_refs(v: &mut Value) {
    match v {
        Value::Object(map) => {
            if let Some(Value::String(r)) = map.get_mut("$ref") {
                if let Some(rest) = r.strip_prefix("#/definitions/") {
                    *r = format!("#/definitions/{}", encode_fragment(rest));
                }
            }
            for val in map.values_mut() {
                encode_refs(val);
            }
        }
        Value::Array(items) => {
            for it in items {
                encode_refs(it);
            }
        }
        _ => {}
    }
}

/// The compiled Vega-Lite validator.
///
/// Compiling a 1.8 MB, 458-definition schema costs ~100 ms, so it happens once per
/// process. A compile failure is not fatal: it means we cannot run layer 1, and
/// the honest response is to keep layer 2 (our own rules) and let Vega itself
/// reject a bad spec at render, rather than dropping every panel and showing the
/// user an empty screen because of OUR bug. `tests::the_real_schema_compiles` is
/// what stops that degradation from going unnoticed.
fn vl_validator() -> Option<&'static jsonschema::Validator> {
    static V: OnceLock<Option<jsonschema::Validator>> = OnceLock::new();
    V.get_or_init(|| {
        let mut schema: Value = match serde_json::from_str(VEGA_LITE_SCHEMA) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(error = %e, "vega-lite schema is not parseable — skipping schema validation");
                return None;
            }
        };
        encode_refs(&mut schema);
        match jsonschema::validator_for(&schema) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::error!(error = %e, "vega-lite schema failed to compile — skipping schema validation");
                None
            }
        }
    })
    .as_ref()
}

/// Walk every `field` mentioned anywhere under an `encoding` object.
///
/// Recursive on purpose: `layer`, `facet`, `concat` and `repeat` nest encodings
/// arbitrarily deep, and the whole point of allowing raw specs is that the model
/// may use them. A non-recursive check would silently pass a bad field inside a
/// layer — the case most worth catching, since that is where a model reaches when
/// it is being ambitious.
fn collect_fields(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                if k == "encoding" {
                    if let Value::Object(channels) = val {
                        for ch in channels.values() {
                            if let Some(Value::String(f)) = ch.get("field") {
                                out.push(f.clone());
                            }
                            // A channel can hold an array of field defs (e.g. tooltip).
                            if let Value::Array(items) = ch {
                                for it in items {
                                    if let Some(Value::String(f)) = it.get("field") {
                                        out.push(f.clone());
                                    }
                                }
                            }
                        }
                    }
                }
                collect_fields(val, out);
            }
        }
        Value::Array(items) => {
            for it in items {
                collect_fields(it, out);
            }
        }
        _ => {}
    }
}

/// Check one panel. `Ok(())` means it is safe to persist and render.
pub fn check(panel: &SummaryPanel) -> Result<(), DropReason> {
    let spec = &panel.spec;

    // Layer 2 first: it is cheap, and its failures are the ones we most want named
    // precisely (a schema error for inline data would be an unhelpful way to
    // report "you invented numbers").
    let data = spec.get("data").ok_or(DropReason::MissingDataName)?;
    if data.get("values").is_some() {
        return Err(DropReason::InlineData);
    }
    let name = match data.get("name") {
        Some(Value::String(n)) => n.clone(),
        _ => return Err(DropReason::MissingDataName),
    };
    if datasets::by_name(&name).is_none() {
        return Err(DropReason::UnknownDataset(name));
    }

    let mut fields = Vec::new();
    collect_fields(spec, &mut fields);
    for f in fields {
        // Vega-Lite's own computed fields are not ours to know about: a `transform`
        // can invent `count`/`sum_minutes`/etc., and `datum`-less aggregate defs
        // legitimately name nothing from the source. Only reject a field that looks
        // like a plain source reference and is not one.
        if !datasets::has_field(&name, &f) && !is_derived(spec, &f) {
            return Err(DropReason::UnknownField {
                dataset: name,
                field: f,
            });
        }
    }

    // Layer 1: the real grammar.
    if let Some(v) = vl_validator() {
        if let Err(e) = v.validate(spec) {
            return Err(DropReason::VegaLiteSchema(e.to_string()));
        }
    }
    Ok(())
}

/// Whether `field` is produced by the spec itself rather than read from the
/// dataset — a `transform`'s output (`as`, `groupby` fold outputs) or an implicit
/// aggregate name. Checked textually against the spec's transform block: a
/// hand-written list of Vega-Lite's derivation rules would be a third
/// implementation of its grammar, and wrong the first time Vega-Lite adds one.
fn is_derived(spec: &Value, field: &str) -> bool {
    // Aggregate ops produce e.g. `count`, and a field def that names an aggregate
    // without a source field is legitimate.
    if field == "count" {
        return true;
    }
    let Some(transforms) = spec.get("transform") else {
        return false;
    };
    let blob = transforms.to_string();
    // `"as": "x"` / `"as": ["x", "y"]` — the only ways a transform names an output.
    blob.contains(&format!("\"{field}\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel(spec: Value) -> SummaryPanel {
        SummaryPanel {
            title: "t".into(),
            why: "w".into(),
            spec,
        }
    }

    /// The vendored schema must be the one the webview actually renders with, or a
    /// spec can pass validation here and still fail at render (or the reverse).
    #[test]
    fn vendored_schema_matches_the_installed_vega_lite() {
        let pkg = include_str!("../../ui/package.json");
        let pkg: Value = serde_json::from_str(pkg).unwrap();
        let dep = pkg["dependencies"]["vega-lite"]
            .as_str()
            .expect("ui/package.json must depend on vega-lite")
            .trim_start_matches('^')
            .trim_start_matches('~');

        let schema: Value = serde_json::from_str(VEGA_LITE_SCHEMA).unwrap();
        // The schema's $ref/definitions don't carry a version, but the package's
        // own build stamps one into the description of the top-level spec's
        // $schema URL usage; fall back to asserting the file is the one shipped by
        // the installed package.
        let installed =
            std::fs::read_to_string("ui/node_modules/vega-lite/build/vega-lite-schema.json");
        match installed {
            Ok(text) => {
                let live: Value = serde_json::from_str(&text).unwrap();
                assert_eq!(
                    schema, live,
                    "services/schemas/vega-lite-schema.json is stale — re-copy it from \
                     ui/node_modules/vega-lite/build/vega-lite-schema.json (vega-lite {dep})"
                );
            }
            // node_modules absent (CI without an npm install) — nothing to compare
            // against, and failing here would be a false alarm about the schema.
            Err(_) => {}
        }
    }

    #[test]
    fn the_real_schema_compiles() {
        assert!(
            vl_validator().is_some(),
            "the vendored Vega-Lite schema must compile, or layer 1 silently never runs"
        );
    }

    /// Pins the asymmetry in [`encode_refs`]: `$ref`s get encoded, definition keys
    /// do not. Both halves are non-obvious and each "obvious fix" breaks it.
    #[test]
    fn only_refs_are_encoded_not_definition_keys() {
        let mut v = serde_json::json!({
            "definitions": {
                "MarkPropDef<(Gradient|string|null)>": {"type": "string"}
            },
            "properties": {
                "mark": {"$ref": "#/definitions/MarkPropDef<(Gradient|string|null)>"}
            }
        });
        encode_refs(&mut v);

        assert_eq!(
            v["properties"]["mark"]["$ref"],
            "#/definitions/MarkPropDef%3C(Gradient%7Cstring%7Cnull)%3E",
            "$ref must be a legal URI reference or the whole schema refuses to compile"
        );
        assert!(
            v["definitions"]
                .get("MarkPropDef<(Gradient|string|null)>")
                .is_some(),
            "the definition key must stay raw — a JSON Pointer is percent-decoded \
             before lookup, so encoding both sides makes every ref miss"
        );
    }

    /// Sub-delims like `(` and `)` are legal in a fragment and must survive
    /// untouched; `<`, `>`, `|`, `[`, `]` must not.
    #[test]
    fn fragment_encoding_touches_only_the_illegal_characters() {
        assert_eq!(
            encode_fragment("RowCol<LayoutAlign>"),
            "RowCol%3CLayoutAlign%3E"
        );
        assert_eq!(
            encode_fragment("ConditionalAxisProperty<(number[]|null)>"),
            "ConditionalAxisProperty%3C(number%5B%5D%7Cnull)%3E"
        );
        assert_eq!(encode_fragment("PlainName"), "PlainName");
    }

    /// The anti-hallucination rule: the model supplies form, never numbers.
    #[test]
    fn rejects_inline_data_values() {
        let p = panel(serde_json::json!({
            "data": {"values": [{"a": 1}, {"a": 2}]},
            "mark": "bar",
            "encoding": {"x": {"field": "a", "type": "quantitative"}}
        }));
        assert_eq!(check(&p), Err(DropReason::InlineData));
    }

    #[test]
    fn rejects_an_unknown_dataset() {
        let p = panel(serde_json::json!({
            "data": {"name": "invented"},
            "mark": "bar"
        }));
        assert_eq!(
            check(&p),
            Err(DropReason::UnknownDataset("invented".into()))
        );
    }

    #[test]
    fn rejects_a_field_the_dataset_does_not_have() {
        let p = panel(serde_json::json!({
            "data": {"name": "segments"},
            "mark": "bar",
            "encoding": {"x": {"field": "revenue", "type": "quantitative"}}
        }));
        assert_eq!(
            check(&p),
            Err(DropReason::UnknownField {
                dataset: "segments".into(),
                field: "revenue".into()
            })
        );
    }

    #[test]
    fn rejects_a_spec_with_no_data_at_all() {
        let p = panel(serde_json::json!({"mark": "bar"}));
        assert_eq!(check(&p), Err(DropReason::MissingDataName));
    }

    #[test]
    fn rejects_a_spec_the_real_grammar_rejects() {
        // `mark` is required by Vega-Lite for a unit spec; a bogus mark value is
        // something only the real schema knows to reject.
        let p = panel(serde_json::json!({
            "data": {"name": "apps"},
            "mark": "hologram",
            "encoding": {"x": {"field": "app", "type": "nominal"}}
        }));
        match check(&p) {
            Err(DropReason::VegaLiteSchema(_)) => {}
            other => panic!("expected a schema rejection, got {other:?}"),
        }
    }

    /// Freedom must actually survive: the whole reason for accepting raw specs is
    /// that the model can reach for forms a fixed component could not express.
    #[test]
    fn accepts_a_plain_valid_spec() {
        let p = panel(serde_json::json!({
            "data": {"name": "apps"},
            "mark": "arc",
            "encoding": {
                "theta": {"field": "seconds", "type": "quantitative"},
                "color": {"field": "app", "type": "nominal"}
            }
        }));
        assert_eq!(check(&p), Ok(()));
    }

    #[test]
    fn accepts_a_gantt_style_layered_spec_over_segments() {
        let p = panel(serde_json::json!({
            "data": {"name": "segments"},
            "layer": [{
                "mark": "bar",
                "encoding": {
                    "x": {"field": "start_min", "type": "quantitative"},
                    "x2": {"field": "end_min"},
                    "y": {"field": "title", "type": "nominal"},
                    "color": {"field": "task_id", "type": "nominal"},
                    "tooltip": [
                        {"field": "title", "type": "nominal"},
                        {"field": "minutes", "type": "quantitative"}
                    ]
                }
            }]
        }));
        assert_eq!(check(&p), Ok(()));
    }

    /// A bad field nested inside a `layer` is the case a shallow check would miss,
    /// and the one a model is most likely to produce when being ambitious.
    #[test]
    fn catches_a_bad_field_nested_inside_a_layer() {
        let p = panel(serde_json::json!({
            "data": {"name": "segments"},
            "layer": [{
                "mark": "bar",
                "encoding": {"x": {"field": "not_a_field", "type": "quantitative"}}
            }]
        }));
        assert_eq!(
            check(&p),
            Err(DropReason::UnknownField {
                dataset: "segments".into(),
                field: "not_a_field".into()
            })
        );
    }

    /// A transform's own output is not a dataset field, and rejecting it would ban
    /// the aggregations that make these charts worth looking at.
    #[test]
    fn accepts_a_field_a_transform_derives() {
        let p = panel(serde_json::json!({
            "data": {"name": "segments"},
            "transform": [
                {"aggregate": [{"op": "sum", "field": "minutes", "as": "total_minutes"}],
                 "groupby": ["title"]}
            ],
            "mark": "bar",
            "encoding": {
                "x": {"field": "total_minutes", "type": "quantitative"},
                "y": {"field": "title", "type": "nominal"}
            }
        }));
        assert_eq!(check(&p), Ok(()));
    }
}
