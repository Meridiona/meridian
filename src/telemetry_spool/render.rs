//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Decode spooled `.otlp` files back into human-readable log/span records —
//! the read side of the telemetry spool. This is what makes the OTel spool
//! usable as a local, tool-free log store: no live OpenObserve is needed to
//! read it, just this decoder.
//!
//! # Who calls this
//! `src/main.rs`'s `logs` dispatch (the `meridian logs` CLI) — the direct
//! replacement for the old JSONL-tailing `meridian logs` (which read
//! launchd-redirected stdout/stderr text; see `observability.rs`'s module doc
//! for why that mirror was removed).
//!
//! # Related
//! - `writer.rs` — the filename scheme (`<signal>-<unix_micros>-<seq>.otlp`) this decodes.
//! - `shipper.rs` — the background task that eventually deletes shipped files
//!   from `pending/`; a record vanishes from `meridian logs` once shipped+pruned
//!   on a Dev/Bare install. A Canonical install never ships, so nothing here
//!   ever disappears except via the age-based retention prune.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use opentelemetry_proto::tonic::{
    collector::{logs::v1::ExportLogsServiceRequest, trace::v1::ExportTraceServiceRequest},
    common::v1::{any_value::Value, AnyValue},
    resource::v1::Resource,
};
use prost::Message;

use crate::telemetry_spool::writer::{pending_dir, resolve_telemetry_dir, sent_dir};

/// One decoded, human-readable record — a log line or a span start, both
/// normalised to the same shape so `meridian logs` can sort/filter/render
/// them uniformly regardless of signal.
pub struct RenderedRecord {
    pub time_unix_nano: u64,
    pub is_span: bool,
    pub service_name: String,
    pub severity: String,
    pub body: String,
    pub trace_id: String,
    pub span_id: String,
    /// The record's structured fields, already stringified, in emission order.
    ///
    /// These were previously decoded and thrown away, so `meridian logs` showed
    /// the message of every record and none of its data. That is the opposite
    /// of how this codebase is instrumented — CLAUDE.md requires values to be
    /// structured fields and never formatted into the message — so the one
    /// supported way to read logs locally was systematically hiding the half
    /// that was written to be machine-readable. A `SIGTERM received` with no
    /// `pid`, an `ETL run failed` with no `error`.
    pub attributes: Vec<(String, String)>,
}

/// Attribute keys `meridian logs` does not print.
///
/// Not a privacy filter — this is the local, full-fidelity read path and
/// nothing here is withheld from the spool or from an export bundle. It is
/// purely about signal: `opentelemetry-appender-tracing`'s
/// `experimental_metadata_attributes` feature stamps call-site metadata on
/// EVERY record, which would triple the width of every line with the one thing
/// a reader already knows (they can see the message).
///
/// Namespaces: every key under them is call-site metadata, so a prefix match
/// is correct and `code.filepath`/`code.lineno`/`code.namespace` are covered by
/// one entry.
const UNPRINTED_ATTR_NAMESPACES: &[&str] = &["code.", "thread."];

/// Whole keys. Deliberately NOT prefixes: these are single field names, and
/// matching them by prefix would silently swallow an emitter's own field that
/// merely starts the same way - `log.target_override`, `busy_ns_budget`,
/// `idle_ns_extra`. Hiding a field the author chose to record is precisely the
/// bug this rendering exists to fix, so it must not be reintroduced by the
/// filter meant to reduce noise.
const UNPRINTED_ATTR_KEYS: &[&str] = &["log.target", "busy_ns", "idle_ns"];

/// Is this a call-site metadata key rather than a value the emitter chose?
fn is_unprinted_attr(key: &str) -> bool {
    UNPRINTED_ATTR_NAMESPACES.iter().any(|p| key.starts_with(p))
        || UNPRINTED_ATTR_KEYS.contains(&key)
}

/// Decode one spooled file into [`RenderedRecord`]s. The signal (logs vs
/// traces) is read off the filename (`writer.rs`'s naming scheme). A
/// corrupt/unparseable file yields an empty Vec rather than an error — one
/// bad spool file must never wedge `meridian logs` for everything else.
pub fn decode_file(path: &Path) -> Vec<RenderedRecord> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.starts_with("logs-") {
        decode_logs(&bytes).unwrap_or_default()
    } else if name.starts_with("traces-") {
        decode_traces(&bytes).unwrap_or_default()
    } else {
        Vec::new()
    }
}

fn decode_logs(bytes: &[u8]) -> Result<Vec<RenderedRecord>> {
    let req = ExportLogsServiceRequest::decode(bytes).context("decode ExportLogsServiceRequest")?;
    let mut out = Vec::new();
    for rl in req.resource_logs {
        let service_name = resource_service_name(rl.resource.as_ref());
        for sl in rl.scope_logs {
            for lr in sl.log_records {
                let time = if lr.time_unix_nano != 0 {
                    lr.time_unix_nano
                } else {
                    lr.observed_time_unix_nano
                };
                let severity = if !lr.severity_text.is_empty() {
                    lr.severity_text
                } else {
                    severity_number_name(lr.severity_number).to_string()
                };
                out.push(RenderedRecord {
                    time_unix_nano: time,
                    is_span: false,
                    service_name: service_name.clone(),
                    severity,
                    body: any_value_to_string(lr.body.as_ref()),
                    trace_id: hex::encode(&lr.trace_id),
                    span_id: hex::encode(&lr.span_id),
                    attributes: collect_attributes(
                        lr.attributes.iter().map(|kv| (&kv.key, kv.value.as_ref())),
                    ),
                });
            }
        }
    }
    Ok(out)
}

fn decode_traces(bytes: &[u8]) -> Result<Vec<RenderedRecord>> {
    let req =
        ExportTraceServiceRequest::decode(bytes).context("decode ExportTraceServiceRequest")?;
    let mut out = Vec::new();
    for rs in req.resource_spans {
        let service_name = resource_service_name(rs.resource.as_ref());
        for ss in rs.scope_spans {
            for span in ss.spans {
                out.push(RenderedRecord {
                    time_unix_nano: span.start_time_unix_nano,
                    is_span: true,
                    service_name: service_name.clone(),
                    severity: "SPAN".to_string(),
                    body: span.name,
                    trace_id: hex::encode(&span.trace_id),
                    span_id: hex::encode(&span.span_id),
                    attributes: collect_attributes(
                        span.attributes
                            .iter()
                            .map(|kv| (&kv.key, kv.value.as_ref())),
                    ),
                });
            }
        }
    }
    Ok(out)
}

fn resource_service_name(resource: Option<&Resource>) -> String {
    resource
        .and_then(|r| r.attributes.iter().find(|kv| kv.key == "service.name"))
        .map(|kv| any_value_to_string(kv.value.as_ref()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Stringify a record's attributes for display, dropping call-site metadata
/// and anything with an empty value (a field that was recorded but never set —
/// printing `outcome=` adds width and no information).
fn collect_attributes<'a>(
    kvs: impl Iterator<Item = (&'a String, Option<&'a AnyValue>)>,
) -> Vec<(String, String)> {
    kvs.filter(|(k, _)| !is_unprinted_attr(k))
        .map(|(k, v)| (k.clone(), any_value_to_string(v)))
        .filter(|(_, v)| !v.is_empty())
        .collect()
}

fn any_value_to_string(v: Option<&AnyValue>) -> String {
    match v.and_then(|v| v.value.as_ref()) {
        Some(Value::StringValue(s)) => s.clone(),
        Some(Value::BoolValue(b)) => b.to_string(),
        Some(Value::IntValue(i)) => i.to_string(),
        Some(Value::DoubleValue(d)) => d.to_string(),
        Some(Value::BytesValue(b)) => hex::encode(b),
        Some(Value::ArrayValue(_)) | Some(Value::KvlistValue(_)) => "<complex>".to_string(),
        None => String::new(),
    }
}

/// OTel `SeverityNumber` ranges per the spec: 1-4 TRACE, 5-8 DEBUG, 9-12 INFO,
/// 13-16 WARN, 17-20 ERROR, 21-24 FATAL.
fn severity_number_name(n: i32) -> &'static str {
    match n {
        1..=4 => "TRACE",
        5..=8 => "DEBUG",
        9..=12 => "INFO",
        13..=16 => "WARN",
        17..=20 => "ERROR",
        21..=24 => "FATAL",
        _ => "UNKNOWN",
    }
}

/// Ordinal rank for `--min-severity` comparisons, matching the OTel range
/// ordering above. `SPAN` (a trace record, not a log) and anything
/// unrecognised sort below every real severity, so they're naturally excluded
/// by any `--min-severity` filter — see [`collect_all_records`].
fn severity_ordinal(s: &str) -> i32 {
    match s.to_ascii_uppercase().as_str() {
        "TRACE" => 1,
        "DEBUG" => 2,
        "INFO" => 3,
        "WARN" | "WARNING" => 4,
        "ERROR" => 5,
        "FATAL" => 6,
        _ => 0,
    }
}

/// Format one record as a single human-readable line, roughly matching the
/// old `tracing_subscriber::fmt` compact style.
pub fn format_line(r: &RenderedRecord) -> String {
    let ts = chrono::DateTime::from_timestamp(
        (r.time_unix_nano / 1_000_000_000) as i64,
        (r.time_unix_nano % 1_000_000_000) as u32,
    )
    .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
    .unwrap_or_else(|| "?".to_string());

    let mut line = format!("{ts} {:>7} [{}] {}", r.severity, r.service_name, r.body);
    // After the message, before `trace_id`: the fields are what the message is
    // about, and the trace id is a join key nobody reads inline.
    for (k, v) in &r.attributes {
        line.push_str(&format!(" {k}={v}"));
    }
    if !r.trace_id.is_empty() {
        line.push_str(&format!(" trace_id={}", r.trace_id));
    }
    line
}

// ─────────────────────────────────────────────────────────────────────────────
// `meridian logs` CLI
// ─────────────────────────────────────────────────────────────────────────────
//
// The direct replacement for the old bash `meridian logs` (which tailed
// launchd-redirected stdout/stderr text — see scripts/meridian-cli.sh's
// cmd_logs). This reads the same OTel spool every other piece of this
// architecture already uses, so there is exactly one place logs live, not two.
//
//   meridian logs [--service <name>] [--min-severity LEVEL] [-n N] [-f]
//     --service       filter to one service.name (e.g. meridian-rust, meridian-tray)
//     --min-severity  only show log records at/above this level (TRACE|DEBUG|
//                     INFO|WARN|ERROR|FATAL) — spans are excluded when this is
//                     set, matching the old `*-error.log` targets, which were
//                     WARN+ log text only, never span data. Omit for no filter.
//     -n N            show the last N records (default 200)
//     -f              follow: keep polling for new records every second

/// Collect every `.otlp` file in `pending/` (+ `sent/`, for a Dev/Bare install
/// that still has recently-shipped copies around) and decode them all.
fn collect_all_records(
    service_filter: Option<&str>,
    min_severity: Option<i32>,
) -> Result<Vec<RenderedRecord>> {
    let base = resolve_telemetry_dir()?;
    let mut records = Vec::new();
    for dir in [pending_dir(&base), sent_dir(&base)] {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "otlp") {
                continue;
            }
            records.extend(decode_file(&path));
        }
    }
    if let Some(svc) = service_filter {
        records.retain(|r| r.service_name.eq_ignore_ascii_case(svc));
    }
    if let Some(min) = min_severity {
        // Spans have no real severity (see `severity_ordinal`'s SPAN case,
        // ordinal 0) so any active min-severity filter excludes them too.
        records.retain(|r| severity_ordinal(&r.severity) >= min);
    }
    records.sort_by_key(|r| r.time_unix_nano);
    Ok(records)
}

/// Parse a `--min-severity` value into its ordinal, or exit with an error on
/// an unrecognised level — silently accepting garbage and filtering
/// everything out would look like "no logs" rather than a typo.
fn parse_min_severity(raw: &str) -> i32 {
    let ord = severity_ordinal(raw);
    if ord == 0 {
        eprintln!(
            "meridian logs: unrecognised --min-severity {raw:?} \
             (expected TRACE|DEBUG|INFO|WARN|ERROR|FATAL)"
        );
        std::process::exit(1);
    }
    ord
}

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

/// Identity key for a record, used to tell "already printed" apart from "new"
/// when two records share the exact same `time_unix_nano` (coarse clock
/// resolution, or several records stamped from one batch flush's single
/// `SystemTime::now()` read) — see the tie-break note in [`run`]'s follow loop.
fn record_key(r: &RenderedRecord) -> (u64, bool, &str, &str, &str, &str, &str) {
    (
        r.time_unix_nano,
        r.is_span,
        &r.service_name,
        &r.severity,
        &r.body,
        &r.trace_id,
        &r.span_id,
    )
}

/// Dispatch `meridian logs [--service <name>] [--min-severity LEVEL] [-n N] [-f]`.
pub async fn run(args: &[String]) {
    let service = flag_value(args, "--service");
    let min_severity = flag_value(args, "--min-severity").map(|v| parse_min_severity(&v));
    let n: usize = flag_value(args, "-n")
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let follow = args.iter().any(|a| a == "-f");

    let records = match collect_all_records(service.as_deref(), min_severity) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("meridian logs: {e}");
            std::process::exit(1);
        }
    };

    let mut last_time = 0u64;
    // Records already printed whose time_unix_nano == last_time — lets the
    // follow loop below tell "already shown" apart from "new at the same
    // timestamp" instead of relying on a bare `>` high-water mark, which
    // would permanently drop a same-nanosecond record split across a poll tick.
    let mut seen_at_last_time: std::collections::HashSet<(
        u64,
        bool,
        String,
        String,
        String,
        String,
        String,
    )> = std::collections::HashSet::new();
    for r in records
        .iter()
        .rev()
        .take(n)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        println!("{}", format_line(r));
        last_time = last_time.max(r.time_unix_nano);
    }
    for r in &records {
        if r.time_unix_nano == last_time {
            let (t, s, sv, se, b, ti, sp) = record_key(r);
            seen_at_last_time.insert((
                t,
                s,
                sv.to_string(),
                se.to_string(),
                b.to_string(),
                ti.to_string(),
                sp.to_string(),
            ));
        }
    }

    if !follow {
        return;
    }

    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let records = match collect_all_records(service.as_deref(), min_severity) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let new_records: Vec<&RenderedRecord> = records
            .iter()
            .filter(|r| {
                if r.time_unix_nano > last_time {
                    return true;
                }
                if r.time_unix_nano < last_time {
                    return false;
                }
                let (t, s, sv, se, b, ti, sp) = record_key(r);
                !seen_at_last_time.contains(&(
                    t,
                    s,
                    sv.to_string(),
                    se.to_string(),
                    b.to_string(),
                    ti.to_string(),
                    sp.to_string(),
                ))
            })
            .collect();
        for r in &new_records {
            println!("{}", format_line(r));
        }
        if let Some(max_new) = new_records.iter().map(|r| r.time_unix_nano).max() {
            last_time = last_time.max(max_new);
        }
        // Rebuild against the FULL current record set (not just newly
        // printed) so a record already seen in a prior tick but still at
        // last_time stays excluded.
        seen_at_last_time.clear();
        for r in &records {
            if r.time_unix_nano == last_time {
                let (t, s, sv, se, b, ti, sp) = record_key(r);
                seen_at_last_time.insert((
                    t,
                    s,
                    sv.to_string(),
                    se.to_string(),
                    b.to_string(),
                    ti.to_string(),
                    sp.to_string(),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::{
        common::v1::{any_value, InstrumentationScope, KeyValue},
        logs::v1::{LogRecord, ResourceLogs, ScopeLogs},
    };

    fn make_log_bytes(service_name: &str, severity: &str, body: &str) -> Vec<u8> {
        let req = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: Some(Resource {
                    attributes: vec![KeyValue {
                        key: "service.name".to_string(),
                        value: Some(AnyValue {
                            value: Some(any_value::Value::StringValue(service_name.to_string())),
                        }),
                    }],
                    dropped_attributes_count: 0,
                }),
                scope_logs: vec![ScopeLogs {
                    scope: Some(InstrumentationScope::default()),
                    log_records: vec![LogRecord {
                        time_unix_nano: 1_700_000_000_000_000_000,
                        severity_text: severity.to_string(),
                        body: Some(AnyValue {
                            value: Some(any_value::Value::StringValue(body.to_string())),
                        }),
                        ..Default::default()
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };
        req.encode_to_vec()
    }

    /// Build one log record carrying the given attributes, so the rendering of
    /// structured fields can be exercised without a live subscriber.
    fn make_log_bytes_with_attrs(body: &str, attrs: &[(&str, any_value::Value)]) -> Vec<u8> {
        let req = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: Some(Resource {
                    attributes: vec![KeyValue {
                        key: "service.name".to_string(),
                        value: Some(AnyValue {
                            value: Some(any_value::Value::StringValue("test-svc".to_string())),
                        }),
                    }],
                    dropped_attributes_count: 0,
                }),
                scope_logs: vec![ScopeLogs {
                    scope: Some(InstrumentationScope::default()),
                    log_records: vec![LogRecord {
                        time_unix_nano: 1_700_000_000_000_000_000,
                        severity_text: "INFO".to_string(),
                        body: Some(AnyValue {
                            value: Some(any_value::Value::StringValue(body.to_string())),
                        }),
                        attributes: attrs
                            .iter()
                            .map(|(k, v)| KeyValue {
                                key: (*k).to_string(),
                                value: Some(AnyValue {
                                    value: Some(v.clone()),
                                }),
                            })
                            .collect(),
                        ..Default::default()
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };
        req.encode_to_vec()
    }

    /// A record's structured fields must reach the rendered line.
    ///
    /// They were decoded and discarded before, which made `meridian logs` show
    /// the message of every record and none of its data — so `SIGTERM received`
    /// could not be attributed to a process and `ETL run failed` did not carry
    /// its error. `pid` is the concrete case this was added for: an `i64`, which
    /// must render as a plain number rather than a debug-formatted string.
    #[test]
    fn structured_fields_are_rendered_not_discarded() {
        let bytes = make_log_bytes_with_attrs(
            "SIGTERM received",
            &[("pid", any_value::Value::IntValue(4321))],
        );
        let records = decode_logs(&bytes).unwrap();
        let r = &records[0];
        assert_eq!(
            r.attributes,
            vec![("pid".to_string(), "4321".to_string())],
            "the log's structured fields must survive decoding"
        );
        let line = format_line(r);
        assert!(
            line.contains("SIGTERM received pid=4321"),
            "the fields must appear on the rendered line, right after the \
             message; got {line:?}"
        );
    }

    /// Call-site metadata is stamped on EVERY record by
    /// `experimental_metadata_attributes`. Printing it would bury the fields
    /// that were chosen deliberately under three that never vary in usefulness.
    #[test]
    fn call_site_metadata_is_not_printed() {
        let bytes = make_log_bytes_with_attrs(
            "shutting down",
            &[
                (
                    "code.filepath",
                    any_value::Value::StringValue("src/main.rs".into()),
                ),
                ("code.lineno", any_value::Value::IntValue(1547)),
                (
                    "code.namespace",
                    any_value::Value::StringValue("meridian".into()),
                ),
                (
                    "log.target",
                    any_value::Value::StringValue("meridian".into()),
                ),
                ("pid", any_value::Value::IntValue(99)),
            ],
        );
        let r = &decode_logs(&bytes).unwrap()[0];
        assert_eq!(
            r.attributes,
            vec![("pid".to_string(), "99".to_string())],
            "only the emitter's own fields should survive; got {:?}",
            r.attributes
        );
    }

    /// A near-miss on an exact-match key must NOT be suppressed.
    ///
    /// `log.target`, `busy_ns` and `idle_ns` are whole field names, not
    /// namespaces. When they were prefix-matched alongside `code.`/`thread.`,
    /// any emitter field that merely started the same way was silently hidden -
    /// which is the exact failure this rendering was added to fix, reintroduced
    /// by the filter meant to reduce noise.
    #[test]
    fn a_field_that_merely_starts_like_a_metadata_key_still_prints() {
        for key in ["log.target_override", "busy_ns_budget", "idle_ns_extra"] {
            assert!(
                !is_unprinted_attr(key),
                "{key} is an emitter field, not call-site metadata, and must be \
                 displayed"
            );
        }
        // ...while the exact keys and the real namespaces stay suppressed.
        for key in [
            "log.target",
            "busy_ns",
            "idle_ns",
            "code.filepath",
            "code.lineno",
            "thread.id",
            "thread.name",
        ] {
            assert!(is_unprinted_attr(key), "{key} is call-site metadata");
        }
    }

    /// A field declared on a span but never recorded arrives as an empty value.
    /// Rendering `outcome=` costs width and carries nothing.
    #[test]
    fn unset_fields_are_omitted() {
        let bytes = make_log_bytes_with_attrs(
            "daemon_lifecycle.stop",
            &[
                ("outcome", any_value::Value::StringValue(String::new())),
                ("reason", any_value::Value::StringValue("quit".to_string())),
            ],
        );
        let r = &decode_logs(&bytes).unwrap()[0];
        assert_eq!(
            r.attributes,
            vec![("reason".to_string(), "quit".to_string())]
        );
    }

    #[test]
    fn decode_logs_round_trips_service_severity_and_body() {
        let bytes = make_log_bytes("test-svc", "WARN", "hello from the spool");
        let records = decode_logs(&bytes).unwrap();
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(r.service_name, "test-svc");
        assert_eq!(r.severity, "WARN");
        assert_eq!(r.body, "hello from the spool");
        assert!(!r.is_span);
    }

    #[test]
    fn decode_file_reads_a_real_spooled_logs_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let bytes = make_log_bytes("meridian-rust", "INFO", "daemon started");
        let path = dir.path().join("logs-123-0.otlp");
        std::fs::write(&path, &bytes).unwrap();

        let records = decode_file(&path);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].service_name, "meridian-rust");
        assert_eq!(records[0].body, "daemon started");
    }

    #[test]
    fn decode_file_ignores_unknown_filenames() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("garbage.otlp");
        std::fs::write(&path, b"not otlp").unwrap();
        assert!(decode_file(&path).is_empty());
    }

    #[test]
    fn decode_file_returns_empty_on_corrupt_bytes() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("logs-1-0.otlp");
        std::fs::write(&path, b"not a valid protobuf payload at all").unwrap();
        assert!(decode_file(&path).is_empty());
    }

    #[test]
    fn severity_number_name_covers_all_ranges() {
        assert_eq!(severity_number_name(1), "TRACE");
        assert_eq!(severity_number_name(8), "DEBUG");
        assert_eq!(severity_number_name(9), "INFO");
        assert_eq!(severity_number_name(16), "WARN");
        assert_eq!(severity_number_name(20), "ERROR");
        assert_eq!(severity_number_name(24), "FATAL");
        assert_eq!(severity_number_name(0), "UNKNOWN");
    }
}
