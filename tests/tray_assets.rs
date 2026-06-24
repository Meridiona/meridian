//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Static regression guards for the Tauri tray's config + frontend contract.
//!
//! These do NOT render the GUI — they lock the *invariants* that broke (often
//! silently) across the tray-redesign work, so a future edit that re-breaks one
//! fails `cargo test` before a build/commit instead of after a slow rebuild +
//! manual hover/click. Each test documents the incident it guards.
//!
//! Placed in the root `meridian` crate (not `tray/src-tauri`) on purpose: the
//! pre-push hook's `cargo test` and CI both build this crate, and these checks
//! only read files by path — they need none of the tray's macOS/capture deps.
//!
//! # What is and isn't covered
//! Covered: capability permissions, window config, JS syntax, the JS↔HTML
//! element-id contract. NOT covered: actual pixel rendering / hover behaviour —
//! that still needs a human (or the tray_debug stderr log) on a packaged build.

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is the root `meridian` crate dir = repo root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_json(rel: &str) -> serde_json::Value {
    let p = repo_root().join(rel);
    let txt = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
    serde_json::from_str(&txt).unwrap_or_else(|e| panic!("parse {} as JSON: {e}", p.display()))
}

fn read_text(rel: &str) -> String {
    let p = repo_root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

const CAPS: &str = "tray/src-tauri/capabilities/default.json";
const CONF: &str = "tray/src-tauri/tauri.conf.json";

/// Window labels the tray code (`lib.rs` / commands) opens or targets. Every one
/// MUST appear in the capability's `windows` list or its `invoke`s are silently
/// denied (the fold playbook's rule). Keep in sync when adding a window.
const REQUIRED_WINDOW_LABELS: &[&str] = &["main", "tray-tooltip", "dashboard", "setup"];

/// GUARD: the popover/tooltip resize themselves via JS `setSize`, which is a
/// CORE plugin command gated by the capability ACL. `core:default` grants only
/// read-only window ops, so without `core:window:allow-set-size` every resize is
/// silently denied and the popover stays stuck at its config height (clipped
/// bottom). This bit us for ~3 iterations — lock it.
#[test]
fn capability_grants_window_set_size() {
    let caps = read_json(CAPS);
    let perms: Vec<&str> = caps["permissions"]
        .as_array()
        .expect("capabilities.permissions is an array")
        .iter()
        .filter_map(|p| p.as_str())
        .collect();
    assert!(
        perms.contains(&"core:window:allow-set-size"),
        "capabilities/default.json must list core:window:allow-set-size \
         (popover/tooltip JS setSize is denied without it). Found: {perms:?}"
    );
}

/// GUARD: a window not listed in the capability's `windows` array has ALL its
/// `invoke`s denied. Every label the tray opens must be present.
#[test]
fn capability_lists_all_tray_windows() {
    let caps = read_json(CAPS);
    let windows: Vec<&str> = caps["windows"]
        .as_array()
        .expect("capabilities.windows is an array")
        .iter()
        .filter_map(|w| w.as_str())
        .collect();
    for label in REQUIRED_WINDOW_LABELS {
        assert!(
            windows.contains(label),
            "capabilities/default.json `windows` is missing {label:?} \
             (its invokes would be silently denied). Found: {windows:?}"
        );
    }
}

/// GUARD: `signingIdentity` must be `null`, never `"-"`. `"-"` forces ad-hoc
/// signing → fresh cdhash every build → macOS TCC re-prompts for Screen
/// Recording / Accessibility / Input Monitoring on every rebuild. `null` lets
/// the `APPLE_SIGNING_IDENTITY` env (the stable "Meridian Dev" cert from
/// `npm run build`) take effect. Regression guard for commit 6dd81d5.
#[test]
fn signing_identity_is_null_not_adhoc() {
    let conf = read_json(CONF);
    let sid = &conf["bundle"]["macOS"]["signingIdentity"];
    assert!(
        sid.is_null(),
        "tauri.conf.json bundle.macOS.signingIdentity must be null (not {sid}); \
         a literal \"-\" forces ad-hoc signing and breaks TCC grant persistence."
    );
}

/// GUARD: the popover + tooltip windows must exist and stay transparent /
/// undecorated (the rounded card relies on it) and hidden at boot (shown on
/// demand). Catches an accidental flip of these flags.
#[test]
fn tray_windows_have_expected_shape() {
    let conf = read_json(CONF);
    let windows = conf["app"]["windows"]
        .as_array()
        .expect("app.windows is an array");

    let find = |label: &str| -> serde_json::Value {
        windows
            .iter()
            .find(|w| w["label"] == label)
            .unwrap_or_else(|| panic!("tauri.conf.json app.windows missing {label:?}"))
            .clone()
    };

    for label in ["main", "tray-tooltip"] {
        let w = find(label);
        assert_eq!(w["transparent"], true, "{label} must be transparent");
        assert_eq!(w["decorations"], false, "{label} must be undecorated");
        assert_eq!(w["visible"], false, "{label} must start hidden");
        let width = w["width"].as_f64().unwrap_or(0.0);
        assert!(width > 0.0, "{label} needs a positive width");
    }
}

/// GUARD: the popover/tooltip JS must parse. There is no JS test runner, so a
/// syntax error otherwise only surfaces as a blank/broken popover in a packaged
/// build. Uses `node --check`; skips (does not fail) when node is unavailable.
#[test]
fn frontend_js_parses() {
    let node = Command::new("node").arg("--version").output();
    if node.map(|o| !o.status.success()).unwrap_or(true) {
        eprintln!("SKIP frontend_js_parses: node not available");
        return;
    }
    for rel in ["tray/src/app.js", "tray/src/tooltip.js"] {
        let path = repo_root().join(rel);
        let out = Command::new("node")
            .arg("--check")
            .arg(&path)
            .output()
            .unwrap_or_else(|e| panic!("run node --check {rel}: {e}"));
        assert!(
            out.status.success(),
            "node --check failed for {rel}:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

// ── Dev-setup contract guards (v1.64.0 fold) ──────────────────────────────
// These lock invariants of the dev-script changes that landed with the fold.
// A future edit that accidentally reverts the topology (re-adding a separate
// Next.js window, re-enabling screenpipe agents) should break `cargo test`
// rather than silently breaking new-contributor setup.

/// GUARD: `dev-start.sh` must no longer bootstrap screenpipe as a launchd
/// agent. v1.64.0 runs capture in-process inside the Tauri tray binary — a
/// screenpipe launchd agent conflicts and is unneeded. The old 80-line block
/// (`Ensure screenpipe is up`) was removed; this assertion prevents it from
/// creeping back.
#[test]
fn dev_start_no_screenpipe_launchd_bootstrap() {
    let src = read_text("dev-start.sh");
    assert!(
        !src.contains("LABEL_SCREENPIPE"),
        "dev-start.sh must not bootstrap a screenpipe launchd agent \
         (capture is in-process inside the Tauri tray since v1.64.0). \
         Remove the LABEL_SCREENPIPE / launchctl bootstrap block."
    );
    assert!(
        !src.contains("install-screenpipe-daemon.sh"),
        "dev-start.sh must not call install-screenpipe-daemon.sh"
    );
}

/// GUARD: `dev-start.sh` must not open a separate Next.js terminal window.
/// `npm run tauri dev` starts the Next.js dev server automatically via
/// `beforeDevCommand` in `tauri.conf.json` — a second window runs it twice.
/// The fold removed window 3 ("Next.js UI"); this prevents it returning.
#[test]
fn dev_start_no_separate_nextjs_window() {
    let src = read_text("dev-start.sh");
    assert!(
        !src.contains("npm run dev"),
        "dev-start.sh must not start a separate `npm run dev` window — \
         `npm run tauri dev` handles this via beforeDevCommand. \
         Running both starts two Next.js processes on the same port."
    );
}

/// GUARD: `dev-start.sh` must open exactly 3 Terminal windows (daemon, MLX,
/// tray). The old 4-window setup had a redundant Next.js window; the new fold
/// topology has 3. Counts `do script` calls in the embedded AppleScript block.
#[test]
fn dev_start_opens_three_terminal_windows() {
    let src = read_text("dev-start.sh");
    let count = src.matches("do script \"").count();
    assert_eq!(
        count, 3,
        "dev-start.sh must open exactly 3 Terminal windows (daemon, MLX, tray). \
         Found {count} `do script` calls. If you added a new service window, \
         update this test to reflect the new expected count."
    );
}

/// GUARD: `install-dev.sh` must not register screenpipe or a11y-helper as
/// launchd agents. Both are unneeded since v1.64.0 (capture is in-process);
/// registering them causes a duplicate-capture conflict.
#[test]
fn install_dev_skips_screenpipe_agents() {
    let src = read_text("install-dev.sh");
    assert!(
        !src.contains("install-screenpipe-daemon.sh"),
        "install-dev.sh must not call install-screenpipe-daemon.sh \
         (screenpipe is no longer a separate process — capture is in-process \
         inside the Tauri tray since v1.64.0)."
    );
    assert!(
        !src.contains("install-a11y-helper-daemon.sh"),
        "install-dev.sh must not call install-a11y-helper-daemon.sh \
         (a11y capture is in-process inside the Tauri tray since v1.64.0)."
    );
}

/// GUARD: `tauri.conf.json`'s `beforeDevCommand` must start the Next.js dev
/// server. This is what removes the need for a separate Next.js terminal in
/// `dev-start.sh` — if the beforeDevCommand is removed or changed, the
/// dashboard would be blank in dev mode without an explicit `npm run dev`.
#[test]
fn tauri_before_dev_command_starts_nextjs() {
    let conf = read_json(CONF);
    let before_dev = conf["build"]["beforeDevCommand"]
        .as_str()
        .unwrap_or_default();
    assert!(
        before_dev.contains("npm run dev"),
        "tauri.conf.json build.beforeDevCommand must start the Next.js dev \
         server (contain `npm run dev`). Without it, `npm run tauri dev` opens \
         a blank webview and a separate Next.js terminal is needed again. \
         Found: {before_dev:?}"
    );
}

/// Collect element ids referenced from a JS file via `getElementById('x')` and
/// the `$('x')` helper the popover defines (`const $ = (id) => ...`).
fn referenced_ids(js: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for marker in ["getElementById(", "$("] {
        let mut rest = js;
        while let Some(i) = rest.find(marker) {
            let after = &rest[i + marker.len()..];
            let bytes = after.as_bytes();
            if let Some(&q) = bytes.first() {
                if q == b'\'' || q == b'"' {
                    if let Some(end) = after[1..].find(q as char) {
                        let id = &after[1..1 + end];
                        // `$(` is also used for non-id calls in some code; only
                        // keep plausible id tokens (no spaces / punctuation).
                        if !id.is_empty()
                            && id
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                        {
                            ids.push(id.to_string());
                        }
                    }
                }
            }
            rest = &after[1..];
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

fn assert_ids_present(js_rel: &str, html_rel: &str) {
    let js = read_text(js_rel);
    let html = read_text(html_rel);
    for id in referenced_ids(&js) {
        let needle = format!("id=\"{id}\"");
        assert!(
            html.contains(&needle),
            "{js_rel} references #{id} but {html_rel} has no element with {needle} \
             — a render-time crash (getElementById returns null) that no compiler catches."
        );
    }
}

/// GUARD: every id the popover/tooltip JS looks up must exist in its HTML.
/// A mismatch makes `getElementById` return null → a runtime TypeError on first
/// render → blank popover, with no compile error. Guards the kind of regression
/// suspected in the tooltip rewrite.
#[test]
fn js_element_id_contract_holds() {
    assert_ids_present("tray/src/app.js", "tray/src/index.html");
    assert_ids_present("tray/src/tooltip.js", "tray/src/tooltip.html");
}
