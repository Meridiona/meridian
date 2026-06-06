// meridian — normalises screenpipe activity into structured app sessions
//
// meridian-a11y-helper — enables macOS accessibility on Electron/Chromium apps.
//
// Chromium-based apps (Claude Desktop, Codex, Slack, Discord, …) ship with
// their accessibility tree DISABLED and only build it when an assistive
// technology announces itself. An app with no AX tree never registers with
// macOS's accessibility focus tracker, so screenpipe attributes its frames to
// the previously focused app (or content-dedup drops them entirely) — the
// app's activity silently vanishes from the timeline. Worse, screenpipe's own
// AXManualAccessibility enable poke is aimed using that same stale focus
// answer, so it never reaches the app: a chicken-and-egg that keeps the app
// invisible forever.
//
// This helper breaks the egg from outside screenpipe: every POLL_SECS it asks
// the window server (CGWindowList — always fresh, no run-loop dependency)
// which app owns the frontmost normal window, and sets
// `AXManualAccessibility = true` on that pid once. Chromium honours the flag
// and materialises its tree within ~1–3 s; non-Chromium apps reject the
// attribute harmlessly. After the poke, stock screenpipe sees and captures
// the app like any other.
//
// Runs as its own launchd agent (com.meridiona.a11y-helper) so the
// Accessibility grant is keyed to THIS binary, not the frequently-updated
// meridian daemon. The committed prebuilt binary must NOT be rebuilt by CI —
// a byte-identical binary keeps its CDHash, which keeps the user's TCC grant
// valid across meridian updates. Rebuild only when this source changes
// (scripts/a11y-helper/build.sh), and call out the required permission
// re-grant in the release notes.
//
// Requires: Accessibility permission (System Settings → Privacy & Security).
// Without it the poke calls fail; the helper logs the state and keeps
// retrying so a later grant is picked up without a restart.

import AppKit
import ApplicationServices

let POLL_SECS: TimeInterval = 3.0
/// Re-check trust state at this cadence when untrusted (cheap call).
let PRUNE_EVERY: Int = 100 // poll iterations between dead-pid prunes

func log(_ msg: String) {
    let ts = ISO8601DateFormatter().string(from: Date())
    print("\(ts) a11y-helper: \(msg)")
    fflush(stdout)
}

/// Frontmost application pid straight from the window server: owner of the
/// first layer-0 (normal app) window in front-to-back z-order. Fresh on every
/// call — unlike NSWorkspace, whose activation state only refreshes when the
/// main thread pumps an AppKit run loop. Works without Screen Recording
/// permission (window names are redacted without it, but pid + layer are
/// always present).
func frontmostPid() -> pid_t? {
    let options: CGWindowListOption = [.optionOnScreenOnly, .excludeDesktopElements]
    guard let list = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] else {
        return nil
    }
    for window in list {
        guard let layer = window[kCGWindowLayer as String] as? Int, layer == 0,
              let pid = window[kCGWindowOwnerPID as String] as? Int else { continue }
        return pid_t(pid)
    }
    return nil
}

/// Set AXManualAccessibility=true on the pid. Chromium/Electron builds its
/// accessibility tree in response; everything else returns an error we ignore.
/// Returns true when the attribute was accepted.
@discardableResult
func poke(_ pid: pid_t) -> Bool {
    let app = AXUIElementCreateApplication(pid)
    let err = AXUIElementSetAttributeValue(app, "AXManualAccessibility" as CFString, kCFBooleanTrue)
    return err == .success
}

var pokedPids = Set<pid_t>()
var lastTrusted: Bool? = nil
var iteration = 0

log("started (poll \(POLL_SECS)s)")

while true {
    iteration += 1

    let trusted = AXIsProcessTrusted()
    if trusted != lastTrusted {
        if trusted {
            log("AX trusted: true — poking enabled")
            // A grant arriving mid-run means earlier pokes silently failed —
            // forget them so the apps get poked again.
            pokedPids.removeAll()
        } else {
            log("AX trusted: false — grant Accessibility to meridian-a11y-helper in System Settings → Privacy & Security → Accessibility (pokes are no-ops until then)")
        }
        lastTrusted = trusted
    }

    if trusted, let pid = frontmostPid(), !pokedPids.contains(pid) {
        let accepted = poke(pid)
        pokedPids.insert(pid)
        let name = NSRunningApplication(processIdentifier: pid)?.localizedName ?? "pid \(pid)"
        log("poked \(name) (pid \(pid)) → \(accepted ? "accepted (Chromium/Electron — tree will materialise)" : "rejected (native app — no-op)")")
    }

    // Prune pids of exited processes so reused pids get re-poked.
    if iteration % PRUNE_EVERY == 0 {
        pokedPids = pokedPids.filter { kill($0, 0) == 0 || errno == EPERM }
    }

    Thread.sleep(forTimeInterval: POLL_SECS)
}
