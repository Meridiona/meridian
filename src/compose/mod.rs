//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Compose: the key that writes into any text box.
//!
//! The user taps a key while a text box has focus. The tray reads the box, this module
//! decides what kind of box it is and what the user wants, builds the model request,
//! and parses the answer; the tray then delivers it. Everything here is pure - no
//! Accessibility, no windows, no I/O - so the decisions that matter (refuse or write,
//! which surface, which intent, what the prompt says) are covered by plain unit tests.
//!
//! ```text
//! tray: read field ──▶ FieldSnapshot ──▶ classify ──▶ prompt::build ──▶ LLM
//!                                                                         │
//! tray: deliver  ◀── output::parse  ◀──────────────────────────────────────┘
//! ```
//!
//! # Scope of v1
//! Single model call, no tools, no stored memory. Grounding is what is on the screen:
//! the box, the text around it, the rest of its window and other visible windows.
//!
//! # Who calls this
//! The tray's compose controller (`tray/src-tauri/src/compose/controller.rs`), once per press,
//! through [`generate::draft`].
//!
//! # Related
//! - [`types`] - the values exchanged with the tray.
//! - [`classify`] - refuse, or name the surface and intent.
//! - [`prompt`] - the layered, versioned prompt.
//! - [`output`] - parsing the model's answer.
//! - [`crate::llm`] - the provider layer [`generate::draft`] calls, marked interactive.

pub mod classify;
pub mod generate;
pub mod output;
pub mod prompt;
pub mod types;
