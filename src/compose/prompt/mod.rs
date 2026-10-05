//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Building the model request for a classified press.
//!
//! [`builder::build`] turns a [`crate::compose::types::DraftRequest`] and its
//! [`crate::compose::classify::Plan`] into a [`builder::BuiltPrompt`]: one static system
//! prompt plus a user half of labelled sections. [`budget`] keeps each section inside its
//! size limit while preserving the text closest to the box.
//!
//! # Who calls this
//! The compose pipeline, between classification and the LLM call.
//!
//! # Related
//! - [`crate::compose::classify`] - produces the plan this consumes.
//! - [`crate::compose::output`] - parses the model's answer.

pub mod budget;
pub mod builder;

pub use budget::{OMITTED_AFTER, OMITTED_BEFORE};
pub use builder::{build, BuiltPrompt, PromptStats, CURSOR_MARKER, PROMPT_VERSION};
