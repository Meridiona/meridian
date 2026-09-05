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
                        out.push((p.display().to_string(), strip_test_modules(&src)));
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

    /// Remove `#[cfg(test)] mod … { … }` BLOCKS, brace-balanced, and nothing
    /// else.
    ///
    /// # Why not truncate at the first `#[cfg(test)]`
    /// That is the older idiom (`errors.rs::no_bare_error_display_in_db_paths`
    /// still uses it) and it is silently catastrophic for a whole-tree scan:
    /// everything after the FIRST test module is discarded, including the real
    /// code below it. Measured on this tree before the change -
    /// `tray/src-tauri/src/lib.rs` was scanned at 6.6% of its bytes (a test
    /// module sits at line 150 of 1915), and `intelligence/providers/jira/mod.rs`
    /// at 2%, because a bare `#[cfg(test)] mod tests;` DECLARATION on line 15
    /// truncated the file. A declaration points at a separate file, which the
    /// walk already skips by name, so it must not truncate anything at all.
    ///
    /// The scan would still have passed - just over a fraction of the codebase.
    /// That is the failure mode `SCANNED_TREES`'s file-count assert and
    /// `MIN_MACRO_SITES` below both exist to make loud.
    fn strip_test_modules(src: &str) -> String {
        const MARKER: &str = "#[cfg(test)]";
        let mut out = String::with_capacity(src.len());
        let mut rest = src;
        while let Some(i) = rest.find(MARKER) {
            let after = &rest[i + MARKER.len()..];
            // `#[cfg(test)] mod name { … }` - only a BLOCK is stripped. A `;`
            // declaration, or the attribute on a fn/const/use, is kept.
            let Some(brace) = after.find('{') else {
                out.push_str(&rest[..i + MARKER.len()]);
                rest = after;
                continue;
            };
            let head = &after[..brace];
            if !head.trim_start().starts_with("mod ") || head.contains(';') {
                out.push_str(&rest[..i + MARKER.len()]);
                rest = after;
                continue;
            }
            out.push_str(&rest[..i]);
            // Balance braces from the module's opening one, ignoring string
            // literals and line comments (both can contain a stray brace).
            let body: Vec<char> = after[brace..].chars().collect();
            let (mut depth, mut k) = (0usize, 0usize);
            let (mut in_str, mut esc, mut in_comment) = (false, false, false);
            while k < body.len() {
                let c = body[k];
                if in_comment {
                    if c == '\n' {
                        in_comment = false;
                    }
                } else if in_str {
                    if esc {
                        esc = false;
                    } else if c == '\\' {
                        esc = true;
                    } else if c == '"' {
                        in_str = false;
                    }
                } else if c == '"' {
                    in_str = true;
                } else if c == '/' && body.get(k + 1) == Some(&'/') {
                    in_comment = true;
                } else if c == '{' {
                    depth += 1;
                } else if c == '}' {
                    depth -= 1;
                    if depth == 0 {
                        k += 1;
                        break;
                    }
                }
                k += 1;
            }
            let consumed: usize = body[..k].iter().map(|c| c.len_utf8()).sum();
            rest = &after[brace + consumed..];
        }
        out.push_str(rest);
        out
    }

    /// Every `{ident}` interpolated into the message body of a `tracing::warn!`
    /// or `tracing::error!` in `src`, with its line number.
    fn interpolated_body_idents(src: &str) -> Vec<(usize, String, String)> {
        scan_bodies(src).1
    }

    /// [`interpolated_body_idents`] plus the number of WARN+/ERROR macro sites
    /// the parser REACHED. The count comes out of the same loop on purpose - a
    /// separate counting fn could drift from the real scan, and then
    /// `MIN_MACRO_SITES` would be measuring something the guard does not use.
    fn scan_bodies(src: &str) -> (usize, Vec<(usize, String, String)>) {
        let mut found = Vec::new();
        let mut reached = 0usize;
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
                reached += 1;
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
        (reached, found)
    }

    /// The last `"…"` literal in a macro argument list, unescaped enough to read
    /// its `{}` placeholders - `tracing`'s own rule for which literal is the
    /// message.
    ///
    /// # Residual, stated
    /// A message followed by a string-literal ARGUMENT
    /// (`warn!("dropped {n} for {}", "unknown")`) resolves to the argument, so
    /// that site's real message goes unscanned. No such site exists in the tree
    /// today (473 of 493 macro sites parse a message; the other 20 carry no
    /// literal at all). Scanning every literal instead would false-positive on
    /// a non-message field value, which is the worse trade for a shape that
    /// does not occur.
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

    /// What a positional `{}` / `{:?}` is reported as. It has no name to print,
    /// but it is exactly as dangerous - `warn!("failed for {}", ticket_key)`
    /// ships the key just as surely as `"failed for {ticket_key}"` does.
    ///
    /// It is enforced rather than deferred because there are currently ZERO of
    /// them in a WARN+/ERROR body across all four trees, so the rule costs
    /// nothing to hold and `docs/privacy.md` can state it to users without
    /// qualification. Deferring it to `no_bare_error_display_in_db_paths` (the
    /// earlier plan) would not have worked: that scan's own doc scopes it to
    /// five DB files and explicitly leaves ~120 sites elsewhere uncovered.
    const POSITIONAL: &str = "<positional argument>";

    /// Every placeholder in a format string: named `{ident}` captures by name,
    /// and positional `{}` / `{:?}` as [`POSITIONAL`].
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
                let closes = matches!(chars.get(j), Some('}') | Some(':'));
                if !name.is_empty() && !name.starts_with(|c: char| c.is_ascii_digit()) && closes {
                    // A NAMED capture: `{x}` or `{x:?}`.
                    out.push(name);
                } else if closes {
                    // `{}`, `{:?}`, `{0}` - the value comes from a trailing
                    // argument, which the scan cannot see. Reported all the
                    // same; see `POSITIONAL`.
                    out.push(POSITIONAL.to_string());
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
    /// A floor on how many WARN+/ERROR macro sites the parser actually REACHES.
    ///
    /// `offenders.is_empty()` passes when the scan finds nothing, so a
    /// regression in the paren balancing, the literal extraction, or
    /// [`strip_test_modules`] would turn this guard off without failing it -
    /// which is precisely how the truncation bug documented on
    /// `strip_test_modules` hid a 94% coverage loss on one file. The file-count
    /// assert in [`production_sources`] guards the walk; this guards the parse.
    ///
    /// Set well below the real count - 526 today, up from 493 before
    /// [`strip_test_modules`] replaced the truncation - so ordinary deletions
    /// don't trip it. Raise it if it ever does; do not lower it.
    const MIN_MACRO_SITES: usize = 400;

    #[test]
    fn no_user_data_interpolated_into_a_log_body() {
        let mut offenders = Vec::new();
        let mut reached = 0usize;
        for (path, src) in production_sources() {
            let (sites, hits) = scan_bodies(&src);
            reached += sites;
            for (line, ident, msg) in hits {
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
        assert!(
            reached >= MIN_MACRO_SITES,
            "the scan reached only {reached} WARN+/ERROR macro sites, below the \
             {MIN_MACRO_SITES} floor - the parse or the test-module stripping has \
             regressed and this guard is now passing over a fraction of the \
             codebase. See MIN_MACRO_SITES."
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

        // `strip_test_modules` must remove a test module's BODY and nothing
        // else. The `mod tests;` case is the one that silently truncated 98% of
        // `jira/mod.rs` under the old `split_once` idiom.
        let with_decl = "#[cfg(test)]\nmod tests;\nfn f() { warn!(\"x {y}\"); }";
        assert!(
            !interpolated_body_idents(&strip_test_modules(with_decl)).is_empty(),
            "a `#[cfg(test)] mod tests;` DECLARATION truncated the rest of the file"
        );
        let with_block = "fn f() { warn!(\"a {p}\"); }\n#[cfg(test)]\nmod t {\n    warn!(\"b {q}\");\n}\nfn g() { warn!(\"c {r}\"); }";
        let idents: Vec<String> = interpolated_body_idents(&strip_test_modules(with_block))
            .into_iter()
            .map(|(_, i, _)| i)
            .collect();
        assert!(
            idents.contains(&"p".to_string()) && idents.contains(&"r".to_string()),
            "code around a test module was stripped with it: {idents:?}"
        );
        assert!(
            !idents.contains(&"q".to_string()),
            "the test module's own body was scanned: {idents:?}"
        );
        assert!(
            interpolated_body_idents(r#"fn my_error!(x) { "{msg}" }"#).is_empty(),
            "`my_error!` is a different macro and must not be scanned"
        );
        // A positional placeholder is caught too - it has no name, but
        // `warn!("failed for {}", ticket_key)` ships the key all the same.
        assert!(
            interpolated_body_idents(r#"tracing::warn!(n = 1, "dropped {} rows", n);"#)
                .iter()
                .any(|(_, i, _)| i == POSITIONAL),
            "a positional `{{}}` placeholder was not flagged - see `POSITIONAL`"
        );
        assert!(
            interpolated_body_idents(r#"tracing::warn!("100{{msg}} percent");"#).is_empty(),
            "an escaped `{{{{`/`}}}}` literal was read as a placeholder"
        );
    }
}
