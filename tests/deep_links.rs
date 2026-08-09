//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Static regression guard for the notification/notice `deep_link` vocabulary.
//!
//! # The incident
//! `MeridianTimelineShell.tsx`'s `navigate` resolved exactly three targets
//! (`/plan`, `/worklogs`, `/whats-new`) with **no `else` arm**, while producers
//! emitted seven. The other four fell through in silence and opened the
//! dashboard's default view — so the `[View]` button on every fault toast went
//! nowhere in particular. Worse, `/tasks?integrations=1` kept being emitted by
//! all five `pm.*` sync faults long after the `/tasks` route was deleted in the
//! Next fold, because a link that resolves to nothing is indistinguishable from
//! a link that resolves correctly: no error, no log, no failing test.
//!
//! # What this locks
//! Every `deep_link` literal a Rust producer emits is a value
//! [`meridian_core::notifications::deep_links`] declares. The other half of the
//! contract — that the shell actually handles each declared value — is
//! `ui/__tests__/deep-links.test.ts`, which reads the same constants. Both are
//! needed: this one alone would happily pass while `navigate` ignored the lot.
//!
//! Placed in the root crate for the same reason as `tray_assets.rs` — it only
//! reads files by path, so it runs under the pre-push hook and CI without any
//! of the tray's macOS/capture dependencies.

use meridian_core::notifications::deep_links;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Source trees that contain notification/notice producers.
const PRODUCER_DIRS: &[&str] = &["src", "tray/src-tauri/src", "meridian-core/src"];

/// Collect every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// How far past a `link` / `navigate` mention a path literal still counts as
/// that mention's argument. Generous enough to cross the line break in
///
/// ```ignore
/// let link = if id.starts_with("pm.") {
///     Some("/tasks?integrations=1")
/// ```
///
/// and tight enough that an unrelated path later in the function doesn't get
/// attributed to it.
const LOOKAHEAD: usize = 200;

/// The four shapes that actually attach a deep link to something, lowercase.
///
/// Anchoring on these rather than on the bare word `link` is not fussiness: a
/// looser rule flagged `pm_worklog/trello.rs`'s `parse_short_link`, a URL
/// parser that mentions `/c/` and has nothing to do with notifications. The
/// anchors are, in order: the struct/builder field (`Notice { deep_link: … }`,
/// `NewNotification.deep_link`, `(…).then_some(…)` assigned to it), the builder
/// method, the tray's non-outbox navigation call, and the local binding in
/// `notices.rs::raise` that computes a link before handing it over.
const ANCHORS: [&str; 4] = ["deep_link", ".link(", "navigate_dashboard", "let link"];

/// Pull every path-shaped string literal that sits near a link/navigate mention.
///
/// # Why it scans a window rather than a line
/// Two earlier drafts of this test passed while catching nothing, and both
/// failures are instructive:
///
/// 1. Matching exact call shapes (`deep_link: Some(`, `.link(`, `then_some(`)
///    missed the single most important site in the repo — `notices.rs`'s
///    `let link = if id.starts_with("pm.") { Some("…") }`, a bare `Some(`
///    bound to a variable, which is where the dead `/tasks?integrations=1`
///    actually lived.
/// 2. Widening to "any `"/…"` on a line mentioning link" *still* missed it,
///    because `let link =` and the literal are on **different lines**.
///
/// So the rule is: find each `link`/`navigate` mention, then look ahead
/// [`LOOKAHEAD`] bytes — across newlines — for path literals. A guard that only
/// recognises the shapes its author thought of is how the original bug survived
/// for months; verify any future change to this function against a deliberate
/// regression, not just against a clean tree.
///
/// Only *literal* arguments are returned; a constant reference
/// (`deep_links::LOGS`) yields nothing. That is the point — the remediation is
/// that producers reference constants, so a clean scan means "no raw paths were
/// reintroduced" as much as "every path is known".
fn deep_link_literals(code: &str) -> Vec<String> {
    let lower = code.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while cursor < lower.len() {
        // Next anchor from `cursor`.
        let next = ANCHORS
            .iter()
            .filter_map(|k| lower[cursor..].find(k).map(|i| cursor + i))
            .min();
        let Some(at) = next else { break };
        // Advance past this anchor regardless of what we find, so overlapping
        // matches (`deep_link` contains `link`) can't loop forever.
        cursor = at + 1;
        let end = (at + LOOKAHEAD).min(code.len());
        // Respect char boundaries — source files carry non-ASCII (em-dashes in
        // comments), and slicing mid-codepoint would panic.
        if !code.is_char_boundary(at) || !code.is_char_boundary(end) {
            continue;
        }
        let mut rest = &code[at..end];
        while let Some(i) = rest.find('"') {
            let body = &rest[i + 1..];
            let Some(q) = body.find('"') else { break };
            let lit = &body[..q];
            if lit.starts_with('/') {
                out.push(lit.to_string());
            }
            rest = &body[q + 1..];
        }
    }
    out.sort();
    out.dedup();
    out
}

/// GUARD: no producer emits a deep link the shell has never heard of.
///
/// A failure means one of two things, and the fix differs:
///   * you added a genuinely new destination → add it to `deep_links::ALL`
///     AND teach `navigate` to resolve it (the bun guard enforces the second
///     half);
///   * you hardcoded a path that already has a constant → use the constant.
#[test]
fn deep_link_literals_are_all_known() {
    let root = repo_root();
    let mut files = Vec::new();
    for dir in PRODUCER_DIRS {
        rust_files(&root.join(dir), &mut files);
    }
    assert!(
        files.len() > 50,
        "producer scan found only {} files — the paths are probably wrong, and a \
         scan that reads nothing passes vacuously",
        files.len()
    );

    let mut offenders = Vec::new();
    for f in &files {
        // This test's own doc comments name the retired spellings.
        if f.ends_with("tests/deep_links.rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        // Doc/line comments are stripped first: they discuss retired spellings
        // by name (this module's own header does), and a prose mention is not
        // an emission. Only whole-line comments go — a trailing `//` cannot be
        // removed safely without a real lexer, since `"https://…"` contains one.
        let code: String = text
            .lines()
            .map(|l| {
                if l.trim_start().starts_with("//") {
                    ""
                } else {
                    l
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        for lit in deep_link_literals(&code) {
            if !deep_links::is_known(&lit) {
                offenders.push(format!(
                    "{} emits unknown deep_link {lit:?}",
                    f.strip_prefix(&root).unwrap_or(f).display(),
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "deep links not in `meridian_core::notifications::deep_links`:\n  {}",
        offenders.join("\n  ")
    );
}

/// GUARD: the retired spellings stay resolvable.
///
/// Rows carrying `/tasks?integrations=1` are sitting in users' outboxes right
/// now (they were still being written until this change). Dropping them from
/// `LEGACY` because no producer emits them any more would strand exactly those
/// rows — the same "the producer stopped, the data didn't" trap that left 22
/// dead board-hygiene banners behind.
#[test]
fn legacy_spellings_remain_resolvable() {
    for legacy in deep_links::LEGACY {
        assert!(
            deep_links::is_known(legacy),
            "{legacy} dropped out of the resolvable set"
        );
        assert!(
            !deep_links::ALL.contains(&legacy),
            "{legacy} is retired — it must not be offered to new producers"
        );
    }
}
