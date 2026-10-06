//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Classifying a press: refuse it, or say what kind of box it is and what the user wants.
//!
//! This is the whole of "works in any text box". Every press goes through
//! [`classify`] and comes out as either a [`Classification::Refused`] (a privacy or
//! safety call) or a [`Classification::Ready`] carrying a [`SurfaceKind`] and an
//! [`Intent`]. There is no third outcome: an unknown app is a generic surface, and
//! generic surfaces get a sensible default, so no text box is a dead end.
//!
//! # Who calls this
//! The compose pipeline, once per press, before any prompt is built.
//!
//! # Related
//! - [`page`] - host and path parsing for site rules.
//! - [`surface`] - the registry and the refusal rules.
//! - [`intent`] - what the user is trying to do.
//! - [`crate::compose::prompt`] - consumes the result.

pub mod intent;
pub mod page;
pub mod surface;

use crate::compose::types::{FieldSnapshot, Intent, RefusalReason, SurfaceKind};

pub use page::Page;

/// A classified press that will go to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub surface: SurfaceKind,
    pub intent: Intent,
    /// The parsed page, kept so later stages do not re-parse the address.
    pub page: Page,
}

/// The outcome of classifying a press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classification {
    /// Do nothing and tell the user why.
    Refused(RefusalReason),
    /// Write something.
    Ready(Plan),
}

/// Classify a press. Pure: the same snapshot always gives the same answer.
#[tracing::instrument(skip_all, fields(surface, intent, refused))]
pub fn classify(field: &FieldSnapshot) -> Classification {
    if let Some(reason) = surface::refusal(field) {
        tracing::Span::current().record("refused", reason.as_str());
        return Classification::Refused(reason);
    }
    let page = Page::parse(field.url.as_deref());
    let kind = surface::surface_kind(field, &page);
    let intent = intent::detect(field, kind);
    tracing::Span::current().record("surface", kind.as_str());
    tracing::Span::current().record("intent", intent.as_str());
    Classification::Ready(Plan {
        surface: kind,
        intent,
        page,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::types::NearbyText;

    #[test]
    fn password_field_is_refused_before_anything_else() {
        let f = FieldSnapshot {
            bundle_id: "com.tinyspeck.slackmacgap".into(),
            ax_subrole: "AXSecureTextField".into(),
            ..Default::default()
        };
        assert_eq!(
            classify(&f),
            Classification::Refused(RefusalReason::SecureField)
        );
    }

    #[test]
    fn slack_reply_is_direct_chat_reply() {
        let f = FieldSnapshot {
            bundle_id: "com.tinyspeck.slackmacgap".into(),
            multiline: true,
            nearby: NearbyText {
                above: "Priya: can you confirm the rollout plan for Thursday?".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        match classify(&f) {
            Classification::Ready(p) => {
                assert_eq!(p.surface, SurfaceKind::DirectChat);
                assert_eq!(p.intent, Intent::Reply);
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_app_still_gets_a_plan() {
        let f = FieldSnapshot {
            bundle_id: "com.example.never-heard-of-it".into(),
            multiline: true,
            ..Default::default()
        };
        assert!(matches!(
            classify(&f),
            Classification::Ready(Plan {
                surface: SurfaceKind::Generic,
                intent: Intent::StartConversation,
                ..
            })
        ));
    }

    #[test]
    fn classification_is_deterministic() {
        let f = FieldSnapshot {
            before: "hello".into(),
            multiline: true,
            ..Default::default()
        };
        assert_eq!(classify(&f), classify(&f));
    }
}
