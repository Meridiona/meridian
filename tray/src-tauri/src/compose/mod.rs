//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The tray half of compose: the key tap, the field reader and writer, and the controller.
//!
//! The decisions live in the library (`meridian::compose`); this module is the platform
//! shell around them. Even here the judgement is kept out of the unsafe code: what counts
//! as a tap is [`trigger`], what range a draft replaces and whether it is still safe to
//! write is [`delivery`], and how UTF-16 offsets map to text is [`ranges`] - all pure and
//! tested. The macOS layers only read facts and obey those answers.
//!
//! ```text
//! tap (listen-only) ──▶ controller ──▶ reader ──▶ meridian::compose::generate::draft
//!                                                         │
//!                       feedback ◀── writer ◀── delivery::guard ◀──┘
//! ```
//!
//! # Who calls this
//! `lib.rs`'s setup hook calls [`start`] once, on macOS only.
//!
//! # Related
//! - [`meridian::compose`] - classify, prompt, generate, parse.
//! - [`crate::capture`] - the other Accessibility consumer; unrelated code path.

mod ax;
pub(crate) mod badge;
mod clipboard;
mod controller;
pub(crate) mod delivery;
mod dots;
mod element_tree;
mod feedback;
mod field_text;
mod keys;
mod mention;
pub(crate) mod ranges;
mod reader;
mod sides;
mod synth;
mod tap;
pub(crate) mod trigger;
mod typing;
mod walk;
mod writer;

pub(crate) use controller::{press, start};
