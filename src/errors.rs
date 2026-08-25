//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

//! One way to render an error for a log line: its **full `anyhow` source chain**.
//!
//! # Why this exists
//! `tracing::warn!(error = %e, …)` renders only the **outermost** `.context()`.
//! Everything under it - the actual cause - is dropped before the record is
//! written, and therefore before the redacted WARN+ ship leg sends it anywhere.
//! On a developer's machine that is a readability annoyance. On the fleet it
//! deletes the only evidence there is, because `meridian logs` and central
//! OpenObserve both read what `tracing` recorded, not what the error contained.
//!
//! Measured on 2026-08-17 while trying to date a recurring
//! `(code: 11) database disk image is malformed` across 8 affected hosts: the
//! daemon's DB failures arrived as bare context strings - `fetch pending
//! summaries` x27,591, `upsert coding-agent segment` x29,159 - with no SQLite
//! code anywhere. It was impossible to tell corruption from a missing table
//! from a locked file, and impossible to say when any host's corruption began.
//! One host read as a clean "corrupted with no update" case purely because a
//! *different* component (the tray, which does render chains) happened to start
//! reporting on a later day.
//!
//! This is the daemon-side counterpart of the tray's `cmd_err!` /`with_causes`
//! (PR #704), which fixed exactly this defect for `#[tauri::command]` results.
//! The daemon was never covered.
//!
//! # Who calls this
//! Every daemon site that logs a failed database operation. The rule is enforced
//! for the converted modules by `no_bare_error_display_in_db_paths` in
//! `src/errors.rs`'s test module.
//!
//! # Related
//! [`crate::intelligence::providers::http::classify::chain`] - the original,
//! provider-scoped version this generalises; it now delegates here so there is
//! still exactly one place `{e:#}` is written.

/// Format an error as its FULL `anyhow` source chain.
///
/// Use in place of a bare `%e` wherever the cause is what a reader would need:
///
/// ```ignore
/// // before - renders "fetch pending summaries", cause discarded
/// tracing::warn!(error = %e, "summariser: fetch_pending failed");
/// // after  - renders "fetch pending summaries: (code: 11) database disk image is malformed"
/// tracing::warn!(error = %chain(&e), "summariser: fetch_pending failed");
/// ```
///
/// Takes `&anyhow::Error` rather than a generic `impl Display` deliberately: the
/// whole point is the `chain()` walk, and a generic would silently accept a
/// `sqlx::Error` or a `String` and format it identically to `%e`, reintroducing
/// the bug at a call site that looks fixed.
pub fn chain(err: &anyhow::Error) -> String {
    format!("{err:#}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    /// The whole point: a `.context()` layer must not hide what it wraps.
    #[test]
    fn the_cause_survives_a_context_layer() {
        let err = Err::<(), _>(anyhow::anyhow!(
            "(code: 11) database disk image is malformed"
        ))
        .context("fetch pending summaries")
        .unwrap_err();

        // What the old `%e` rendered, and why it was useless on the fleet.
        assert_eq!(format!("{err}"), "fetch pending summaries");

        let rendered = chain(&err);
        assert!(
            rendered.contains("fetch pending summaries")
                && rendered.contains("(code: 11) database disk image is malformed"),
            "the chain must carry BOTH the context and its cause, got {rendered:?}"
        );
    }

    /// Deeper nesting is what the DB paths actually produce - sqlx error inside
    /// a query context inside a caller context.
    #[test]
    fn every_layer_of_a_deep_chain_survives() {
        let err = Err::<(), _>(anyhow::anyhow!("disk I/O error"))
            .context("read pm_worklog_hours")
            .context("worklog: hour rollup")
            .unwrap_err();

        let rendered = chain(&err);
        for layer in [
            "worklog: hour rollup",
            "read pm_worklog_hours",
            "disk I/O error",
        ] {
            assert!(
                rendered.contains(layer),
                "{layer:?} missing from {rendered:?}"
            );
        }
    }

    /// Does this source line log an error in a way that DROPS its cause?
    ///
    /// Extracted from the scan so the scan's own coverage is testable. That is
    /// not ceremony: the previous version matched only `error = %e`, `main.rs`
    /// was added to `CONVERTED` while three of its sites used a positional
    /// format placeholder instead, and the test passed - recording coverage it
    /// did not provide. A predicate nothing exercises is indistinguishable from
    /// one that works.
    ///
    /// Two spellings, both rendering only the outermost `.context()`:
    /// - `error = %e`
    /// - `tracing::error!("… failed: {}", e)` - which also breaks the repo's
    ///   "structured fields, no format strings for data values" rule.
    ///
    /// Only the POSITIONAL `{}` form counts. Inline named captures
    /// (`"{label}: timed out"`) are pervasive, usually name one of our own
    /// static labels, and sweeping them belongs in its own change.
    fn drops_the_cause(line: &str) -> bool {
        if line.trim_start().starts_with("//") {
            return false;
        }
        line.contains("error = %e") || (line.contains("tracing::") && line.contains("{}\", "))
    }

    /// The scan must catch BOTH spellings, and must not fire on the fixed form.
    #[test]
    fn the_scan_catches_every_cause_dropping_spelling() {
        assert!(drops_the_cause(
            r#"tracing::warn!(error = %e, "fetch failed");"#
        ));
        assert!(drops_the_cause(
            r#"tracing::error!("cleanup_incomplete_runs failed: {}", e),"#
        ));
        assert!(
            drops_the_cause(
                r#"        Err(e) => tracing::error!("intelligence run failed: {}", e),"#
            ),
            "the exact spelling that slipped past the original scan"
        );

        // The fixed form, and things that must not trip it.
        assert!(!drops_the_cause(
            r#"tracing::warn!(error = %meridian::errors::chain(&e), "fetch failed");"#
        ));
        assert!(!drops_the_cause(r#"// tracing::warn!(error = %e, "…");"#));
        assert!(
            !drops_the_cause(r#"tracing::warn!(bin = %bin, "{label}: timed out");"#),
            "inline named captures are out of scope - see drops_the_cause's doc"
        );
    }

    /// The database modules converted by this change must not reintroduce an
    /// **unjustified** bare `%e`, which is how the fleet lost its corruption
    /// evidence in the first place.
    ///
    /// `%e` is still correct where the value is not an `anyhow::Error` - a
    /// `JoinError`, a `sqlx::Error`, a typed enum - because there is no source
    /// chain to walk and `%e` already renders the whole thing. Those sites carry
    /// a `not-anyhow:` marker stating so, which is the only exemption. That is
    /// deliberate: an unexplained `%e` is indistinguishable from the bug, so the
    /// marker forces whoever writes one to have made the distinction on purpose.
    /// [`chain`] takes `&anyhow::Error` rather than `impl Display` for the same
    /// reason - the compiler finds these sites instead of formatting them
    /// identically to the bug.
    ///
    /// Scoped to the DB paths rather than the whole crate on purpose: ~120 sites
    /// elsewhere (the LLM probes, the telemetry spool, uninstall) still use
    /// `%e`, and failing the build on those would force an unrelated sweep into
    /// this change. Widening this list is the follow-up.
    ///
    /// Source-scanned because the defect is invisible at runtime - the record is
    /// emitted, well-formed, just missing its cause. Nothing observes it.
    #[test]
    fn no_bare_error_display_in_db_paths() {
        // (path, source) pairs. `include_str!` needs a literal, so this cannot
        // be a directory walk.
        const CONVERTED: &[(&str, &str)] = &[
            (
                "coding_agent_session_ingest/indexer.rs",
                include_str!("coding_agent_session_ingest/indexer.rs"),
            ),
            (
                "coding_agent_session_ingest/summariser/mod.rs",
                include_str!("coding_agent_session_ingest/summariser/mod.rs"),
            ),
            (
                "coding_agent_session_ingest/sources/mod.rs",
                include_str!("coding_agent_session_ingest/sources/mod.rs"),
            ),
            ("intelligence/mod.rs", include_str!("intelligence/mod.rs")),
            // Added 2026-08-24. `main.rs` carries the single highest-value
            // error site in the daemon: the startup DB open, whose own comment
            // says it exists so a daemon that cannot open its database "is
            // finally diagnosable in central telemetry instead of crash-looping
            // silently" - and which then used `%e` and shipped
            // `failed to open SQLite at <url>` with no cause under it.
            //
            // Measured the same day: 73,489 of those records across 58 hosts in
            // 7 days (7.6% of the reporting fleet), one host logging 12,267 -
            // about one every 50s, launchd `KeepAlive` against a database it
            // will never open. 28 of the 58 also show `code: 11`; the other 30
            // show no corruption at all and are indistinguishable from them,
            // because the one field that would tell them apart was dropped.
            ("main.rs", include_str!("main.rs")),
        ];

        for (path, src) in CONVERTED {
            // Truncate at the test module: a converted file's own tests may
            // legitimately format errors, and this file scans itself.
            let prod = src.split_once("\n#[cfg(test)]").map_or(*src, |(a, _)| a);
            let offenders: Vec<&str> = prod
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .filter(|l| drops_the_cause(l))
                .filter(|l| !l.contains("not-anyhow:"))
                .map(str::trim)
                .collect();
            assert!(
                offenders.is_empty(),
                "{path} logs a bare `error = %e`, which renders only the outermost \
                 .context() and drops the cause before it can egress. Use \
                 `error = %crate::errors::chain(&e)` - or, if the value is not an \
                 `anyhow::Error` and so has no chain to walk, keep `%e` and say why \
                 with a `// not-anyhow: …` comment on the same line. This scan \
                 matches BOTH spellings - `error = %e` and a positional `{{}}` \
                 format placeholder - because `main.rs` was added to this list \
                 while three of its sites used the second one, so the entry \
                 recorded coverage the scan did not provide. \
                 Offending line(s): {offenders:#?}"
            );
        }
    }
}
