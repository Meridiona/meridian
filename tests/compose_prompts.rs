//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Golden prompt snapshots for the compose pipeline.
//!
//! Each scenario is a realistic press (a Slack reply, a Gmail draft, a terminal note...)
//! run through classification and prompt building. The resulting user-half prompt is
//! compared byte for byte with `tests/compose_golden/<name>.txt`, so any change to a
//! prompt file, a budget or the layout is a visible diff in review rather than a silent
//! shift in what the model reads.
//!
//! To accept an intentional change: `UPDATE_GOLDEN=1 cargo test --test compose_prompts`,
//! review the diff, and bump `PROMPT_VERSION` in `src/compose/prompt/builder.rs`.
//!
//! The scenarios also assert the classification they are named for, so a registry change
//! that moves an app to another surface fails loudly here too.

use std::path::PathBuf;

use meridian::compose::classify::{classify, Classification};
use meridian::compose::prompt::{build, PROMPT_VERSION};
use meridian::compose::types::{
    DraftRequest, FieldSnapshot, HeaderField, Intent, NearbyText, PreviousAttempt, SurfaceKind,
};

struct Scenario {
    name: &'static str,
    surface: SurfaceKind,
    intent: Intent,
    request: DraftRequest,
}

fn req(field: FieldSnapshot) -> DraftRequest {
    DraftRequest {
        field,
        previous: None,
        user_name: Some("Adithya Harish".into()),
    }
}

fn base(app: &str, bundle: &str, url: Option<&str>, multiline: bool) -> FieldSnapshot {
    FieldSnapshot {
        app_name: app.into(),
        bundle_id: bundle.into(),
        url: url.map(str::to_string),
        multiline,
        ax_role: if multiline {
            "AXTextArea".into()
        } else {
            "AXTextField".into()
        },
        ..Default::default()
    }
}

fn nearby(above: &str) -> NearbyText {
    NearbyText {
        above: above.into(),
        ..Default::default()
    }
}

const CHROME: &str = "com.google.Chrome";

fn scenarios() -> Vec<Scenario> {
    let slack = || base("Slack", "com.tinyspeck.slackmacgap", None, true);
    let gmail = |title: &str| {
        let mut f = base(
            "Google Chrome",
            CHROME,
            Some("https://mail.google.com/mail/u/0/"),
            true,
        );
        f.window_title = title.into();
        f.label = "Message Body".into();
        f
    };
    let mut out = Vec::new();

    let mut f = slack();
    f.window_title = "platform-eng - Acme - Slack".into();
    f.nearby = nearby("Alex: shipping the connector Thursday\nPriya: can you confirm the rollout plan for the three pilot workspaces?");
    out.push(Scenario {
        name: "slack_reply",
        surface: SurfaceKind::DirectChat,
        intent: Intent::Reply,
        request: req(f),
    });

    let mut f = slack();
    f.window_title = "Priya Nair - Acme - Slack".into();
    out.push(Scenario {
        name: "slack_start_conversation",
        surface: SurfaceKind::DirectChat,
        intent: Intent::StartConversation,
        request: req(f),
    });

    let mut f = gmail("Compose - Meridiona Mail");
    f.header = vec![
        HeaderField {
            label: "To".into(),
            value: "Nabeel Al-Kady".into(),
        },
        HeaderField {
            label: "Subject".into(),
            value: "Intro call next week".into(),
        },
    ];
    f.nearby = nearby("To Nabeel Al-Kady Subject Intro call next week Cc Bcc");
    out.push(Scenario {
        name: "gmail_new_email_start",
        surface: SurfaceKind::EmailCompose,
        intent: Intent::StartConversation,
        request: req(f),
    });

    let mut f = gmail("Re: Walkthrough invite - Meridiona Mail");
    f.header = vec![HeaderField {
        label: "To".into(),
        value: "Stuti Rastogi".into(),
    }];
    f.below_draft = "On Mon, 5 Oct 2026 at 11:06, Stuti Rastogi wrote:\nHi, thank you for connecting. Could we do a walkthrough at 3:30 pm today?".into();
    out.push(Scenario {
        name: "gmail_reply",
        surface: SurfaceKind::EmailCompose,
        intent: Intent::Reply,
        request: req(f),
    });

    let mut f = gmail("Compose - Meridiona Mail");
    f.before = "Hi Rajiv, thanks for pointing me to those contacts. I have already reached out to both of them so no need to follow up on that.".into();
    f.after = "\n\nBest,\nAdithya".into();
    out.push(Scenario {
        name: "gmail_polish_draft",
        surface: SurfaceKind::EmailCompose,
        intent: Intent::Refine,
        request: req(f),
    });

    let mut f = gmail("Compose - Meridiona Mail");
    f.before =
        "polite decline to sarah, say we are heads down on the release until the 20th".into();
    f.header = vec![HeaderField {
        label: "To".into(),
        value: "Sarah Lindqvist".into(),
    }];
    out.push(Scenario {
        name: "gmail_instruction_note",
        surface: SurfaceKind::EmailCompose,
        intent: Intent::Refine,
        request: req(f),
    });

    let mut f = base(
        "Google Chrome",
        CHROME,
        Some("https://www.linkedin.com/messaging/thread/2-abc/"),
        true,
    );
    f.placeholder = "Write a message\u{2026}".into();
    f.header = vec![HeaderField {
        label: "To".into(),
        value: "Nabeel Al-Kady".into(),
    }];
    out.push(Scenario {
        name: "linkedin_dm_start",
        surface: SurfaceKind::DirectChat,
        intent: Intent::StartConversation,
        request: req(f),
    });

    let mut f = base(
        "Google Chrome",
        CHROME,
        Some("https://www.linkedin.com/feed/"),
        true,
    );
    f.kept_prefix = "Jurgen P\u{eb}rgega".into();
    f.nearby = nearby("Jurgen P\u{eb}rgega\nI show teams what a lead did before they filled out the form. Here's a quick look at how it works.");
    out.push(Scenario {
        name: "linkedin_reply_with_tag",
        surface: SurfaceKind::PublicComment,
        intent: Intent::Reply,
        request: req(f),
    });

    let mut f = base(
        "Google Chrome",
        CHROME,
        Some("https://www.linkedin.com/feed/"),
        true,
    );
    f.placeholder = "Add a comment\u{2026}".into();
    f.nearby = nearby("Maya Chen posted: We just closed our seed round to build tooling that turns screen activity into accurate timesheets. Grateful to our early users.");
    out.push(Scenario {
        name: "linkedin_comment_reply",
        surface: SurfaceKind::PublicComment,
        intent: Intent::Reply,
        request: req(f),
    });

    let mut f = base(
        "Google Chrome",
        CHROME,
        Some("https://docs.google.com/document/d/1/edit"),
        true,
    );
    f.before = "The rollout begins with the three smallest workspaces so that we can".into();
    out.push(Scenario {
        name: "gdocs_continue",
        surface: SurfaceKind::Document,
        intent: Intent::Continue,
        request: req(f),
    });

    let mut f = base("Code", "com.microsoft.VSCode", None, true);
    f.window_title = "parse.rs - meridian - Visual Studio Code".into();
    f.before = "/// Returns the host of a page address, lower-cased.\npub fn host_of(url: &str) -> Option<String> {\n".into();
    out.push(Scenario {
        name: "vscode_continue",
        surface: SurfaceKind::CodeEditor,
        intent: Intent::Continue,
        request: req(f),
    });

    let mut f = base("iTerm2", "com.googlecode.iterm2", None, false);
    f.before = "show the ten largest files under this directory".into();
    out.push(Scenario {
        name: "terminal_refine",
        surface: SurfaceKind::Terminal,
        intent: Intent::Refine,
        request: req(f),
    });

    let mut f = base(
        "Google Chrome",
        CHROME,
        Some("https://chatgpt.com/c/123"),
        true,
    );
    f.before = "i want to test this with a real meridian task, what acceptance criteria for the missing signup emails".into();
    out.push(Scenario {
        name: "chatgpt_refine",
        surface: SurfaceKind::AiPrompt,
        intent: Intent::Refine,
        request: req(f),
    });

    let mut f = base(
        "Google Chrome",
        CHROME,
        Some("https://calendar.google.com/calendar/u/0/r/eventedit"),
        false,
    );
    f.placeholder = "Add title".into();
    f.window_title = "Meridiona - Calendar - Event editor".into();
    let r = req(f);
    out.push(Scenario {
        name: "calendar_title_fill",
        surface: SurfaceKind::StructuredField,
        intent: Intent::FillField,
        request: r,
    });

    let mut f = base("Notes", "com.apple.Notes", None, true);
    f.before = "Notes from the call: ".into();
    f.selection = "we agreed that the pilot starts on the 12th and the budget is fixed for the quarter, nothing else is open".into();
    f.after = "\n\nNext steps TBD.".into();
    out.push(Scenario {
        name: "notes_rewrite_selection",
        surface: SurfaceKind::Document,
        intent: Intent::RewriteSelection,
        request: req(f),
    });

    let mut f = slack();
    f.nearby = nearby("Priya: can you confirm the rollout plan for the three pilot workspaces?");
    let mut r = req(f);
    r.previous = Some(PreviousAttempt {
        output: "Confirmed, the plan is staged by workspace size.".into(),
    });
    out.push(Scenario {
        name: "slack_rerun",
        surface: SurfaceKind::DirectChat,
        intent: Intent::Reply,
        request: r,
    });

    // A chat the user spoke in last: Google Chat labels the user "You" and prints a sender label
    // only when the sender changes, so the Meet link under "Today" is the user's own message.
    let mut f = base(
        "Google Chrome",
        CHROME,
        Some("https://chat.google.com/u/0/"),
        true,
    );
    f.window_title = "Akarsh - Mail - Google Chrome".into();
    f.nearby = nearby(
        "Akarsh\ngoldfish has windows app also\nFri 15:29\nYesterday\nYou\nhttps://example.com/call-notes\nYesterday 14:06\nToday\nhttps://meet.google.com/abc-defg-hij\nJoin video meeting, Video call.\n11:41",
    );
    out.push(Scenario {
        name: "google_chat_user_spoke_last",
        surface: SurfaceKind::Generic,
        intent: Intent::Reply,
        request: req(f),
    });

    let f = base("Unknown App", "com.example.unknown", None, true);
    out.push(Scenario {
        name: "generic_unknown_app",
        surface: SurfaceKind::Generic,
        intent: Intent::StartConversation,
        request: req(f),
    });

    out
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("compose_golden")
}

fn render(s: &Scenario) -> String {
    let plan = match classify(&s.request.field) {
        Classification::Ready(p) => p,
        other => panic!("{}: expected a plan, got {other:?}", s.name),
    };
    assert_eq!(plan.surface, s.surface, "{}: surface", s.name);
    assert_eq!(plan.intent, s.intent, "{}: intent", s.name);
    let built = build(&s.request, &plan);
    assert_eq!(built.version, PROMPT_VERSION);
    format!(
        "# scenario={} surface={} intent={} version={}\n\n{}\n",
        s.name,
        plan.surface.as_str(),
        plan.intent.as_str(),
        built.version,
        built.user
    )
}

#[test]
fn prompts_match_their_golden_snapshots() {
    let update = std::env::var_os("UPDATE_GOLDEN").is_some();
    let dir = golden_dir();
    if update {
        std::fs::create_dir_all(&dir).expect("create golden dir");
    }
    let mut failures = Vec::new();
    for s in scenarios() {
        let got = render(&s);
        let path = dir.join(format!("{}.txt", s.name));
        if update {
            std::fs::write(&path, &got).expect("write golden");
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(want) if want == got => {}
            Ok(_) => failures.push(format!("{}: prompt changed", s.name)),
            Err(_) => failures.push(format!(
                "{}: missing golden file {}",
                s.name,
                path.display()
            )),
        }
    }
    assert!(
        failures.is_empty(),
        "golden mismatches (UPDATE_GOLDEN=1 to accept):\n{}",
        failures.join("\n")
    );
}

#[test]
fn every_surface_has_at_least_one_scenario() {
    let covered: std::collections::HashSet<_> = scenarios().iter().map(|s| s.surface).collect();
    for kind in SurfaceKind::ALL {
        assert!(covered.contains(&kind), "no scenario for {kind:?}");
    }
}

#[test]
fn every_intent_has_at_least_one_scenario() {
    let covered: std::collections::HashSet<_> = scenarios().iter().map(|s| s.intent).collect();
    for intent in Intent::ALL {
        assert!(covered.contains(&intent), "no scenario for {intent:?}");
    }
}

#[test]
fn no_scenario_prompt_leaks_an_unresolved_placeholder() {
    for s in scenarios() {
        let text = render(&s);
        assert!(!text.contains("{{") && !text.contains("}}"), "{}", s.name);
    }
}

#[test]
fn the_system_prompt_explains_who_said_what() {
    use meridian::compose::prompt::builder::CORE;
    for needle in [
        "\"You\" or \"Me\"",
        "only when the sender changes",
        "date divider",
    ] {
        assert!(CORE.contains(needle), "core prompt lost: {needle}");
    }
}

#[test]
fn a_reply_task_covers_the_user_having_spoken_last() {
    let plan = match classify(
        &scenarios()
            .iter()
            .find(|s| s.name == "google_chat_user_spoke_last")
            .unwrap()
            .request
            .field,
    ) {
        Classification::Ready(p) => p,
        other => panic!("expected a plan, got {other:?}"),
    };
    let task = build(
        &scenarios()
            .iter()
            .find(|s| s.name == "google_chat_user_spoke_last")
            .unwrap()
            .request,
        &plan,
    )
    .user;
    assert!(task.contains("the last message was the user's own"));
}
