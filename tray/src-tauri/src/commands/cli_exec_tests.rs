//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Tests for [`super`] - split out to keep `cli_exec.rs` under the 500-line
//! rule. Attached with `#[path]` rather than a `cli_exec/` directory so the
//! `include_str!("cli_exec.rs")` source scan below keeps resolving against the
//! same directory.
use super::{log_non_zero_exit, NonZeroExit};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing_subscriber::{layer::SubscriberExt, registry, Layer};

/// One captured tracing event: level, message body, and `(field, value)`
/// pairs. Mirrors `commands::setup`'s recorder, which exists for the same
/// reason - `telemetry_spool::redact` filters on the EXACT field name, so a
/// test that only asserted "the stderr is logged somewhere" would pass
/// whether it landed in the body (always ships) or an unallowlisted
/// attribute (never ships), which is the entire distinction under test.
#[derive(Debug)]
struct CapturedEvent {
    message: String,
    fields: Vec<(String, String)>,
}

struct Recorder(Arc<Mutex<Vec<CapturedEvent>>>);

impl<S: tracing::Subscriber> Layer<S> for Recorder {
    fn on_event(&self, event: &tracing::Event<'_>, _c: tracing_subscriber::layer::Context<'_, S>) {
        struct V(Vec<(String, String)>);
        impl tracing::field::Visit for V {
            fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
                self.0.push((f.name().to_string(), format!("{v:?}")));
            }
        }
        let mut v = V(Vec::new());
        event.record(&mut v);
        // `tracing` records the formatted message body under the reserved
        // field name `message`; everything else is a structured attribute.
        let message =
            v.0.iter()
                .find(|(k, _)| k == "message")
                .map(|(_, val)| val.clone())
                .unwrap_or_default();
        let fields = v.0.into_iter().filter(|(k, _)| k != "message").collect();
        self.0
            .lock()
            .unwrap()
            .push(CapturedEvent { message, fields });
    }
}

/// Run `f` under a bare recording subscriber (no `EnvFilter`, so level is
/// irrelevant) and return what it emitted. Drains through the `Mutex`
/// rather than `Arc::try_unwrap` - see `commands::setup::capture`'s note on
/// the Windows CI flake that caused.
fn capture(f: impl FnOnce()) -> Vec<CapturedEvent> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let subscriber = registry().with(Recorder(Arc::clone(&seen)));
    tracing::subscriber::with_default(subscriber, f);
    let events = std::mem::take(&mut *seen.lock().unwrap());
    events
}

/// The measured leak from issue #872, verbatim: a real user's ticket key
/// reached central OpenObserve inside a WARN body, because the tray spliced
/// the `meridian` subprocess's stderr into the message. `task_key` is denied
/// as an attribute and pinned so by
/// `redact::user_scoped_diagnostic_keys_stay_off_the_allowlist` - but the
/// body is not an attribute, so the allowlist never saw it.
const MEASURED_STDERR: &str = "Jira GET transitions for ENG-7041 returned 404 Not Found:         {\"errorMessages\":[\"Issue does not exist or you do not have permission to see it.\"]}";

/// The regression this whole change exists for.
///
/// Asserts BOTH halves, because either one alone is satisfiable by a bad
/// fix: the ticket key must not be in the body (or it ships), AND the
/// stderr must still be captured under `stderr_tail` (or we have repeated
/// #867 in the opposite direction and deleted the only record the failure
/// leaves on the engineer's own machine).
#[test]
fn a_non_zero_exit_keeps_subprocess_stderr_out_of_the_message_body() {
    let events = capture(|| {
        log_non_zero_exit(NonZeroExit {
            label: "ticket-statuses",
            bin: Some("/Users/someone/.meridian/bin/meridian"),
            provider: Some("jira"),
            key: Some("ENG-7041"),
            code: Some(1),
            stderr: MEASURED_STDERR,
        })
    });
    let e = events.first().expect("log_non_zero_exit emitted no event");

    assert!(
        !e.message.contains("ENG-7041"),
        "the user's ticket key is in the log BODY, which always ships - \
             redact's attribute allowlist cannot reach it. Body was: {}",
        e.message
    );
    assert!(
        !e.message.contains("Issue does not exist"),
        "the provider's error payload is in the log BODY. Body was: {}",
        e.message
    );
    assert!(
        e.message.contains("ticket-statuses"),
        "the body must still name WHICH subcommand failed. Body was: {}",
        e.message
    );

    let tail = e
        .fields
        .iter()
        .find(|(k, _)| k == "stderr_tail")
        .map(|(_, v)| v.as_str())
        .expect(
            "stderr is no longer captured at all - it must move to an \
                 unallowlisted ATTRIBUTE, not disappear",
        );
    assert!(
        tail.contains("ENG-7041"),
        "stderr_tail lost the diagnostic it exists to carry: {tail}"
    );
}

/// `provider` SHIPS, so an unrecognised value must not reach the wire verbatim.
/// It arrives here from the frontend as a plain `String` with nothing
/// validating it - see `provider_label`.
#[test]
fn an_unrecognised_provider_is_narrowed_before_it_ships() {
    fn shipped_provider(p: &str) -> String {
        let events = capture(|| {
            log_non_zero_exit(NonZeroExit {
                label: "ticket-update",
                bin: None,
                provider: Some(p),
                key: None,
                code: Some(1),
                stderr: "boom",
            })
        });
        events[0]
            .fields
            .iter()
            .find(|(k, _)| k == "provider")
            .map(|(_, v)| v.clone())
            .expect("provider field missing")
    }

    let shipped = shipped_provider("acme-internal-tracker-prod");
    assert!(
        !shipped.contains("acme-internal-tracker-prod"),
        "an unvalidated provider name reached an ALLOWLISTED field: {shipped}"
    );
    assert!(shipped.contains("other"), "{shipped}");

    // …while every real tracker still reports itself.
    for p in [
        "jira",
        "linear",
        "github",
        "azure_devops",
        "asana",
        "trello",
    ] {
        let shipped = shipped_provider(p);
        assert!(
            shipped.contains(p),
            "`{p}` was narrowed away - provider_label has drifted from \
             meridian_core::canonical_task::Provider::as_str: {shipped}"
        );
    }
}

/// `stderr_tail` must stay OFF `redact::SAFE_STRING_KEYS`, or moving the
/// stderr out of the body accomplishes nothing - it would ship under its new
/// name instead. Pinning it here rather than trusting the reader to
/// remember: the fix above is only a fix while this holds.
#[test]
fn the_key_the_stderr_moved_to_is_not_allowlisted_for_egress() {
    let redact = include_str!("../../../../src/telemetry_spool/redact.rs");
    let allowlist = redact
        .split_once("const SAFE_STRING_KEYS")
        .expect("SAFE_STRING_KEYS was renamed - this guard no longer reads it")
        .1
        .split_once("\n];")
        .expect("SAFE_STRING_KEYS list terminator moved")
        .0;
    for key in ["stderr_tail", "key", "bin"] {
        assert!(
            !allowlist.contains(&format!("\"{key}\"")),
            "`{key}` was added to redact::SAFE_STRING_KEYS, so the stderr / \
                 ticket key / home-directory path this module deliberately keeps \
                 local now egresses. See log_non_zero_exit's doc (#872)."
        );
    }
}

/// Regression guard: on a timeout, `tokio::time::timeout` drops the
/// `.output()` future, and without `kill_on_drop(true)` the spawned
/// `meridian <args>` keeps running in the background after the caller has
/// already reported failure to the user. For an LLM-backed call like
/// `plan-task-draft`, that orphan then competes with the next "Try again"
/// click's fresh process for the same provider/DB — a plausible reason a
/// draft that missed its 150s budget once keeps missing it on retry.
///
/// This can't drive `run_meridian` itself as a real spawn-and-verify test:
/// it resolves its binary via `crate::install::meridian_bin()`, and
/// overriding that via `MERIDIAN_BIN` would mean `std::env::set_var` on a
/// shared test binary — exactly what `integrations.rs`'s "avoiding
/// `std::env::set_var` on a Tokio worker thread" note warns off. So this
/// is source-scanned, mirroring `tasks.rs::sync_tasks`, the sibling call
/// site that already carries this fix. The MECHANISM itself — that
/// `kill_on_drop(true)` actually terminates an orphaned child, on this
/// platform, with tokio's real process reaping — is verified separately
/// below, against a plain `tokio::process::Command` that needs no
/// `MERIDIAN_BIN` override at all.
#[test]
fn run_meridian_kills_the_child_on_timeout() {
    let src = include_str!("cli_exec.rs");
    let prod = src.split_once("\n#[cfg(test)]").map_or(src, |(a, _)| a);
    let spawn = prod
        .find("tokio::process::Command::new(&bin)")
        .expect("run_meridian's Command builder moved or was renamed");
    let output_call = prod[spawn..]
        .find(".output();")
        .expect("run_meridian's Command builder no longer ends in .output()");
    let builder = &prod[spawn..spawn + output_call];
    assert!(
        builder.contains(".kill_on_drop(true)"),
        "run_meridian's spawned child is missing .kill_on_drop(true) — a \
             timeout will orphan it instead of killing it. Builder was: {builder}"
    );
}

/// A process that runs far longer than the timeout below, so the timeout
/// always wins the race — the exact shape `run_meridian` puts its child
/// in. No dependency on `meridian`/`MERIDIAN_BIN`: `sleep`/`ping` are
/// present on every macOS and Windows runner this crate's tests run on
/// (see `.github/workflows/ci.yml`'s `windows-latest` + `macos-latest`
/// `cargo test --workspace` jobs).
#[cfg(unix)]
fn long_running_command() -> (&'static str, &'static [&'static str]) {
    ("sleep", &["30"])
}
#[cfg(windows)]
fn long_running_command() -> (&'static str, &'static [&'static str]) {
    // `timeout.exe` refuses to run with stdin redirected (no console
    // handle) — `ping` to loopback is the standard "sleep N seconds"
    // substitute on Windows and needs no real network.
    ("ping", &["-n", "30", "127.0.0.1"])
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
#[cfg(windows)]
fn process_is_alive(pid: u32) -> bool {
    // `tasklist` exits 0 either way, printing "No tasks are running..."
    // when nothing matches — the pid has to actually appear in the
    // output, not just a success status.
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
        .unwrap_or(false)
}

/// Proves the MECHANISM `run_meridian_kills_the_child_on_timeout` pins the
/// wiring for: that `kill_on_drop(true)` on a `tokio::process::Command`
/// actually terminates the child once the future racing it is dropped —
/// i.e. that this fix does what its own reasoning claims, not just that
/// the flag is textually present.
#[tokio::test]
async fn kill_on_drop_actually_terminates_the_orphaned_child() {
    let (bin, args) = long_running_command();
    let child = tokio::process::Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("failed to spawn the long-running test process");
    let pid = child.id().expect("a just-spawned child must have a pid");
    assert!(
        process_is_alive(pid),
        "test bug: process not observed alive right after spawn"
    );

    {
        // Mirrors `run_meridian` exactly: race the child's `.output()`
        // against a timeout far shorter than the process's own runtime.
        let output_fut = child.wait_with_output();
        let result = tokio::time::timeout(Duration::from_millis(50), output_fut).await;
        assert!(
            result.is_err(),
            "test bug: the process exited before the timeout could fire"
        );
        // `output_fut` (and the `Child` it consumed) drops here — exactly
        // what happens when `tokio::time::timeout` drops `run_meridian`'s
        // `.output()` future on a real timeout.
    }

    // `kill_on_drop`'s kill is fired from `Drop`, not awaited to
    // completion — poll briefly rather than asserting instantaneously.
    let mut still_alive = process_is_alive(pid);
    for _ in 0..20 {
        if !still_alive {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        still_alive = process_is_alive(pid);
    }
    assert!(
        !still_alive,
        "child (pid {pid}) is still running ~2s after being dropped — \
             kill_on_drop did not terminate it"
    );
}
