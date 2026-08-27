//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity

//! Build-time lints on what a log record is allowed to CONTAIN.
//!
//! # Why this is a module and not a comment in a style guide
//! `telemetry_spool::redact` filters attributes by key: anything not on
//! `SAFE_STRING_KEYS` is dropped before the ship leg. The **message body** has
//! no such gate - it *is* the record, so it always ships, having passed only
//! `scrub_text`'s URL/email/blob patterns, which know nothing about ticket keys,
//! window titles, or a tracker's error payload.
//!
//! CLAUDE.md has said "structured fields - never format data values into the
//! message string" since the beginning, and nothing checked it. It drifted at
//! three tray call sites that spliced a `meridian` subprocess's stderr into a
//! WARN body, and issue #872 measured the result in central OpenObserve: a real
//! user's ticket key (`ENG-7041`), from a value the allowlist denies as an
//! attribute. A rule nothing enforces is a rule that decays, which is the whole
//! argument for putting it here instead.
//!
//! The lint is source-scanning because the defect is invisible at runtime - the
//! record is emitted, well-formed, and carries exactly what it was told to.
//! Nothing observes the difference until it is already on someone else's server.
//!
//! # Who calls this
//! Nothing at runtime. `cargo test` runs [`tests::no_user_data_interpolated_into_a_log_body`]
//! over every workspace source tree, and it fails the build for the whole
//! workspace, not just this crate.
//!
//! # Related
//! - [`crate::errors`] - the sibling log-hygiene lint, on the opposite failure:
//!   `no_bare_error_display_in_db_paths` catches a cause being DROPPED, this
//!   catches user data being ADDED. Both exist because a well-formed log line
//!   is not a correct one.
//! - `crate::telemetry_spool::redact` - the attribute-side boundary this one
//!   complements, and the reason a body needs its own rule at all.

#[cfg(test)]
mod tests {

    /// The identifiers a WARN+/ERROR **message body** may interpolate.
    ///
    /// # Why this is an allowlist and not "don't do that"
    /// A log body is the one field `telemetry_spool::redact` structurally cannot
    /// filter. Attributes are dropped unless their key is on
    /// `SAFE_STRING_KEYS`; the body IS the record, so it always ships. Anything
    /// interpolated into it egresses verbatim, having passed only `scrub_text`'s
    /// URL/email/blob patterns - which know nothing about ticket keys, window
    /// titles, or a tracker's error payload.
    ///
    /// Issue #872 measured the consequence: a real user's ticket key
    /// (`ENG-7041`) in central OpenObserve, spliced in from a `meridian`
    /// subprocess's stderr by three tray call sites, from a value the allowlist
    /// denies as an attribute. CLAUDE.md has forbidden this since the beginning
    /// - "structured fields - never format data values into the message string"
    /// - and nothing enforced it, so it drifted three times.
    ///
    /// # Adding an identifier
    /// It must be provably a compile-time constant or a closed set of our own
    /// literals at EVERY call site, and it must earn a justification here. If
    /// the value is runtime data, it belongs in a structured field instead,
    /// where the allowlist can make a deliberate decision about it - which is
    /// the entire point of having one.
    const INTERPOLABLE: &[&str] = &[
        // A subcommand/operation label. `&'static str` at every call site
        // (`"ticket-statuses"`, `"plan-task-draft"`, …) - it names OUR
        // subcommand, never the user's data.
        "label",
        // `catch_setup_panic`'s stage name - a `&str` literal at both call
        // sites, naming a tray setup step.
        "what",
        // A panic payload, from `catch_setup_panic`. Deliberately still in the
        // body: it is our own `expect`/`panic!` text and it is THE diagnostic
        // for a startup panic, so moving it to an unallowlisted attribute would
        // delete it from the fleet's telemetry entirely - #867's mistake, not
        // #872's. It already ships today and this change does not widen that;
        // the rename to `panic_msg` exists so the exemption is visible at the
        // call site rather than riding on the generic name `msg`, which is what
        // all three #872 sites used.
        "panic_msg",
    ];

    /// The source trees this scans. A directory WALK, not an `include_str!`
    /// list: the defect is a new call site, so a "remember to add your file"
    /// list would be exactly the shape of hole issue #878 was about. Every
    /// crate in the workspace that emits telemetry is covered.
    const SCANNED_TREES: &[&str] = &[
        "src",
        "meridian-core/src",
        "meridian-oauth/src",
        "tray/src-tauri/src",
    ];

    /// Collect `(path, production source)` for every `.rs` under `SCANNED_TREES`,
    /// truncated at the file's test module - a file's own tests legitimately
    /// contain example log lines, and this scan reads the file it lives in.
    fn production_sources() -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs")
                    // A dedicated test file (`tests.rs`, `cli_exec_tests.rs`)
                    // has no in-file `#[cfg(test)]` to truncate at, so the
                    // marker heuristic below would read all of it as
                    // production and flag its example log lines.
                    && !p.file_name().is_some_and(|n| {
                        let n = n.to_string_lossy();
                        n == "tests.rs" || n.ends_with("_tests.rs")
                    })
                {
                    if let Ok(src) = std::fs::read_to_string(&p) {
                        let prod = src
                            .split_once("\n#[cfg(test)]")
                            .map_or(src.as_str(), |(a, _)| a)
                            .to_string();
                        out.push((p.display().to_string(), prod));
                    }
                }
            }
        }
        // Set at COMPILE time to this crate's directory, which is the workspace
        // root (`members = [".", …]`), so the walk is rooted correctly no matter
        // where the test binary is invoked from.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut out = Vec::new();
        for tree in SCANNED_TREES {
            walk(&root.join(tree), &mut out);
        }
        assert!(
            out.len() > 100,
            "the source walk found only {} files - SCANNED_TREES is stale or the \
             layout moved, and a scan that reads nothing passes silently",
            out.len()
        );
        out
    }

    /// Every `{ident}` interpolated into the message body of a `tracing::warn!`
    /// or `tracing::error!` in `src`, with its line number.
    fn interpolated_body_idents(src: &str) -> Vec<(usize, String, String)> {
        let mut found = Vec::new();
        // Matched WITHOUT the `tracing::` prefix on purpose: 14 sites in
        // `etl/` and `capture/` import the macro (`use tracing::warn`) and call
        // it bare, and a scan anchored on the qualified spelling would have read
        // as full coverage while silently skipping every one of them.
        for macro_name in ["warn!", "error!"] {
            let mut from = 0;
            while let Some(rel) = src[from..].find(macro_name) {
                let start = from + rel;
                from = start + macro_name.len();
                // Not part of a longer identifier (`some_error!`, `debug_warn!`).
                if src[..start]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
                {
                    continue;
                }
                // Skip a commented-out example.
                let line_start = src[..start].rfind('\n').map_or(0, |i| i + 1);
                if src[line_start..start].trim_start().starts_with("//") {
                    continue;
                }
                let Some(open) = src[start..].find('(').map(|i| start + i) else {
                    continue;
                };
                // Balance parens, ignoring anything inside a string literal.
                let (mut depth, mut i) = (0usize, open);
                let (mut in_str, mut esc) = (false, false);
                let bytes: Vec<char> = src[open..].chars().collect();
                let mut k = 0usize;
                while k < bytes.len() {
                    let c = bytes[k];
                    if in_str {
                        if esc {
                            esc = false;
                        } else if c == '\\' {
                            esc = true;
                        } else if c == '"' {
                            in_str = false;
                        }
                    } else if c == '"' {
                        in_str = true;
                    } else if c == '(' {
                        depth += 1;
                    } else if c == ')' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    k += 1;
                }
                i += k;
                let args = &src[open + 1..i.min(src.len())];
                // The message is the LAST top-level string literal in the arg
                // list - `tracing`'s own rule.
                let Some(msg) = last_string_literal(args) else {
                    continue;
                };
                let line = src[..start].matches('\n').count() + 1;
                for ident in placeholders(&msg) {
                    found.push((line, ident, msg.clone()));
                }
            }
        }
        found
    }

    /// The last `"…"` literal in a macro argument list, unescaped enough to read
    /// its `{}` placeholders.
    fn last_string_literal(args: &str) -> Option<String> {
        let chars: Vec<char> = args.chars().collect();
        let (mut i, mut last) = (0usize, None);
        while i < chars.len() {
            if chars[i] == '"' {
                let mut j = i + 1;
                let mut buf = String::new();
                while j < chars.len() {
                    if chars[j] == '\\' {
                        j += 2;
                        continue;
                    }
                    if chars[j] == '"' {
                        break;
                    }
                    buf.push(chars[j]);
                    j += 1;
                }
                last = Some(buf);
                i = j + 1;
            } else {
                i += 1;
            }
        }
        last
    }

    /// Named `{ident}` captures in a format string. Positional `{}` / `{:?}` are
    /// out of scope: they take their value from a trailing argument, and
    /// `no_bare_error_display_in_db_paths` already covers the `%e` case that
    /// produces in practice.
    fn placeholders(msg: &str) -> Vec<String> {
        let mut out = Vec::new();
        let chars: Vec<char> = msg.chars().collect();
        let mut i = 0usize;
        while i < chars.len() {
            if chars[i] == '{' {
                if chars.get(i + 1) == Some(&'{') {
                    i += 2;
                    continue;
                }
                let mut j = i + 1;
                let mut name = String::new();
                while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                    name.push(chars[j]);
                    j += 1;
                }
                // Only a NAMED capture: `{x}` or `{x:?}`, not `{}` or `{:?}`.
                if !name.is_empty()
                    && !name.starts_with(|c: char| c.is_ascii_digit())
                    && matches!(chars.get(j), Some('}') | Some(':'))
                {
                    out.push(name);
                }
                i = j;
            } else {
                i += 1;
            }
        }
        out
    }

    /// A WARN+ log BODY always ships - it is the one field the redaction
    /// allowlist cannot reach - so nothing runtime-valued may be formatted into
    /// it. See [`INTERPOLABLE`] for the full reasoning and issue #872 for the
    /// leak that prompted this.
    #[test]
    fn no_user_data_interpolated_into_a_log_body() {
        let mut offenders = Vec::new();
        for (path, src) in production_sources() {
            for (line, ident, msg) in interpolated_body_idents(&src) {
                if !INTERPOLABLE.contains(&ident.as_str()) {
                    offenders.push(format!("{path}:{line} - `{{{ident}}}` in \"{msg}\""));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "a WARN+/ERROR message body interpolates a value that is not on \
             `INTERPOLABLE`. The body is the ONE field `telemetry_spool::redact` \
             cannot filter - attributes are dropped unless allowlisted, but the \
             body always ships - so whatever this formats in egresses verbatim \
             to central OpenObserve. Issue #872 measured a real user's ticket key \
             arriving this way. Put the value in a structured field instead \
             (`tracing::warn!(some_key = %v, \"static message\")`), where the \
             allowlist can make a deliberate decision about it; if it is genuinely \
             a compile-time literal at every call site, add it to `INTERPOLABLE` \
             with a justification. Offenders: {offenders:#?}"
        );
    }

    /// The scan above is only worth having if it actually catches the #872
    /// shape. Feeds it the pre-fix source verbatim - a scan nobody has seen fail
    /// is a scan nobody knows works.
    #[test]
    fn the_body_scan_catches_the_splice_it_was_written_for() {
        let pre_fix = r#"
            tracing::warn!(
                bin = %bin,
                code = ?output.status.code(),
                "{label} non-zero: {msg}"
            );
        "#;
        let hits = interpolated_body_idents(pre_fix);
        let idents: Vec<&str> = hits.iter().map(|(_, i, _)| i.as_str()).collect();
        assert!(
            idents.contains(&"msg"),
            "the scan missed `{{msg}}`, the exact splice #872 measured: {idents:?}"
        );
        assert!(
            idents.contains(&"label"),
            "the scan missed `{{label}}` - it must SEE allowlisted idents too, or \
             the allowlist is doing nothing and a rename would slip through: {idents:?}"
        );

        assert!(
            interpolated_body_idents(r#"warn!(error = %e, "gap {kind} misclassified");"#)
                .iter()
                .any(|(_, i, _)| i == "kind"),
            "the scan missed a BARE `warn!(` - 14 sites in etl/ and capture/ \
             import the macro and call it unqualified"
        );

        // Things that must NOT trip it.
        assert!(
            interpolated_body_idents(r#"tracing::warn!(error = %e, "fetch failed");"#).is_empty(),
            "a body with no interpolation was flagged"
        );
        assert!(
            interpolated_body_idents(r#"// tracing::warn!("{msg}");"#).is_empty(),
            "a commented-out example was flagged"
        );
        assert!(
            interpolated_body_idents(r#"fn my_error!(x) { "{msg}" }"#).is_empty(),
            "`my_error!` is a different macro and must not be scanned"
        );
        assert!(
            interpolated_body_idents(r#"tracing::warn!(n = 1, "dropped {} rows", n);"#).is_empty(),
            "a positional `{{}}` placeholder is out of scope - see `placeholders`"
        );
        assert!(
            interpolated_body_idents(r#"tracing::warn!("100{{msg}} percent");"#).is_empty(),
            "an escaped `{{{{`/`}}}}` literal was read as a placeholder"
        );
    }
}
