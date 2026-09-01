//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Strict flag validation for the hand-rolled subcommand CLI in `main.rs`.
//!
//! # The bug this exists for
//!
//! Every subcommand parses its arguments by searching argv for tokens it
//! recognises — `args.iter().any(|a| a == "--dry-run")`, `flag("--day")`.
//! Nothing ever walked argv asking whether a token *was* recognised, so an
//! unknown flag contributed nothing and was silently discarded. What ran was
//! then the command's **default configuration**, byte-identical to invoking it
//! bare.
//!
//! Reported from production on 2026-08-27: an operator investigating a stuck
//! queue ran `meridian coding-agent-summarise --help` expecting usage text and
//! instead summarised and persisted real rows (`dry_run` defaults to `false`,
//! and `summariser::cli_summarise` passes `!dry_run` as its persist flag).
//!
//! That was the mild case. The same hole sat in front of
//! **`worklog-post-approved`**, which takes no arguments at all and posts every
//! approved worklog as a real comment to Jira / Linear / GitHub
//! (`pm_worklog::cli_post_approved` — "the only path that writes to real
//! Jira"). `meridian worklog-post-approved --help` would have written to a
//! customer's tracker, irreversibly and visibly to their whole team, in
//! response to the single most natural thing an operator does when probing an
//! unfamiliar command.
//!
//! `tasks-sync` and `pm-sync` were the same shape one tier down (DB mirror
//! rewrites including terminal-row pruning). The remaining subcommands were
//! safe only *by accident* — they happen to require a positional argument and
//! bail without it. That is a coincidence, not a guard, and it would not have
//! survived the next all-optional-flag subcommand.
//!
//! # Why a table rather than a check per call site
//!
//! `main.rs` dispatches through ~30 sequential `if argv[1] == "..."` blocks.
//! Adding a validation call inside each one is a rule that has to be
//! remembered 30 times and then again for every new subcommand — the same
//! shape of omission that caused this. Instead [`SPECS`] is a single table
//! consulted **once**, before the dispatch chain runs, and
//! [`tests::every_dispatched_subcommand_is_registered`] scans `main.rs` for
//! dispatch literals and fails the build if one has no entry here. Forgetting
//! to register a new subcommand is a compile-time-ish error, not a latent
//! production write.
//!
//! # Scope of the check, deliberately narrow
//!
//! Only `-`-prefixed tokens are inspected, against a flat allowlist per
//! subcommand. Flag *values* are not modelled: a value that does not itself
//! start with `-` is simply not a candidate, and none of the flags below takes
//! a negative number.
//!
//! That reasoning was **wrong**, and the fix for it is [`VALUELESS_FLAGS`]. It
//! covered negative numbers and forgot free text: `--note`, `--title`,
//! `--description` and `--value` all carry user-written prose, and prose starts
//! with a hyphen often enough to matter — a bullet, a dash, a diff line.
//! `meridian plan-task-draft --note "- fixed the bug"` was rejected with
//! `unrecognised flag "- fixed the bug"` and exit 2, and the tray drives that
//! command with text the user typed. Wrongly rejecting a working invocation is
//! the one failure mode worse than the bug this module fixes, and it shipped in
//! the module that says so.
//!
//! Two independent guards now stop it, either of which suffices:
//!
//! 1. **Shape** ([`looks_like_a_flag`]): a candidate must be `-h` or `--` plus
//!    a letter. `"- fixed the bug"`, `"-5"` and a bare `"--"` are not flags.
//! 2. **Arity**: a recognised flag consumes the next token unless it is in
//!    [`VALUELESS_FLAGS`] or already carried its value as `--flag=value`.
//!
//! The two fail in opposite directions, which is why both are kept. Arity alone
//! mis-set (a value-taking flag wrongly listed as valueless) reopens the bug for
//! that flag; shape alone still trips on a value that begins `--word`. Getting
//! [`VALUELESS_FLAGS`] wrong in the other direction — omitting a boolean — costs
//! only a *missed* unknown flag, never a wrongly rejected command, which is the
//! direction to err in.
//!
//! # Who calls this
//! [`enforce`] runs from `main.rs` immediately before the subcommand dispatch
//! chain, ahead of every side effect.
//!
//! # Related
//! - `main.rs`'s unknown-*subcommand* rejection, which this mirrors for flags.
//!   That check deliberately skips anything starting with `-` (bare `meridian`
//!   and `meridian --version` still start the daemon), and that exclusion is
//!   precisely the gap this module closes.

/// One subcommand's accepted flags plus the usage line `--help` prints.
pub struct Spec {
    /// The argv[1] literal `main.rs` dispatches on.
    pub name: &'static str,
    /// Every `-`-prefixed token this subcommand accepts. `--help` / `-h` are
    /// accepted everywhere and must not be listed.
    pub flags: &'static [&'static str],
    /// Shown by `--help`. One line, plain hyphens only (user-facing text).
    pub usage: &'static str,
}

/// Every subcommand `main.rs` dispatches, with the flags it actually reads.
///
/// Kept in dispatch order so this table can be diffed against `main.rs` by eye.
/// A subcommand with no flags gets an empty slice — that is the strongest
/// entry in the table, not a placeholder: it is what makes
/// `worklog-post-approved --anything` an error instead of a tracker write.
pub const SPECS: &[Spec] = &[
    Spec {
        name: "coding-agent-hook",
        flags: &[],
        usage: "meridian coding-agent-hook   (reads a SessionEnd JSON payload on stdin)",
    },
    Spec {
        name: "coding-agent-summarise",
        flags: &["--dry-run", "--day", "--limit"],
        usage: "meridian coding-agent-summarise [--dry-run] [--day YYYY-MM-DD] [--limit N]",
    },
    Spec {
        name: "db",
        flags: &[],
        usage: "meridian db <check|repair>",
    },
    Spec {
        name: "oauth-login",
        flags: &["--app-key", "--client-id", "--port"],
        usage: "meridian oauth-login <jira|trello> [--client-id ID] [--app-key KEY] [--port N]",
    },
    Spec {
        name: "worklog-hour",
        flags: &[],
        usage: "meridian worklog-hour <HH:MM-HH:MM>",
    },
    Spec {
        name: "llm-experiment",
        flags: &[
            "--day",
            "--hour",
            "--id",
            "--limit",
            "--process",
            "--task-id",
            "--task-json",
            "--variant",
            "--variants",
        ],
        usage: "meridian llm-experiment <run|create|exec|list|get|draft-task> [flags]",
    },
    Spec {
        name: "worklog-post-approved",
        flags: &[],
        usage: "meridian worklog-post-approved   (posts approved worklogs to your tracker)",
    },
    Spec {
        name: "db-export-plaintext",
        flags: &["--out"],
        usage: "meridian db-export-plaintext [--out PATH]",
    },
    Spec {
        name: "tasks-sync",
        flags: &[],
        usage: "meridian tasks-sync   (forces a fetch from every configured tracker)",
    },
    Spec {
        name: "pm-sync",
        flags: &[],
        usage: "meridian pm-sync   (refreshes the tracker mirror if it is stale)",
    },
    Spec {
        name: "ticket-update",
        flags: &["--field", "--key", "--provider", "--value"],
        usage: "meridian ticket-update --key KEY --provider P --field F --value V",
    },
    Spec {
        name: "ticket-parents",
        flags: &["--key", "--provider"],
        usage: "meridian ticket-parents --key KEY --provider P",
    },
    Spec {
        name: "ticket-statuses",
        flags: &["--key", "--provider"],
        usage: "meridian ticket-statuses --key KEY --provider P",
    },
    Spec {
        name: "ticket-set-status",
        flags: &["--key", "--provider", "--status"],
        usage: "meridian ticket-set-status --key KEY --provider P --status S",
    },
    Spec {
        name: "plan-task-draft",
        flags: &["--note"],
        usage: "meridian plan-task-draft --note \"<text>\"",
    },
    Spec {
        name: "plan-task-create",
        flags: &["--title", "--description", "--issue-type"],
        usage: "meridian plan-task-create --title T [--description D] [--issue-type Task|Bug]",
    },
    Spec {
        name: "plan-task-edit",
        flags: &["--key", "--title", "--description"],
        usage: "meridian plan-task-edit --key K [--title T] [--description D]",
    },
    Spec {
        name: "plan-task-done",
        flags: &["--key", "--done"],
        usage: "meridian plan-task-done --key K --done true|false",
    },
    Spec {
        name: "plan-task-delete",
        flags: &["--key"],
        usage: "meridian plan-task-delete --key K",
    },
    Spec {
        name: "day-summary",
        flags: &["--day", "--now"],
        usage: "meridian day-summary [--day YYYY-MM-DD] [--now]",
    },
    Spec {
        name: "day-summary-get",
        flags: &["--day"],
        usage: "meridian day-summary-get [--day YYYY-MM-DD]",
    },
    Spec {
        name: "day-summary-data",
        flags: &["--day"],
        usage: "meridian day-summary-data [--day YYYY-MM-DD]",
    },
    Spec {
        name: "worklog-generate",
        flags: &["--day", "--task-id"],
        usage: "meridian worklog-generate --task-id ID [--day YYYY-MM-DD]",
    },
    Spec {
        name: "worklog-generate-get",
        flags: &["--day", "--task-id"],
        usage: "meridian worklog-generate-get --task-id ID [--day YYYY-MM-DD]",
    },
    Spec {
        name: "worklog-generate-approve",
        flags: &["--day", "--task-id"],
        usage: "meridian worklog-generate-approve --task-id ID [--day YYYY-MM-DD]",
    },
    Spec {
        name: "worklog-escalate-create",
        flags: &["--task"],
        usage: "meridian worklog-escalate-create --task ID",
    },
    Spec {
        name: "worklog-escalate-match",
        flags: &["--target", "--task"],
        usage: "meridian worklog-escalate-match --task ID --target KEY",
    },
    Spec {
        name: "worklog-status",
        flags: &["--day"],
        usage: "meridian worklog-status [--day YYYY-MM-DD]",
    },
    Spec {
        name: "doctor",
        flags: &["--dry-run", "--fix", "--porcelain"],
        usage: "meridian doctor [--fix] [--dry-run] [--porcelain]",
    },
    Spec {
        name: "uninstall",
        flags: &[
            "--dry-run",
            "--json",
            "--purge",
            "--remove-data",
            "--remove-models",
            "--remove-runtime",
            "--yes",
        ],
        usage: "meridian uninstall [--dry-run] [--purge] [--remove-data] \
                [--remove-models] [--remove-runtime] [--yes] [--json]",
    },
    Spec {
        name: "telemetry",
        flags: &["--auth", "--endpoint", "--out", "--since"],
        usage: "meridian telemetry <status|export|import> [--out PATH] [--since MICROS] \
                [--endpoint URL] [--auth BASE64]",
    },
    Spec {
        name: "logs",
        flags: &["--min-severity", "--service", "-n", "-f"],
        usage: "meridian logs [--service NAME] [--min-severity LEVEL] [-n N] [-f]",
    },
];

/// What [`check`] decided about an argv.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Not a registered subcommand (bare daemon start, `--version`, or a typo
    /// the unknown-subcommand check in `main.rs` will catch). No opinion.
    NotOurs,
    /// Every flag is recognised — carry on and dispatch.
    Ok,
    /// `--help` / `-h` was asked for; print this and exit 0.
    Help(&'static str),
    /// An unrecognised flag. Carries the offending token and the usage line.
    Unknown { flag: String, usage: &'static str },
}

/// The [`Spec`] for `name`, if it is a registered subcommand.
pub fn spec(name: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.name == name)
}

/// Decide what to do with `argv` (the full argv, including argv[0]).
///
/// Pure so it can be tested without a process: [`enforce`] is the thin wrapper
/// that prints and exits.
///
/// `--help` wins over an unknown flag deliberately. Someone typing
/// `meridian worklog-post-approved --help --bogus` is asking what the command
/// does, and answering that is strictly safer than an error that might send
/// them off to retry it bare — which is the invocation that actually posts.
/// The registered flags that are a bare switch and consume no following token.
///
/// Derived from how each is actually parsed - a presence test
/// (`args.iter().any(|a| a == "--fix")`) rather than a value read
/// (`flag_value(args, "--day")`) - not from how the name reads. Anything absent
/// here is treated as value-taking, which is the safe default: over-consuming
/// costs at most a *missed* unknown flag, while under-consuming lets a value
/// beginning with a hyphen be reported as one. See the module header.
const VALUELESS_FLAGS: &[&str] = &[
    "--dry-run",
    "--fix",
    "--json",
    "--now",
    "--porcelain",
    "--purge",
    "--remove-data",
    "--remove-models",
    "--remove-runtime",
    "--yes",
];

/// Whether `token` is shaped like a flag rather than like a value.
///
/// `-h` (the only single-letter flag registered anywhere here) or `--` followed
/// by a letter. Deliberately NOT "starts with `-`", which is what let
/// `"- fixed the bug"` and `"-5"` be read as flags.
///
/// A bare `--` is excluded too: it is the conventional end-of-options marker, is
/// not registered on any subcommand, and rejecting it would break an invocation
/// that works today.
fn looks_like_a_flag(token: &str) -> bool {
    token == "-h"
        || token
            .strip_prefix("--")
            .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_alphabetic()))
}

pub fn check(argv: &[String]) -> Outcome {
    let Some(sub) = argv.get(1) else {
        return Outcome::NotOurs;
    };
    let Some(spec) = spec(sub) else {
        return Outcome::NotOurs;
    };

    let mut unknown: Option<String> = None;
    // Set after a recognised value-taking flag: the NEXT token is that flag's
    // value and must not be inspected, however it is spelled. See the module
    // header - a value beginning with a hyphen was reported as an unknown flag.
    let mut expect_value = false;
    for raw in argv.iter().skip(2) {
        if expect_value {
            expect_value = false;
            continue;
        }
        // `--flag=value` is a shape people type even where the parser wants a
        // separate token; validate the name half so it reports the flag rather
        // than the whole pair. It also carries its own value, so it must not
        // additionally consume the token after it.
        let (token, inline_value) = match raw.split_once('=') {
            Some((name, _)) => (name, true),
            None => (raw.as_str(), false),
        };
        if !looks_like_a_flag(token) {
            continue; // positional, or a flag's value
        }
        if token == "--help" || token == "-h" {
            return Outcome::Help(spec.usage);
        }
        if spec.flags.contains(&token) {
            expect_value = !inline_value && !VALUELESS_FLAGS.contains(&token);
            continue;
        }
        if unknown.is_none() {
            unknown = Some(token.to_string());
        }
    }

    match unknown {
        Some(flag) => Outcome::Unknown {
            flag,
            usage: spec.usage,
        },
        None => Outcome::Ok,
    }
}

/// Apply [`check`] to the real argv, printing and exiting where it says to.
///
/// Exit 2 for a bad flag matches the code `main.rs` already uses for an unknown
/// subcommand, so callers see one consistent bad-usage code.
pub fn enforce() {
    let argv: Vec<String> = std::env::args().collect();
    match check(&argv) {
        Outcome::NotOurs | Outcome::Ok => {}
        Outcome::Help(usage) => {
            println!("{usage}");
            std::process::exit(0);
        }
        Outcome::Unknown { flag, usage } => {
            eprintln!(
                "meridian: unknown flag {flag:?}\nusage: {usage}\n\n\
                 Refusing to run rather than ignoring it - an unrecognised flag \
                 used to be silently dropped, which ran the command with its \
                 defaults instead."
            );
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(rest: &[&str]) -> Vec<String> {
        std::iter::once("meridian".to_string())
            .chain(rest.iter().map(|s| s.to_string()))
            .collect()
    }

    /// THE incident, at its worst reachable point. `worklog-post-approved`
    /// takes no arguments and posts real comments to a customer's tracker;
    /// before this module `--help` ran it.
    #[test]
    fn help_never_reaches_a_command_that_writes_to_a_real_tracker() {
        assert_eq!(
            check(&argv(&["worklog-post-approved", "--help"])),
            Outcome::Help(
                "meridian worklog-post-approved   (posts approved worklogs to your tracker)"
            ),
            "--help must be answered, never dispatched - this command posts to Jira"
        );
        assert!(
            matches!(
                check(&argv(&["worklog-post-approved", "--dry-run"])),
                Outcome::Unknown { .. }
            ),
            "an unknown flag must be refused, not dropped so the command runs bare"
        );
    }

    /// The reported incident itself.
    #[test]
    fn the_reported_summarise_incident_is_refused() {
        assert!(matches!(
            check(&argv(&["coding-agent-summarise", "--help"])),
            Outcome::Help(_)
        ));
    }

    /// The other direction, and the one that would be a worse regression than
    /// the bug: every flag these commands genuinely read must still be accepted.
    /// The tray shells out to several of these.
    #[test]
    fn real_invocations_still_pass() {
        for a in [
            vec!["coding-agent-summarise", "--dry-run", "--day", "2026-08-27"],
            vec!["coding-agent-summarise", "--limit", "8"],
            vec![
                "ticket-update",
                "--key",
                "KAN-1",
                "--provider",
                "jira",
                "--field",
                "f",
                "--value",
                "v",
            ],
            vec!["day-summary", "--day", "2026-08-27", "--now"],
            vec!["doctor", "--fix", "--dry-run", "--porcelain"],
            vec!["logs", "--service", "meridian-rust", "-n", "50", "-f"],
            vec!["uninstall", "--purge", "--yes", "--json"],
            vec!["db", "repair"],
            vec![
                "oauth-login",
                "jira",
                "--client-id",
                "abc",
                "--port",
                "1234",
            ],
            vec![
                "telemetry",
                "import",
                "bundle.tar.gz",
                "--endpoint",
                "http://x",
                "--auth",
                "eyJ",
            ],
            vec!["plan-task-done", "--key", "K", "--done", "true"],
            vec!["worklog-post-approved"],
            vec!["tasks-sync"],
        ] {
            assert_eq!(
                check(&argv(&a)),
                Outcome::Ok,
                "a working invocation was rejected: {a:?}"
            );
        }
    }

    /// A flag's value is not a flag, and `--flag=value` reports the name half.
    #[test]
    fn values_are_not_mistaken_for_flags() {
        assert_eq!(
            check(&argv(&["day-summary", "--day", "2026-08-27"])),
            Outcome::Ok
        );
        assert_eq!(
            check(&argv(&["day-summary", "--day=2026-08-27"])),
            Outcome::Ok
        );
        match check(&argv(&["day-summary", "--nope=1"])) {
            Outcome::Unknown { flag, .. } => assert_eq!(flag, "--nope"),
            other => panic!("expected the flag name alone, got {other:?}"),
        }
    }

    /// A value that begins with a hyphen is still a value.
    ///
    /// This shipped broken: `--note "- fixed the bug"` was rejected as
    /// `unrecognised flag "- fixed the bug"` with exit 2. The module header's
    /// claim that values were safe reasoned only about negative numbers and
    /// missed free text, which is exactly what `--note`, `--title`,
    /// `--description` and `--value` carry - and the tray drives
    /// `plan-task-draft --note` with whatever the user typed.
    ///
    /// A guard that wrongly rejects a working command is worse than the missing
    /// guard it replaced, so these cases are pinned individually rather than as
    /// one representative.
    #[test]
    fn a_value_beginning_with_a_hyphen_is_not_a_flag() {
        for (label, args) in [
            (
                "a bulleted note",
                vec!["plan-task-draft", "--note", "- fixed the bug"],
            ),
            (
                "an em-dash-ish title",
                vec!["plan-task-create", "--title", "-- rework"],
            ),
            (
                "a negative-looking value",
                vec![
                    "ticket-update",
                    "--key",
                    "K",
                    "--provider",
                    "jira",
                    "--field",
                    "f",
                    "--value",
                    "-1",
                ],
            ),
            (
                "a diff line as a description",
                vec![
                    "plan-task-edit",
                    "--key",
                    "K",
                    "--description",
                    "-x removed",
                ],
            ),
        ] {
            assert_eq!(
                check(&argv(&args)),
                Outcome::Ok,
                "{label}: a flag's value must never be inspected as a flag"
            );
        }
    }

    /// `--help` inside a VALUE is text, not a request for help.
    ///
    /// Without arity this printed usage and exited 0, so a note containing
    /// `--help` silently drafted nothing.
    #[test]
    fn help_inside_a_value_is_text() {
        assert_eq!(
            check(&argv(&["plan-task-draft", "--note", "--help me name this"])),
            Outcome::Ok
        );
        // ...but asked for on its own it is still help.
        assert!(matches!(
            check(&argv(&["plan-task-draft", "--help"])),
            Outcome::Help(_)
        ));
    }

    /// Consuming a value must not blind the check to a later unknown flag.
    #[test]
    fn an_unknown_flag_after_a_value_is_still_caught() {
        match check(&argv(&["day-summary", "--day", "2026-08-27", "--bogus"])) {
            Outcome::Unknown { flag, .. } => assert_eq!(flag, "--bogus"),
            other => panic!("expected --bogus to be caught, got {other:?}"),
        }
        // `--flag=value` carries its own value and must NOT eat the next token.
        match check(&argv(&["day-summary", "--day=2026-08-27", "--bogus"])) {
            Outcome::Unknown { flag, .. } => assert_eq!(flag, "--bogus"),
            other => panic!("an inline value must not consume the next token, got {other:?}"),
        }
    }

    /// A valueless flag consumes nothing, so what follows is still checked.
    #[test]
    fn a_valueless_flag_does_not_swallow_the_next_token() {
        match check(&argv(&["doctor", "--fix", "--bogus"])) {
            Outcome::Unknown { flag, .. } => assert_eq!(flag, "--bogus"),
            other => panic!("--fix takes no value, so --bogus must be caught, got {other:?}"),
        }
    }

    /// Every entry in [`VALUELESS_FLAGS`] must be a flag some subcommand
    /// actually registers.
    ///
    /// A typo here is silent in the dangerous direction: the flag is never
    /// matched, so it is treated as value-taking and swallows the token after
    /// it, hiding a real unknown flag.
    #[test]
    fn every_valueless_flag_is_registered_somewhere() {
        for f in VALUELESS_FLAGS {
            assert!(
                SPECS.iter().any(|s| s.flags.contains(f)),
                "{f} is in VALUELESS_FLAGS but no subcommand registers it - likely a typo"
            );
        }
    }

    /// Bare `meridian` and `meridian --version` start the daemon; that is the
    /// documented entry point and this module must not interfere with it.
    #[test]
    fn the_daemon_entry_point_is_untouched() {
        assert_eq!(check(&argv(&[])), Outcome::NotOurs);
        assert_eq!(check(&argv(&["--version"])), Outcome::NotOurs);
        // A typo is left to main.rs's unknown-subcommand path, which has a
        // better message for it (it names the stale-binary case).
        assert_eq!(check(&argv(&["restrt"])), Outcome::NotOurs);
    }

    /// THE guard that keeps this fix from decaying. A new subcommand added to
    /// `main.rs` without an entry here would silently return `NotOurs` and get
    /// no flag checking at all - the original hole, reopened one command at a
    /// time. Source-scanned because the dispatch chain is `if` statements, not
    /// data, so there is nothing else to enumerate it from.
    #[test]
    fn every_dispatched_subcommand_is_registered() {
        let main_rs = include_str!("main.rs");
        let mut missing = Vec::new();
        for line in main_rs.lines() {
            // Matches `nth(1).as_deref() == Some("name")`.
            let Some(rest) = line.split("nth(1).as_deref() == Some(\"").nth(1) else {
                continue;
            };
            let Some(name) = rest.split('"').next() else {
                continue;
            };
            if spec(name).is_none() {
                missing.push(name.to_string());
            }
        }
        assert!(
            missing.is_empty(),
            "subcommand(s) dispatched in main.rs with no cli_flags::SPECS entry: {missing:?}\n\
             Add one. Without it the subcommand accepts any flag silently, which is how \
             `worklog-post-approved --help` came to post to a real tracker."
        );
    }

    /// The `plan-task-*` family dispatches from `plan_tasks::cli`, not through
    /// `main.rs`'s chain, so the scan above cannot see it. Pin it separately
    /// rather than leaving the family unguarded.
    #[test]
    fn the_plan_task_family_is_registered() {
        let cli_rs = include_str!("plan_tasks/cli.rs");
        let mut missing = Vec::new();
        for line in cli_rs.lines() {
            let trimmed = line.trim();
            let Some(rest) = trimmed.strip_prefix("\"plan-task-") else {
                continue;
            };
            let Some(tail) = rest.split('"').next() else {
                continue;
            };
            let name = format!("plan-task-{tail}");
            if spec(&name).is_none() {
                missing.push(name);
            }
        }
        assert!(
            missing.is_empty(),
            "plan-task subcommand(s) with no SPECS entry: {missing:?}"
        );
    }

    /// Usage strings are user-facing app text: plain hyphens only, per the
    /// repo's hard rule on dashes in displayed strings.
    #[test]
    fn usage_lines_use_plain_hyphens() {
        for s in SPECS {
            for bad in ['\u{2014}', '\u{2013}'] {
                assert!(
                    !s.usage.contains(bad),
                    "{}'s usage line contains {bad:?} - user-facing text takes a plain hyphen",
                    s.name
                );
            }
        }
    }

    /// `--help` is handled centrally, so listing it in a spec would be dead
    /// weight that implies it is optional per-command.
    #[test]
    fn help_is_not_listed_per_command() {
        for s in SPECS {
            assert!(
                !s.flags.contains(&"--help") && !s.flags.contains(&"-h"),
                "{} lists --help/-h; they are accepted for every subcommand",
                s.name
            );
        }
    }

    /// Duplicate entries would make the table's behaviour depend on order.
    #[test]
    fn no_duplicate_specs() {
        let mut names: Vec<&str> = SPECS.iter().map(|s| s.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate subcommand in SPECS");
    }
}
