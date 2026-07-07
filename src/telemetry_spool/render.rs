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
//   meridian logs [--service <name>] [-n N] [-f]
//     --service   filter to one service.name (e.g. meridian-rust, meridian-mlx-server)
//     -n N        show the last N records (default 200)
//     -f          follow: keep polling for new records every second

/// Collect every `.otlp` file in `pending/` (+ `sent/`, for a Dev/Bare install
/// that still has recently-shipped copies around) and decode them all.
fn collect_all_records(service_filter: Option<&str>) -> Result<Vec<RenderedRecord>> {
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
    records.sort_by_key(|r| r.time_unix_nano);
    Ok(records)
}

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

/// Dispatch `meridian logs [--service <name>] [-n N] [-f]`.
pub async fn run(args: &[String]) {
    let service = flag_value(args, "--service");
    let n: usize = flag_value(args, "-n")
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let follow = args.iter().any(|a| a == "-f");

    let records = match collect_all_records(service.as_deref()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("meridian logs: {e}");
            std::process::exit(1);
        }
    };

    let mut last_time = 0u64;
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

    if !follow {
        return;
    }

    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let records = match collect_all_records(service.as_deref()) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let new_records: Vec<&RenderedRecord> = records
            .iter()
            .filter(|r| r.time_unix_nano > last_time)
            .collect();
        for r in new_records {
            println!("{}", format_line(r));
            last_time = last_time.max(r.time_unix_nano);
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
