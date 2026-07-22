//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! On-device redaction + error-only filtering for the *shipping* leg of the
//! telemetry spool. This is the privacy boundary: it runs on a spooled `.otlp`
//! payload immediately before it would leave the machine, and it is the ONLY
//! transform between capture and delivery.
//!
//! # Why this exists
//! Local capture is full-fidelity on purpose — `meridian logs` (see
//! [`crate::telemetry_spool::render`]) needs every span and attribute to be a
//! useful local debugger, and those attributes include the most sensitive data
//! Meridian holds: OCR text, accessibility-tree content, window titles, browser
//! URLs, coding-agent conversation bodies, and (per the observability rules in
//! `CLAUDE.md`) the exact LLM inputs/outputs on `llm.call` spans. None of that
//! may egress. So the spool file on disk stays rich, and this module produces a
//! *separate, stripped* copy for the shipper to POST. Nothing here mutates the
//! on-disk file.
//!
//! # The two rules (fail closed)
//! 1. **Error-only.** Only records that represent a problem are shipped: log
//!    records at `WARN` and above, and spans whose status is `ERROR`. Everything
//!    else is dropped. This is both the volume lever and a privacy lever — the
//!    high-volume `INFO`/`DEBUG`/`SPAN` records that carry the content-bearing
//!    attributes never reach the ship leg at all. A tight [`NOISE_SUBSTRINGS`]
//!    denylist additionally drops known-benign WARN repeaters that would flood
//!    the backend without adding signal.
//! 2. **Allowlist by value type + key.** For every attribute that survives rule
//!    1, we keep it only if it cannot carry free-text content:
//!    - numeric (`int`/`double`) and `bool` values are kept unconditionally —
//!      they are counts, latencies, tokens, status codes; they structurally
//!      cannot hold OCR/PII text;
//!    - `string` values are kept ONLY when the key is on [`SAFE_STRING_KEYS`].
//!      Identifier-like values are home-dir-path-scrubbed; the free-text subset
//!      ([`FREE_TEXT_KEYS`] — `*.message`, stacktraces) additionally gets
//!      URL/email/token scrubbing ([`scrub_text`]) plus a length [`clamp`];
//!    - `bytes`/`array`/`kvlist` values are dropped outright — they can nest
//!      arbitrary strings we cannot cheaply scrub.
//!
//! A brand-new content-bearing attribute added anywhere in the codebase is
//! therefore dropped by default until someone deliberately allowlists it.
//!
//! Span `events` and `links` are cleared entirely — events routinely carry
//! content attributes and are not worth the per-attribute redaction cost for an
//! error report. `status.message` (a span field, NOT an attribute, so the
//! attribute allowlist never sees it) gets the same free-text scrub — it's
//! where Meridian's ERROR-status failure descriptions land.
//!
//! # Who calls this
//! [`crate::telemetry_spool::shipper::run_tick`] — once per pending file, after
//! `resolve_otlp_target()` confirms shipping is allowed, before `ship_one`.
//!
//! # Related
//! - [`crate::telemetry_spool::render`] — the *read* side; shares the same OTLP
//!   proto decode but keeps everything (local, never leaves the machine).
//! - [`crate::observability::otlp_target`] — decides *whether* to ship at all
//!   (consent + target); this decides *what* may ship once that says yes.

use std::sync::OnceLock;

use opentelemetry_proto::tonic::{
    collector::{logs::v1::ExportLogsServiceRequest, trace::v1::ExportTraceServiceRequest},
    common::v1::{any_value::Value, AnyValue, KeyValue},
    logs::v1::LogRecord,
    trace::v1::{Span, Status},
};
use prost::Message;
use regex::Regex;

/// OTLP `SeverityNumber` for `WARN`; the error-only log threshold is "this or
/// higher" (WARN=13, ERROR=17, FATAL=21). Matches the ranges in
/// [`crate::telemetry_spool::render`].
const SEVERITY_WARN: i32 = 13;

/// OTLP `StatusCode::Error` discriminant (`STATUS_CODE_UNSET`=0, `_OK`=1,
/// `_ERROR`=2). A span is shipped only when its status code equals this.
const STATUS_CODE_ERROR: i32 = 2;

/// String-valued attribute keys judged safe to egress on an error report —
/// operational metadata, never content. Everything NOT in this set (with a
/// string value) is dropped by [`keep_attribute`]. Keep this list conservative;
/// it is the reviewable definition of what free text may leave the machine.
///
/// Deliberately EXCLUDED (content-bearing, must never ship): `llm.request`,
/// `llm.response`, `ocr.*`, `window.title`/`window_titles`, `browser.url`,
/// `session.summary`, `prompt`, any coding-agent conversation body, etc.
const SAFE_STRING_KEYS: &[&str] = &[
    // ── resource / SDK identity ──────────────────────────────────────────────
    "service.name",
    "service.version",
    "service.instance.id",
    "host.name",
    "host.arch",
    "os.type",
    "os.name",
    "os.version",
    "process.runtime.name",
    "process.runtime.version",
    "telemetry.sdk.name",
    "telemetry.sdk.language",
    "telemetry.sdk.version",
    "deployment.environment",
    // Meridian build/install identity (non-secret, non-content).
    "app.channel",
    "app.version",
    "app.install_mode",
    // ── error / diagnostic shape ─────────────────────────────────────────────
    // The essential debug signal. The message-class values here are ALSO listed
    // in `FREE_TEXT_KEYS`, so they get the full `scrub_text` + `clamp`, not just
    // a path scrub, before egress.
    "error.type",
    "error.message",
    "exception.type",
    "exception.message",
    "exception.stacktrace",
    "otel.status_code",
    "otel.status_description",
    // ── code location (paths scrubbed) ───────────────────────────────────────
    "code.function",
    "code.namespace",
    "code.filepath",
    "code.lineno",
    "thread.name",
    "target",
    "level",
    // ── protocol metadata (values are enums/verbs, not content) ──────────────
    "http.method",
    "http.request.method",
    "db.system",
    "db.operation",
    "rpc.method",
    "rpc.system",
];

/// The subset of [`SAFE_STRING_KEYS`] whose values are FREE TEXT — a human or
/// error message that interpolates runtime data, not a structured identifier.
/// These get the full [`scrub_text`] + [`clamp`] treatment, not just the
/// home-dir path scrub, because an `anyhow` context chain or a third-party
/// `Display` impl can splice a URL, email, token, or captured fragment into
/// them that a path-only scrub would miss. `status.message` (a span field, not
/// an attribute) is scrubbed the same way in [`scrub_span_status`].
const FREE_TEXT_KEYS: &[&str] = &[
    "error.message",
    "exception.message",
    "exception.stacktrace",
    "otel.status_description",
];

/// Known-benign, high-frequency WARN messages dropped from the ship leg even
/// though they pass the severity filter — they repeat every poll and would
/// flood the central backend without adding signal (the screenpipe
/// noise-allowlist lesson). Matched case-insensitively as a substring of the
/// log body, so entries MUST be lowercase. Keep this list tight: it is a
/// denylist over an otherwise-shipped severity, so an over-broad entry silently
/// hides real warnings.
const NOISE_SUBSTRINGS: &[&str] = &[
    // Fires every poll (~60s) when an approved worklog has no PM provider
    // configured — a benign configuration state, not a fault.
    "approved worklog waiting but its provider is not configured",
];

/// Result of running [`redact_and_filter`] on one spooled payload.
pub enum Redacted {
    /// Nothing survived the error-only filter. The shipper should archive the
    /// source file to `sent/` WITHOUT a network POST — there is nothing to send.
    Empty,
    /// A re-encoded, allowlisted OTLP payload safe to POST, plus what was
    /// stripped (for the shipper's structured log).
    Payload { bytes: Vec<u8>, stats: RedactStats },
    /// The bytes could not be decoded as OTLP for this signal. The caller MUST
    /// NOT fall back to shipping the original bytes — an undecodable payload has
    /// not been through the allowlist, so it could leak. Treat as terminal.
    Undecodable,
}

/// What redaction removed from one payload — emitted as span/log fields by the
/// shipper so the drop rate is observable without exposing any dropped content.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RedactStats {
    /// Log/span records present before the error-only filter.
    pub records_in: usize,
    /// Log/span records that survived the error-only filter (i.e. shipped).
    pub records_out: usize,
    /// Attributes dropped by the allowlist across all surviving records.
    pub attrs_dropped: usize,
}

/// Redact and error-filter one spooled OTLP payload. `signal` is `"traces"` or
/// `"logs"` (from the spool filename); any other value is treated as
/// undecodable rather than shipped blind.
pub fn redact_and_filter(signal: &str, bytes: &[u8]) -> Redacted {
    match signal {
        "logs" => redact_logs(bytes),
        "traces" => redact_traces(bytes),
        _ => Redacted::Undecodable,
    }
}

fn redact_logs(bytes: &[u8]) -> Redacted {
    let Ok(mut req) = ExportLogsServiceRequest::decode(bytes) else {
        return Redacted::Undecodable;
    };
    let mut stats = RedactStats::default();

    for rl in &mut req.resource_logs {
        if let Some(res) = rl.resource.as_mut() {
            stats.attrs_dropped += redact_attributes(&mut res.attributes);
        }
        for sl in &mut rl.scope_logs {
            if let Some(scope) = sl.scope.as_mut() {
                stats.attrs_dropped += redact_attributes(&mut scope.attributes);
            }
            let before = sl.log_records.len();
            stats.records_in += before;
            sl.log_records
                .retain(|lr| log_is_error(lr) && !log_is_noise(lr));
            stats.records_out += sl.log_records.len();
            for lr in &mut sl.log_records {
                stats.attrs_dropped += redact_attributes(&mut lr.attributes);
                scrub_log_body(lr);
            }
        }
        // Drop scope groups that lost all their records so we don't ship empty
        // envelopes.
        rl.scope_logs.retain(|sl| !sl.log_records.is_empty());
    }
    req.resource_logs.retain(|rl| !rl.scope_logs.is_empty());

    if stats.records_out == 0 {
        return Redacted::Empty;
    }
    Redacted::Payload {
        bytes: req.encode_to_vec(),
        stats,
    }
}

fn redact_traces(bytes: &[u8]) -> Redacted {
    let Ok(mut req) = ExportTraceServiceRequest::decode(bytes) else {
        return Redacted::Undecodable;
    };
    let mut stats = RedactStats::default();

    for rs in &mut req.resource_spans {
        if let Some(res) = rs.resource.as_mut() {
            stats.attrs_dropped += redact_attributes(&mut res.attributes);
        }
        for ss in &mut rs.scope_spans {
            if let Some(scope) = ss.scope.as_mut() {
                stats.attrs_dropped += redact_attributes(&mut scope.attributes);
            }
            let before = ss.spans.len();
            stats.records_in += before;
            ss.spans.retain(span_is_error);
            stats.records_out += ss.spans.len();
            for span in &mut ss.spans {
                stats.attrs_dropped += redact_attributes(&mut span.attributes);
                scrub_span_status(span);
                strip_span_children(span);
            }
        }
        rs.scope_spans.retain(|ss| !ss.spans.is_empty());
    }
    req.resource_spans.retain(|rs| !rs.scope_spans.is_empty());

    if stats.records_out == 0 {
        return Redacted::Empty;
    }
    Redacted::Payload {
        bytes: req.encode_to_vec(),
        stats,
    }
}

/// A log record ships iff it is WARN or above. Prefer the numeric severity;
/// fall back to the text when the number is unset (0).
fn log_is_error(lr: &LogRecord) -> bool {
    if lr.severity_number != 0 {
        return lr.severity_number >= SEVERITY_WARN;
    }
    matches!(
        lr.severity_text.to_ascii_uppercase().as_str(),
        "WARN" | "WARNING" | "ERROR" | "FATAL" | "CRITICAL"
    )
}

/// True when a WARN+ record is known-benign, high-frequency noise (see
/// [`NOISE_SUBSTRINGS`]) — dropped from the ship leg so one repeating message
/// can't flood the backend. Non-string bodies never match.
fn log_is_noise(lr: &LogRecord) -> bool {
    let Some(AnyValue {
        value: Some(Value::StringValue(body)),
    }) = lr.body.as_ref()
    else {
        return false;
    };
    let lower = body.to_ascii_lowercase();
    NOISE_SUBSTRINGS.iter().any(|n| lower.contains(n))
}

/// A span ships iff its status code is ERROR. UNSET/OK spans (the overwhelming
/// majority, and the ones carrying content attributes) are dropped.
fn span_is_error(span: &Span) -> bool {
    span.status
        .as_ref()
        .map(|Status { code, .. }| *code == STATUS_CODE_ERROR)
        .unwrap_or(false)
}

/// Scrub the span's status message — a sibling field, NOT an attribute, so
/// [`redact_attributes`] never sees it. Per Meridian's convention (CLAUDE.md:
/// "set span status ERROR with a message on failures") this is exactly where
/// failure descriptions land, and those interpolate data, so it must go through
/// the same free-text scrub + clamp as `*.message`.
fn scrub_span_status(span: &mut Span) {
    if let Some(status) = span.status.as_mut() {
        if !status.message.is_empty() {
            status.message = clamp(scrub_text(&status.message));
        }
    }
}

/// Clear span children that can carry content we don't per-attribute redact:
/// `events` (each has its own attributes + name) and `links` (attributes on a
/// cross-span reference). The error span's own name, status, timing, ids, and
/// allowlisted attributes are enough to triage.
fn strip_span_children(span: &mut Span) {
    span.events.clear();
    span.dropped_events_count = 0;
    span.links.clear();
    span.dropped_links_count = 0;
}

/// Apply the value-type + key allowlist to one attribute list, returning how
/// many attributes were removed.
fn redact_attributes(attrs: &mut Vec<KeyValue>) -> usize {
    let before = attrs.len();
    attrs.retain_mut(keep_attribute);
    before - attrs.len()
}

/// The core allowlist decision for a single attribute. See the module doc's
/// "two rules". Mutates a kept string in place: FREE-TEXT keys get the full
/// [`scrub_text`] + [`clamp`]; other allowlisted strings (structured
/// identifiers, code locations) get only the light home-dir path scrub.
fn keep_attribute(kv: &mut KeyValue) -> bool {
    match kv.value.as_mut().and_then(|v| v.value.as_mut()) {
        // Numeric / bool can't carry free text — always safe.
        Some(Value::IntValue(_)) | Some(Value::DoubleValue(_)) | Some(Value::BoolValue(_)) => true,
        Some(Value::StringValue(s)) => {
            if is_free_text_key(&kv.key) {
                *s = clamp(scrub_text(s));
                true
            } else if is_safe_string_key(&kv.key) {
                *s = scrub_paths(s);
                true
            } else {
                false
            }
        }
        // Bytes / array / kvlist can nest arbitrary content — drop.
        _ => false,
    }
}

fn is_safe_string_key(key: &str) -> bool {
    SAFE_STRING_KEYS.contains(&key)
}

fn is_free_text_key(key: &str) -> bool {
    FREE_TEXT_KEYS.contains(&key)
}

/// Keep the log body (it's the error message — the primary debug signal) but,
/// being free text, run it through the full [`scrub_text`] + [`clamp`] before
/// egress. Non-string bodies are rare for logs and left as-is.
fn scrub_log_body(lr: &mut LogRecord) {
    if let Some(AnyValue {
        value: Some(Value::StringValue(s)),
    }) = lr.body.as_mut()
    {
        *s = clamp(scrub_text(s));
    }
}

/// Clamp free text to a bounded length so a pathological message (a dumped
/// blob, a giant stacktrace) can neither balloon an error report nor smuggle
/// bulk content past the pattern scrubs.
fn clamp(s: String) -> String {
    const MAX_CHARS: usize = 2000;
    if s.chars().count() <= MAX_CHARS {
        return s;
    }
    let mut out: String = s.chars().take(MAX_CHARS).collect();
    out.push_str("…[truncated]");
    out
}

/// Full scrub for FREE-TEXT values (log bodies, `*.message`,
/// `exception.stacktrace`, `status.message`). Redacts the leak vectors a
/// `Display`/`anyhow` chain routinely interpolates and that a home-dir path
/// scrub alone would miss: URLs, emails, home-dir usernames, and long opaque
/// tokens/hashes/base64 blobs. It cannot catch every possible content fragment,
/// so it is paired with the error-only filter (which already excludes the
/// high-volume content-bearing INFO/SPAN records) and the [`clamp`] length cap.
///
/// `pub` because the tray's Sentry `before_send` (`crash.rs`, Phase 2B) runs the
/// exact same scrub over crash event messages/exception values, so the on-device
/// redaction is identical whether an error egresses via the OTLP spool or Sentry.
pub fn scrub_text(s: &str) -> String {
    static RULES: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    let rules = RULES.get_or_init(|| {
        vec![
            // Any scheme://… URL (http, https, file, …) — the whole URL.
            (
                Regex::new(r"(?i)\b[a-z][a-z0-9+.\-]*://\S+").expect("url regex"),
                "<url>",
            ),
            // Email addresses.
            (
                Regex::new(r"(?i)\b[a-z0-9._%+\-]+@[a-z0-9.\-]+\.[a-z]{2,}\b")
                    .expect("email regex"),
                "<email>",
            ),
            // Home-dir username segment — keep the structure, drop the name.
            (
                Regex::new(r"(?i)([/\\](?:Users|home)[/\\])([^/\\]+)").expect("path regex"),
                "${1}<user>",
            ),
            // Long opaque tokens / hashes / base64 blobs (32+ chars) — API keys,
            // bearer tokens, dumped binary content.
            (
                Regex::new(r"[A-Za-z0-9+/=_\-]{32,}").expect("blob regex"),
                "<redacted>",
            ),
        ]
    });
    let mut out = s.to_string();
    for (re, rep) in rules {
        out = re.replace_all(&out, *rep).into_owned();
    }
    out
}

/// Light scrub for allowlisted NON-free-text strings (e.g. `code.filepath`):
/// only redact the home-dir username segment (`/Users/<name>` → `/Users/<user>`,
/// also `/home/` and `C:\Users\`) so a scrubbed path can't leak the OS account
/// name, without risking mangling a structured identifier.
fn scrub_paths(s: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        // (?i) so `C:\Users` and `c:\users` both match; the segment after the
        // root (up to the next separator or end) is the username.
        Regex::new(r"(?i)([/\\](?:Users|home)[/\\])([^/\\]+)").expect("valid scrub regex")
    });
    re.replace_all(s, "${1}<user>").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::{
        common::v1::{any_value, InstrumentationScope},
        logs::v1::{ResourceLogs, ScopeLogs},
        resource::v1::Resource,
        trace::v1::{span, ResourceSpans, ScopeSpans},
    };

    fn str_attr(key: &str, val: &str) -> KeyValue {
        KeyValue {
            key: key.to_string(),
            value: Some(AnyValue {
                value: Some(any_value::Value::StringValue(val.to_string())),
            }),
        }
    }

    fn int_attr(key: &str, val: i64) -> KeyValue {
        KeyValue {
            key: key.to_string(),
            value: Some(AnyValue {
                value: Some(any_value::Value::IntValue(val)),
            }),
        }
    }

    fn log_record(severity_number: i32, attrs: Vec<KeyValue>) -> LogRecord {
        LogRecord {
            severity_number,
            attributes: attrs,
            ..Default::default()
        }
    }

    fn encode_logs(records: Vec<LogRecord>) -> Vec<u8> {
        ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: Some(Resource::default()),
                scope_logs: vec![ScopeLogs {
                    scope: Some(InstrumentationScope::default()),
                    log_records: records,
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        }
        .encode_to_vec()
    }

    fn decode_logs(bytes: &[u8]) -> Vec<LogRecord> {
        ExportLogsServiceRequest::decode(bytes)
            .unwrap()
            .resource_logs
            .into_iter()
            .flat_map(|rl| rl.scope_logs)
            .flat_map(|sl| sl.log_records)
            .collect()
    }

    #[test]
    fn drops_info_keeps_warn_and_error() {
        // INFO(9), WARN(13), ERROR(17)
        let bytes = encode_logs(vec![
            log_record(9, vec![]),
            log_record(13, vec![]),
            log_record(17, vec![]),
        ]);
        let Redacted::Payload { bytes, stats } = redact_and_filter("logs", &bytes) else {
            panic!("expected payload");
        };
        assert_eq!(stats.records_in, 3);
        assert_eq!(stats.records_out, 2);
        let kept = decode_logs(&bytes);
        assert_eq!(kept.len(), 2);
        assert!(kept.iter().all(|r| r.severity_number >= SEVERITY_WARN));
    }

    #[test]
    fn all_info_yields_empty() {
        let bytes = encode_logs(vec![log_record(9, vec![]), log_record(5, vec![])]);
        assert!(matches!(redact_and_filter("logs", &bytes), Redacted::Empty));
    }

    #[test]
    fn string_attrs_allowlisted_numeric_always_kept() {
        let record = log_record(
            17,
            vec![
                str_attr("llm.response", "the model said something sensitive"),
                str_attr("error.type", "TimeoutError"),
                int_attr("retry.count", 3),
                int_attr("duration_ms", 1200),
            ],
        );
        let bytes = encode_logs(vec![record]);
        let Redacted::Payload { bytes, stats } = redact_and_filter("logs", &bytes) else {
            panic!("expected payload");
        };
        // Only the non-allowlisted string (`llm.response`) is dropped.
        assert_eq!(stats.attrs_dropped, 1);
        let kept = decode_logs(&bytes);
        let keys: Vec<&str> = kept[0].attributes.iter().map(|a| a.key.as_str()).collect();
        assert!(keys.contains(&"error.type"));
        assert!(keys.contains(&"retry.count")); // numeric kept even though not allowlisted
        assert!(keys.contains(&"duration_ms"));
        assert!(!keys.contains(&"llm.response"));
    }

    #[test]
    fn kept_string_values_are_path_scrubbed() {
        let record = log_record(
            17,
            vec![str_attr(
                "code.filepath",
                "/Users/akarsh/Documents/Meridiona/meridian/src/main.rs",
            )],
        );
        let bytes = encode_logs(vec![record]);
        let Redacted::Payload { bytes, .. } = redact_and_filter("logs", &bytes) else {
            panic!("expected payload");
        };
        let kept = decode_logs(&bytes);
        let AnyValue {
            value: Some(any_value::Value::StringValue(v)),
        } = kept[0].attributes[0].value.clone().unwrap()
        else {
            panic!("expected string value");
        };
        assert_eq!(v, "/Users/<user>/Documents/Meridiona/meridian/src/main.rs");
    }

    #[test]
    fn undecodable_bytes_never_ship() {
        assert!(matches!(
            redact_and_filter("logs", b"not otlp at all"),
            Redacted::Undecodable
        ));
        assert!(matches!(
            redact_and_filter("traces", b"not otlp at all"),
            Redacted::Undecodable
        ));
        assert!(matches!(
            redact_and_filter("unknown-signal", b"whatever"),
            Redacted::Undecodable
        ));
    }

    // ── traces ───────────────────────────────────────────────────────────────

    fn span_with(status_code: i32, attrs: Vec<KeyValue>, with_event: bool) -> Span {
        Span {
            name: "llm.call".to_string(),
            status: Some(Status {
                code: status_code,
                message: String::new(),
            }),
            attributes: attrs,
            events: if with_event {
                vec![span::Event {
                    name: "prompt".to_string(),
                    attributes: vec![str_attr("llm.request", "secret prompt")],
                    ..Default::default()
                }]
            } else {
                vec![]
            },
            ..Default::default()
        }
    }

    fn encode_spans(spans: Vec<Span>) -> Vec<u8> {
        ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: Some(Resource::default()),
                scope_spans: vec![ScopeSpans {
                    scope: Some(InstrumentationScope::default()),
                    spans,
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        }
        .encode_to_vec()
    }

    fn decode_spans(bytes: &[u8]) -> Vec<Span> {
        ExportTraceServiceRequest::decode(bytes)
            .unwrap()
            .resource_spans
            .into_iter()
            .flat_map(|rs| rs.scope_spans)
            .flat_map(|ss| ss.spans)
            .collect()
    }

    #[test]
    fn keeps_only_error_status_spans() {
        // UNSET(0), OK(1), ERROR(2)
        let bytes = encode_spans(vec![
            span_with(0, vec![], false),
            span_with(1, vec![], false),
            span_with(STATUS_CODE_ERROR, vec![], false),
        ]);
        let Redacted::Payload { bytes, stats } = redact_and_filter("traces", &bytes) else {
            panic!("expected payload");
        };
        assert_eq!(stats.records_in, 3);
        assert_eq!(stats.records_out, 1);
        assert_eq!(decode_spans(&bytes).len(), 1);
    }

    #[test]
    fn error_span_events_and_content_attrs_stripped() {
        let bytes = encode_spans(vec![span_with(
            STATUS_CODE_ERROR,
            vec![
                str_attr("llm.request", "the whole prompt"),
                str_attr("error.type", "RateLimited"),
                int_attr("http.status_code", 429),
            ],
            true,
        )]);
        let Redacted::Payload { bytes, .. } = redact_and_filter("traces", &bytes) else {
            panic!("expected payload");
        };
        let spans = decode_spans(&bytes);
        assert_eq!(spans.len(), 1);
        assert!(spans[0].events.is_empty(), "events must be cleared");
        let keys: Vec<&str> = spans[0].attributes.iter().map(|a| a.key.as_str()).collect();
        assert!(
            !keys.contains(&"llm.request"),
            "content attr must be dropped"
        );
        assert!(keys.contains(&"error.type"));
        assert!(keys.contains(&"http.status_code"));
    }

    #[test]
    fn all_ok_spans_yield_empty() {
        let bytes = encode_spans(vec![span_with(1, vec![], false)]);
        assert!(matches!(
            redact_and_filter("traces", &bytes),
            Redacted::Empty
        ));
    }

    // ── free-text scrubbing (URLs / emails / tokens / status.message) ─────────

    #[test]
    fn free_text_attr_scrubs_url_email_and_token() {
        let record = log_record(
            17,
            vec![str_attr(
                "error.message",
                "POST https://api.example.com/v1 for user a@b.com failed \
                 key ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcd",
            )],
        );
        let bytes = encode_logs(vec![record]);
        let Redacted::Payload { bytes, .. } = redact_and_filter("logs", &bytes) else {
            panic!("expected payload");
        };
        let kept = decode_logs(&bytes);
        let AnyValue {
            value: Some(any_value::Value::StringValue(v)),
        } = kept[0].attributes[0].value.clone().unwrap()
        else {
            panic!("expected string value");
        };
        assert!(v.contains("<url>"), "url redacted: {v}");
        assert!(v.contains("<email>"), "email redacted: {v}");
        assert!(v.contains("<redacted>"), "token redacted: {v}");
        assert!(!v.contains("example.com"));
        assert!(!v.contains("a@b.com"));
    }

    #[test]
    fn span_status_message_is_scrubbed() {
        let bytes = encode_spans(vec![Span {
            name: "worklog.generate".to_string(),
            status: Some(Status {
                code: STATUS_CODE_ERROR,
                message: "failed reading /Users/akarsh/secret.txt via https://x.io".to_string(),
            }),
            ..Default::default()
        }]);
        let Redacted::Payload { bytes, .. } = redact_and_filter("traces", &bytes) else {
            panic!("expected payload");
        };
        let spans = decode_spans(&bytes);
        let msg = &spans[0].status.as_ref().unwrap().message;
        assert!(msg.contains("/Users/<user>/"), "path scrubbed: {msg}");
        assert!(msg.contains("<url>"), "url scrubbed: {msg}");
        assert!(!msg.contains("akarsh"), "username leaked: {msg}");
    }

    #[test]
    fn known_noise_warn_is_dropped_but_real_warn_survives() {
        let noise = {
            let mut r = log_record(13, vec![]);
            r.body = Some(AnyValue {
                value: Some(any_value::Value::StringValue(
                    "approved worklog waiting but its provider is not configured — not posting"
                        .to_string(),
                )),
            });
            r
        };
        let real = {
            let mut r = log_record(13, vec![]);
            r.body = Some(AnyValue {
                value: Some(any_value::Value::StringValue(
                    "disk write failed".to_string(),
                )),
            });
            r
        };
        let bytes = encode_logs(vec![noise, real]);
        let Redacted::Payload { bytes, stats } = redact_and_filter("logs", &bytes) else {
            panic!("expected payload");
        };
        assert_eq!(stats.records_in, 2);
        assert_eq!(stats.records_out, 1, "noise dropped, real WARN kept");
        let kept = decode_logs(&bytes);
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn clamp_truncates_long_free_text() {
        let out = clamp("x".repeat(5000));
        assert!(out.ends_with("…[truncated]"));
        // 2000 kept chars + the truncation marker, nothing more.
        assert_eq!(out.chars().count(), 2000 + "…[truncated]".chars().count());
    }
}
