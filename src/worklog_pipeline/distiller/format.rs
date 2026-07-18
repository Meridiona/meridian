//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Stage 10 — render the selected spans into the compressed body.
//!
//! Ports the thread-grouping + block-formatting tail of `session_distiller._distil`.
//! Consecutive spans sharing an (app, window) become one thread; each thread renders a
//! header line and its (220-char-capped) span lines. A header summarises the window:
//! session count, active minutes, local start-end, and the app timeline.
//!
//! The body is model INPUT (and a diagnostic shown in the hour-detail panel), not UI
//! chrome — so the middot/en-dash separators are kept byte-for-byte identical to the
//! Python output for distiller parity.

use super::Span;

/// Render the (already selected) spans plus the header. `selected` is sorted by
/// `(time, sid)` here. `span_start`/`span_end` are the local `HH:MM` of the hour's first
/// and last session; `active_mins` is the summed session minutes; `header_prefix` is
/// e.g. `"HOUR 09:00"`.
pub fn render(
    mut selected: Vec<Span>,
    header_prefix: &str,
    nsess: usize,
    active_mins: i64,
    span_start: &str,
    span_end: &str,
) -> String {
    selected.sort_by(|a, b| (a.t.as_str(), a.sid).cmp(&(b.t.as_str(), b.sid)));

    // Group consecutive spans by (app, window) into threads.
    struct Thread {
        app: String,
        win: String,
        t0: String,
        spans: Vec<Span>,
    }
    let mut threads: Vec<Thread> = Vec::new();
    for sp in selected {
        match threads.last_mut() {
            Some(t) if t.app == sp.app && t.win == sp.win => t.spans.push(sp),
            _ => threads.push(Thread {
                app: sp.app.clone(),
                win: sp.win.clone(),
                t0: sp.t.clone(),
                spans: vec![sp],
            }),
        }
    }

    let app_timeline = threads
        .iter()
        .map(|t| t.app.as_str())
        .collect::<Vec<_>>()
        .join(" → ");

    let blocks: Vec<String> = threads
        .iter()
        .map(|thread| {
            let win = if thread.win.is_empty() {
                "—"
            } else {
                thread.win.as_str()
            };
            let hdr = format!("\n[{} · {} · {}]", thread.t0, thread.app, win);
            let mut lines = vec![hdr];
            for sp in &thread.spans {
                let capped: String = sp.line.chars().take(220).collect();
                lines.push(format!("  {capped}"));
            }
            lines.join("\n")
        })
        .collect();

    let body_header = format!(
        "=== {header_prefix} · {nsess} sessions · {active_mins} min active · {span_start}–{span_end} ===\n\
         app timeline: {app_timeline}"
    );
    format!("{body_header}\n{}", blocks.join("\n"))
}
