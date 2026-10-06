//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Unit tests for the prompt builder, kept beside it so `builder.rs` stays under the file-size rule.

use super::*;
use crate::compose::classify::{classify, Classification};
use crate::compose::types::{FieldSnapshot, HeaderField, NearbyText, PreviousAttempt};

fn plan_for(f: &FieldSnapshot) -> Plan {
    match classify(f) {
        Classification::Ready(p) => p,
        other => panic!("expected Ready, got {other:?}"),
    }
}

fn request(field: FieldSnapshot) -> (DraftRequest, Plan) {
    let plan = plan_for(&field);
    (
        DraftRequest {
            field,
            previous: None,
            user_name: None,
        },
        plan,
    )
}

fn slack_reply() -> FieldSnapshot {
    FieldSnapshot {
        app_name: "Slack".into(),
        bundle_id: "com.tinyspeck.slackmacgap".into(),
        multiline: true,
        nearby: NearbyText {
            above: "Priya: can you confirm the rollout plan for Thursday?".into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn system_prompt_is_static_and_user_half_carries_the_press() {
    let (r, p) = request(slack_reply());
    let b = build(&r, &p);
    assert_eq!(b.system, CORE);
    assert!(b.user.contains("## Box"));
    assert!(b.user.contains("## Task"));
    assert!(b.user.contains("rollout plan for Thursday"));
    assert_eq!(b.version, PROMPT_VERSION);
}

#[test]
fn empty_sections_are_omitted_not_shown_empty() {
    let (r, p) = request(slack_reply());
    let b = build(&r, &p);
    assert!(!b.user.contains("Compose header"));
    assert!(!b.user.contains("Previous attempt"));
    assert!(!b.user.contains("Text below the box"));
}

#[test]
fn compose_header_is_included_when_present() {
    let mut f = slack_reply();
    f.header = vec![HeaderField {
        label: "To".into(),
        value: "Nabeel Al-Kady".into(),
    }];
    let (r, p) = request(f);
    assert!(build(&r, &p).user.contains("To: Nabeel Al-Kady"));
}

#[test]
fn an_empty_box_says_so_and_a_typed_box_marks_the_cursor() {
    let (r, p) = request(slack_reply());
    assert!(build(&r, &p).user.contains("## The box\n(empty)"));
    let mut f = slack_reply();
    f.before = "ok sounds".into();
    f.after = " good".into();
    let (r, p) = request(f);
    let u = build(&r, &p).user;
    assert!(u.contains(&format!("ok sounds{CURSOR_MARKER} good")));
}

#[test]
fn selection_is_split_into_before_highlight_after() {
    let mut f = slack_reply();
    f.before = "Hello ".into();
    f.selection = "wrold".into();
    f.after = " team".into();
    let (r, p) = request(f);
    let u = build(&r, &p).user;
    assert!(u.contains("Highlighted text (your output replaces only this)\nwrold"));
    assert!(u.contains("Text before the highlight"));
    assert!(u.contains("Text after the highlight"));
    assert!(!u.contains(CURSOR_MARKER));
}

#[test]
fn broad_capture_is_labelled_noisier() {
    let mut f = slack_reply();
    f.nearby.broad = true;
    let (r, p) = request(f);
    assert!(build(&r, &p).user.contains("noisier than usual"));
}

#[test]
fn previous_attempt_appears_when_given() {
    let field = slack_reply();
    let plan = plan_for(&field);
    let req = DraftRequest {
        field,
        previous: Some(PreviousAttempt {
            output: "Sure, Thursday works.".into(),
        }),
        user_name: Some("Adithya Harish".into()),
    };
    let u = build(&req, &plan).user;
    assert!(u.contains("Previous attempt"));
    assert!(u.contains("Sure, Thursday works."));
}

#[test]
fn huge_context_is_bounded_and_keeps_the_closest_text() {
    let mut f = slack_reply();
    let filler: String = (0..2000)
        .map(|i| format!("old message number {i}\n"))
        .collect();
    f.nearby.above = format!("{filler}Priya: the LAST message before the box");
    let (r, p) = request(f);
    let b = build(&r, &p);
    assert!(b.user.contains("the LAST message before the box"));
    assert!(b.stats.above_chars <= ABOVE_CHARS + 64);
    assert!(b
        .user
        .contains(crate::compose::prompt::budget::OMITTED_BEFORE));
}

#[test]
fn header_echo_lines_are_dropped_but_real_messages_that_mention_the_name_survive() {
    let mut f = slack_reply();
    f.header = vec![HeaderField {
        label: "To".into(),
        value: "Nabeel Al-Kady".into(),
    }];
    f.nearby.above = "To Nabeel Al-Kady Cc Bcc\nPriya: tell Nabeel Al-Kady that Thursday works for the long rollout review across all three pilot workspaces, as discussed earlier this week with everyone on the platform team".into();
    let (r, p) = request(f);
    let u = build(&r, &p).user;
    assert!(!u.contains("To Nabeel Al-Kady Cc Bcc"));
    assert!(u.contains("tell Nabeel Al-Kady that Thursday works"));
}

#[test]
fn short_header_values_do_not_eat_unrelated_lines() {
    let mut f = slack_reply();
    f.header = vec![HeaderField {
        label: "To".into(),
        value: "Al".into(),
    }];
    f.nearby.above = "Priya: can you confirm the rollout plan, also Alex joined".into();
    let (r, p) = request(f);
    assert!(build(&r, &p).user.contains("also Alex joined"));
}

#[test]
fn chrome_memory_note_is_removed_from_the_window_title() {
    assert_eq!(
        clean_title(
            "(4) Messaging | LinkedIn - High memory usage - 923 MB - Google Chrome - Aditya"
        ),
        "(4) Messaging | LinkedIn - Google Chrome - Aditya"
    );
    assert_eq!(clean_title("Inbox - High memory usage - 1.7 GB"), "Inbox");
    assert_eq!(clean_title("Plain title"), "Plain title");
}

#[test]
fn text_below_the_box_is_dropped_for_chat_and_kept_for_documents() {
    let mut chat = slack_reply();
    chat.nearby.below = "More inboxes\nPost a free job\nAbout".into();
    let (r, p) = request(chat);
    assert!(!build(&r, &p).user.contains("Post a free job"));

    let mut doc = FieldSnapshot {
        bundle_id: "com.apple.Notes".into(),
        multiline: true,
        before: "Intro paragraph.".into(),
        ..Default::default()
    };
    doc.nearby.below = "Conclusion paragraph that follows.".into();
    let (r, p) = request(doc);
    assert!(build(&r, &p)
        .user
        .contains("Conclusion paragraph that follows."));
}

#[test]
fn duplicated_labels_in_the_thread_are_collapsed() {
    let mut f = slack_reply();
    f.nearby.above =
        "View Ann's profile\nView Ann's profile Ann Lee\nAnn Lee\n5:32 PM\nCan you send the deck over?".into();
    let (r, p) = request(f);
    let u = build(&r, &p).user;
    assert_eq!(u.matches("Ann Lee").count(), 1);
    assert!(u.contains("Can you send the deck over?"));
}

#[test]
fn the_users_name_is_sent_when_known_and_omitted_otherwise() {
    let (mut r, p) = request(slack_reply());
    assert!(!build(&r, &p).user.contains("## The user"));
    r.user_name = Some("  Adithya Harish ".into());
    assert!(build(&r, &p)
        .user
        .contains("## The user\nName: Adithya Harish"));
    r.user_name = Some("   ".into());
    assert!(!build(&r, &p).user.contains("## The user"));
}

#[test]
fn rest_of_window_appears_only_when_present() {
    let (r, p) = request(slack_reply());
    let u = build(&r, &p).user;
    assert!(!u.contains("Rest of this window"));

    let mut f = slack_reply();
    f.nearby.rest_of_window = "Sidebar: #platform-eng  #random".into();
    let (r, p) = request(f);
    let b = build(&r, &p);
    assert!(b.user.contains("Sidebar: #platform-eng"));
}

#[test]
fn page_section_names_the_site_and_field() {
    let mut f = slack_reply();
    f.url = Some("https://www.linkedin.com/messaging/thread/1".into());
    f.placeholder = "Write a message".into();
    let (r, p) = request(f);
    let u = build(&r, &p).user;
    assert!(u.contains("Site: linkedin.com/messaging\n"));
    assert!(!u.contains("thread/1"), "opaque thread ids are not sent");
    assert!(u.contains("Field placeholder: Write a message"));
}

#[test]
fn every_surface_and_intent_block_exists_and_is_nonempty() {
    for k in SurfaceKind::ALL {
        assert!(!surface_block(k).trim().is_empty(), "{k:?}");
    }
    for i in Intent::ALL {
        assert!(!intent_block(i).trim().is_empty(), "{i:?}");
    }
}

#[test]
fn prompt_assets_contain_no_em_or_en_dashes() {
    let mut all = String::from(CORE);
    for k in SurfaceKind::ALL {
        all.push_str(surface_block(k));
    }
    for i in Intent::ALL {
        all.push_str(intent_block(i));
    }
    assert!(!all.contains('\u{2014}') && !all.contains('\u{2013}'));
}

fn email_request(header: Vec<(&str, &str)>) -> (DraftRequest, Plan) {
    let mut f = slack_reply();
    f.header = header
        .into_iter()
        .map(|(label, value)| HeaderField {
            label: label.into(),
            value: value.into(),
        })
        .collect();
    request(f)
}

#[test]
fn a_line_that_only_restates_the_header_is_dropped() {
    let (r, _) = email_request(vec![("To", "Nabeel Al-Kady"), ("Subject", "Intro call")]);
    let text = "To Nabeel Al-Kady Subject Intro call Cc Bcc\nNabeel: can we move it to Friday?";
    let out = drop_header_echo(text, &r);
    assert!(!out.contains("Cc Bcc"));
    assert!(out.contains("can we move it to Friday?"));
}

#[test]
fn a_conversation_line_that_mentions_one_header_value_is_kept() {
    let (r, _) = email_request(vec![("To", "Nabeel Al-Kady"), ("Subject", "Update")]);
    let text = "Nabeel Al-Kady: sending the Update tomorrow after the standup finishes";
    // Mentions both values but is a real sentence far longer than the header itself.
    assert_eq!(drop_header_echo(text, &r), text);
    let single = "Update on the launch plan is due Friday, said Priya, in the meeting this morning";
    assert_eq!(drop_header_echo(single, &r), single);
}

#[test]
fn without_a_header_nothing_is_dropped() {
    let (r, _) = email_request(vec![]);
    assert_eq!(drop_header_echo("anything at all", &r), "anything at all");
}
