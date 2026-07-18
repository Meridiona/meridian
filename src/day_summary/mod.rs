//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The daily summary — an AI-composed, one-screen review of how a day went.
//!
//! # What this is
//! Meridian already knows WHAT you did: [`meridian_core::day_tasks`] folds each
//! hour into a handful of workstreams with minute-precise segments, and
//! [`meridian_core::hour_text`] carries the per-hour activity reports. Nothing
//! told the developer how their day WENT. This module composes that answer: it
//! collects the day's evidence, asks the user's chosen model to compose a review
//! of it, validates what comes back, and persists it.
//!
//! It is deliberately **not** a worklog and **not** monitoring. The daily plan is
//! never passed to the model — this is about what you did, not what you promised.
//!
//! # The prose is the feature
//! This is a few honest sentences about what the day was about, not a chart grid.
//! The model is told never to replay the day in order or quote a clock time — the
//! timeline beside it already shows the when, and reading it back at the person who
//! just lived it is worthless. Charts are OPTIONAL (0-2, the model's call) and most
//! days need none.
//!
//! # The division of labour
//! The model chooses WHETHER to draw anything, HOW MANY, WHAT FORM each takes, and
//! writes the prose. It never supplies a number: specs bind data by name and the
//! frontend injects the real rows at render ([`validate`] enforces this). That is
//! `worklog_pipeline::workstream`'s house rule — the model owns judgement, code
//! owns plumbing — applied to visualisation.
//!
//! # The screen always renders
//! An unparseable answer or a failed call degrades to a single deterministic chart
//! with `fallback = 1`. A feature whose job is to make someone feel good about their
//! day must never greet them with an error. Precedent: the hourly-report path
//! degrades to an even split rather than failing the hour.
//!
//! Note what `fallback` is NOT: an empty `panels` list is a correct answer, not a
//! degraded one.
//!
//! # Layout
//! - [`validate`] — the two-layer spec check (real Vega-Lite schema + our rules).
//! - [`generate`] — the LLM call and everything above, wired together.
//!
//! The evidence itself lives in [`meridian_core::day_evidence`] rather than here:
//! it is a pure DB read that the tray also needs (to inject the real rows into a
//! stored spec at render), and one definition of "what happened that day" is worth
//! more than co-location.
//!
//! # Related
//! - [`meridian_core::day_summaries`] — the DB shape and the wire contract.
//! - [`crate::pm_worklog::generate`] — the sibling generate flow this mirrors.

pub mod generate;
pub mod validate;
