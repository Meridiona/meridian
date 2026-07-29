//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! A stable per-machine seed for telemetry pseudonymisation.
//!
//! # Why the hostname could not be the seed
//! [`crate::telemetry_spool::redact`] hashes a machine identifier into the
//! 16-hex pseudonym that replaces `host.name` on the ship leg and that the user
//! is shown as their **Support ID**. Both uses assume that identifier is stable
//! for the life of the machine: grouping ("one user hit this 500 times" vs "500
//! users hit it once") is meaningless otherwise, and a Support ID that drifts is
//! worse than none, because the user quotes a value matching no rows.
//!
//! `gethostname()` does not satisfy that on macOS. `HostName` is unset by
//! default (only `ComputerName`/`LocalHostName` are set by a rename), so configd
//! derives the kernel hostname from the **network**. Measured on a dev Mac:
//!
//! ```text
//! hostname                    Unknown_a2:97:03:78:a9:6a
//! dig -x 192.168.1.13         Unknown_a2:97:03:78:a9:6a.   <- same string
//! en1 ether                   a2:97:03:78:a9:6a            <- locally administered
//! scutil --get ComputerName   Mac Studio                   <- unrelated
//! ```
//!
//! The hostname is the router's reverse-DNS record for the current IP, itself
//! derived from a **private Wi-Fi address** that macOS rotates per network. So
//! joining a different network changes the hostname, and every downstream
//! pseudonym with it - the same machine arrives as a new one.
//!
//! # What is used instead
//! `IOPlatformUUID`, the hardware UUID: stable across reboots, OS updates,
//! renames and network changes. It also *improves* the pseudonymisation, which
//! is a one-way hash of a low-entropy input: hostnames are enumerable (an
//! attacker can hash `Akarshs-MacBook-Pro.local` and compare), a 128-bit
//! hardware UUID is not.
//!
//! Other platforms fall through to the hostname, deliberately - on Windows
//! `gethostname()` returns the computer name, which is already stable, and
//! Meridian ships no Linux build. Adding untested registry/`/etc/machine-id`
//! probing for a case that is not broken would be risk without benefit.
//!
//! # Who calls this
//! [`crate::telemetry_spool::redact::local_host_pseudonym`] - the single seam
//! through which both the shipped `host.name` and the displayed Support ID are
//! derived.

use std::sync::OnceLock;

/// Absolute path so a hostile `PATH` cannot substitute the binary. This runs in
/// a long-lived daemon, and the value it returns becomes an identity.
#[cfg(target_os = "macos")]
const IOREG: &str = "/usr/sbin/ioreg";

/// A stable hardware-derived identifier for this machine, or `None` when the
/// platform has no better answer than the hostname.
///
/// Cached: the macOS implementation spawns a process, and callers may hit this
/// per shipped file. Returning `None` is cached too, so a failing probe costs
/// one spawn per process, not one per call.
pub(crate) fn stable_machine_id() -> Option<&'static str> {
    static CACHED: OnceLock<Option<String>> = OnceLock::new();
    CACHED
        .get_or_init(|| {
            let id = read_platform_id();
            // Logged once (inside the OnceLock init), not per call. On macOS a
            // miss means every pseudonym silently degrades to the unstable
            // hostname seed - precisely the regression this module exists to
            // prevent - and nothing else would report it. Elsewhere the
            // hostname IS the intended seed, so a miss is not a fault.
            #[cfg(target_os = "macos")]
            if id.is_none() {
                tracing::warn!(
                    probe = "ioreg IOPlatformUUID",
                    "no stable hardware id; falling back to the hostname, so this machine's \
                     pseudonym and Support ID may change when it changes network"
                );
            }
            id
        })
        .as_deref()
}

/// macOS: parse `IOPlatformUUID` out of the `IOPlatformExpertDevice` node.
///
/// Shells out to `ioreg` rather than binding IOKit, to avoid pulling a
/// `core-foundation`/`io-kit-sys` dependency into the daemon for one string
/// read at startup. Any failure returns `None` and the caller falls back to the
/// hostname - a wrong-but-present id would be worse than the status quo.
#[cfg(target_os = "macos")]
fn read_platform_id() -> Option<String> {
    let out = std::process::Command::new(IOREG)
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_ioreg_uuid(&String::from_utf8_lossy(&out.stdout))
}

/// Pull the `IOPlatformUUID` value out of `ioreg` output. Split out from the
/// process spawn so the parsing is testable without a Mac in the loop.
///
/// The line looks like:
/// `    "IOPlatformUUID" = "2D5462F0-45C1-5987-94E9-5CBAD14E4362"`
#[cfg(target_os = "macos")]
fn parse_ioreg_uuid(text: &str) -> Option<String> {
    text.lines()
        .find(|l| l.contains("\"IOPlatformUUID\""))
        .and_then(|l| l.split('"').nth(3))
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// Non-macOS: no probe. See the module doc - the hostname is already stable on
/// the only other platform Meridian ships.
#[cfg(not(target_os = "macos"))]
fn read_platform_id() -> Option<String> {
    None
}

// Every test below is macOS-only (they exercise parse_ioreg_uuid/stable_machine_id,
// which only exist on macOS - see read_platform_id's non-macOS stub above). Gating
// the whole module, not just each test, keeps `use super::*` from being flagged as
// unused on other platforms, where nothing in this module would otherwise compile.
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn parses_the_uuid_out_of_real_ioreg_output() {
        // Trimmed from actual `ioreg -rd1 -c IOPlatformExpertDevice` output.
        let sample = r#"+-o J375dAP  <class IOPlatformExpertDevice, id 0x100000284>
    {
      "IOPlatformSerialNumber" = "MXMFHVWMVQ"
      "IOPlatformUUID" = "2D5462F0-45C1-5987-94E9-5CBAD14E4362"
      "IOBusyInterest" = "IOCommand is not serializable"
    }
"#;
        assert_eq!(
            parse_ioreg_uuid(sample).as_deref(),
            Some("2D5462F0-45C1-5987-94E9-5CBAD14E4362")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn malformed_output_yields_none_rather_than_a_junk_id() {
        // A junk id is worse than none: it would be stable-looking but wrong.
        for bad in [
            "",
            "no uuid here at all",
            "\"IOPlatformUUID\" = \"\"",
            "\"IOPlatformUUID\"",
        ] {
            assert_eq!(parse_ioreg_uuid(bad), None, "accepted junk: {bad:?}");
        }
    }

    /// On a real Mac the probe must actually succeed - a silent fallback to the
    /// hostname would reintroduce the instability this module exists to fix,
    /// and nothing else would report it.
    #[cfg(target_os = "macos")]
    #[test]
    fn probe_succeeds_on_this_machine() {
        let id = stable_machine_id().expect("IOPlatformUUID probe failed on macOS");
        assert!(id.len() >= 32, "implausible platform uuid: {id}");
        // Stable across calls (it is cached, but pin the contract).
        assert_eq!(Some(id), stable_machine_id());
    }
}
