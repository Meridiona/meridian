//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//!
//! Every `record("otel.status_code", …)` must have a span that DECLARED the
//! field.
//!
//! `tracing` resolves `Span::record` against the field set fixed when the span
//! was created. Recording a name that is not in that set is not an error and
//! not a warning - it is a silent no-op. So this compiles, runs, and writes
//! nothing:
//!
//! ```ignore
//! #[tracing::instrument(skip(app))]                    // <- no `fields(…)`
//! async fn resize(app: AppHandle) -> Result<(), String> {
//!     …
//!     tracing::Span::current().record("otel.status_code", "ERROR");  // dropped
//!     tracing::warn!(error = %e, "resize failed");                   // kept
//! }
//! ```
//!
//! The WARN still ships, which is what makes it survive review: the failure is
//! visible in `meridian logs` and in central OO's log stream, so the code looks
//! instrumented from every angle except the one that matters. What is missing
//! is the SPAN STATUS, and span status is what an error-only telemetry query
//! filters on - the exact query someone runs when a user reports a problem and
//! there is nothing to grep for.
//!
//! # Why a derived scan and not a review habit
//!
//! This shipped in 1.88.0 in `commands::system::resize_setup_window`, in the
//! same commit that ADDED the declaration to `poll::permissions::check_permissions`
//! one file away. Knowing the rule was not enough; neither was a reviewer who
//! had just applied it. Nothing else could have caught it - it type-checks,
//! clippy is happy, and no unit test can observe a dropped field without a
//! subscriber harness around every call site.
//!
//! So the list is DERIVED from the source at test time rather than enumerated
//! here. A site added tomorrow, in a file that does not exist yet, is covered
//! the day it lands; an enumerated list would have to be remembered, which is
//! the thing that already failed.
//!
//! # What it can and cannot decide
//!
//! Recording on a span the current function OWNS is checkable locally: the
//! declaration has to be right there, in the `#[tracing::instrument]` attribute
//! or in the `span!` invocation the receiver was bound from.
//!
//! Recording on an INHERITED span is not. `Span::current()` inside a function
//! with no span of its own targets whatever ancestor is on the stack - that is
//! a deliberate pattern here (`poll::permissions::check_bool_permission` marks
//! its caller's `check_permissions` span; `commands::setup::mark_span_failed`
//! exists only to do this), and the declaring span lives in a different
//! function, sometimes a different file. Those are counted and skipped rather
//! than guessed at, because a guess would produce false failures on correct
//! code, and a guard that cries wolf gets deleted.
//!
//! At the time of writing that is 19 checkable sites and 23 inherited ones.
//!
//! # Related
//!
//! - [`crate::observability`] - where the spans this guards are exported from.
//! - `tray/src-tauri/src/poll/watchdog.rs` - the reference shape for a failure
//!   boundary: declare the field, record `ERROR`, log a structured `warn!`.

use std::path::{Path, PathBuf};

/// Crate source roots, relative to the workspace root.
///
/// `CARGO_MANIFEST_DIR` for this package IS the workspace root (the repo root
/// is itself a package - see CLAUDE.md's `--workspace` warning), so these paths
/// need no `../..` climbing and stay correct wherever the checkout lives.
const ROOTS: [&str; 3] = ["src", "meridian-core/src", "tray/src-tauri/src"];

/// One `record("otel.status_code", …)` call found in the source.
struct RecordSite {
    file: String,
    line: usize,
    /// The enclosing function's signature line, for the failure message.
    func: String,
    /// Whether the span being recorded onto belongs to the enclosing function.
    /// `false` means an ancestor's span - see the module header.
    owns_span: bool,
    /// Whether that span declared `otel.status_code`. Meaningless when
    /// `owns_span` is false.
    declared: bool,
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// True when `line` starts a free-function signature at the top level of a
/// file, in any visibility spelling used in this workspace.
fn starts_fn(line: &str) -> bool {
    let rest = line
        .strip_prefix("pub(crate) ")
        .or_else(|| line.strip_prefix("pub(super) "))
        .or_else(|| line.strip_prefix("pub "))
        .unwrap_or(line);
    let rest = rest.strip_prefix("async ").unwrap_or(rest);
    rest.starts_with("fn ")
}

/// Every record site in the workspace, classified.
fn record_sites() -> Vec<RecordSite> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for r in ROOTS {
        rust_files(&root.join(r), &mut files);
    }
    files.sort();

    let mut sites = Vec::new();
    for file in files {
        // This file quotes the very calls it looks for - in a doc example, in
        // panic messages, in the classifier's own string literals. Scanning it
        // would report its own prose as un-instrumented production code. It is
        // `#[cfg(test)]` in its entirety, so there is no real instrumentation
        // here to lose by skipping it.
        if file.ends_with("span_status_guard.rs") {
            continue;
        }
        let whole = std::fs::read_to_string(&file).expect("source file is readable");
        // Truncate at the test module. Test code quotes these very strings (this
        // file is the extreme case - it would otherwise scan itself and match
        // its own examples), and a needle that matches the scanner's own
        // literals is a guard that can never fail. Same trap as the
        // `include_str!` self-scans elsewhere in the tree.
        let src = match whole.find("\n#[cfg(test)]") {
            Some(at) => &whole[..at],
            None => &whole[..],
        };
        let lines: Vec<&str> = src.lines().collect();
        let rel = file
            .strip_prefix(root)
            .unwrap_or(&file)
            .to_string_lossy()
            .into_owned();

        for (i, line) in lines.iter().enumerate() {
            let Some(at) = line.find(".record(\"otel.status_code\"") else {
                continue;
            };
            // Two receiver shapes: `tracing::Span::current().record(…)`, and a
            // local bound from a `span!` macro (`span.record(…)`). Only the
            // second is an identifier, so `Span::current()` is matched on the
            // whole prefix - reading backwards for a "word" would stop at its
            // closing paren and mistake it for a named span.
            let before = &line[..at];
            let receiver: String = before
                .chars()
                .rev()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();

            let fn_line = (0..=i).rev().find(|&j| starts_fn(lines[j]));
            let Some(fn_line) = fn_line else {
                panic!("{rel}:{} is not inside a top-level fn - the scan cannot classify it, so it would be skipped silently", i + 1);
            };
            // The contiguous attribute block above the signature.
            let mut k = fn_line;
            while k > 0 && !lines[k - 1].trim().is_empty() && !lines[k - 1].starts_with("//") {
                k -= 1;
            }
            let attrs = lines[k..fn_line].join("\n");
            let body = lines[fn_line..i].join("\n");

            // `let x = tracing::Span::current();` makes `x` an alias for the
            // current span, so `x.record(…)` is the same case as recording on
            // `Span::current()` directly - not a span this function declared.
            let is_alias = body
                .split("let ")
                .skip(1)
                .filter_map(|s| s.split_once(" = tracing::Span::current();"))
                .any(|(name, _)| name == receiver);
            let on_current = before.ends_with("Span::current()") || is_alias;

            let (owns_span, declared) = if on_current {
                // Own span only if this fn is instrumented; otherwise the
                // record lands on an ancestor's span, declared elsewhere.
                (
                    attrs.contains("#[tracing::instrument"),
                    attrs.contains("otel.status_code"),
                )
            } else {
                // A named receiver was bound from a `span!` invocation in this
                // body, so the declaration has to be in this body.
                (
                    true,
                    body.contains("otel.status_code = tracing::field::Empty"),
                )
            };

            sites.push(RecordSite {
                file: rel.clone(),
                line: i + 1,
                func: lines[fn_line].trim().to_string(),
                owns_span,
                declared,
            });
        }
    }
    sites
}

/// Sites the scan could find when this was written. A floor, not a target: it
/// exists so that renaming `record` or `otel.status_code`, or breaking the
/// walk, fails HERE rather than leaving every assertion below passing over an
/// empty list - which is the usual way a source scan stops testing anything
/// without anyone noticing.
const SITES_WHEN_WRITTEN: usize = 42;

#[test]
fn every_recorded_span_status_was_declared() {
    let sites = record_sites();
    assert!(
        sites.len() >= SITES_WHEN_WRITTEN,
        "found only {} record sites, expected at least {SITES_WHEN_WRITTEN} - \
         the scan itself is broken (renamed field? moved crate root?), and a \
         broken scan passes every check below",
        sites.len()
    );

    let dead: Vec<String> = sites
        .iter()
        .filter(|s| s.owns_span && !s.declared)
        .map(|s| format!("  {}:{}  in `{}`", s.file, s.line, s.func))
        .collect();

    assert!(
        dead.is_empty(),
        "these `record(\"otel.status_code\", …)` calls are SILENT NO-OPS - the \
         span they record onto never declared the field, so the span status is \
         never set and an error-only telemetry query cannot see the failure at \
         all:\n{}\n\nFix: add `fields(otel.status_code = tracing::field::Empty)` \
         to the fn's `#[tracing::instrument(…)]`, or to the `span!` the \
         receiver was bound from.",
        dead.join("\n")
    );
}

#[test]
fn the_scan_still_finds_both_kinds_of_site() {
    // The check above passes vacuously if the classifier decides every site is
    // inherited. Both populations are real and neither should empty out
    // quietly.
    let sites = record_sites();
    let owned = sites.iter().filter(|s| s.owns_span).count();
    let inherited = sites.len() - owned;
    assert!(
        owned >= 15,
        "only {owned} sites classified as owning their span - the classifier \
         is now skipping work it used to check"
    );
    assert!(
        inherited >= 15,
        "only {inherited} sites classified as inheriting a caller's span - \
         suspicious, since that is a deliberate pattern in the tray's poll loop"
    );
}
