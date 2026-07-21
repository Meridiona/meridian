//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The daily summary — a one-screen review of how a day went.
//!
//! # What this is
//! Meridian already knows WHAT you did: [`meridian_core::day_tasks`] folds each
//! hour into a handful of workstreams with minute-precise segments, and
//! [`meridian_core::hour_text`] carries the per-hour activity reports. Nothing told
//! the developer how their day WENT. This module composes that answer: it collects
//! the day's evidence, asks the user's chosen model to compose a review of it,
//! folds the answer against what the database already knows, and persists it.
//!
//! # The question it answers
//! **Did the day go the way you meant it to?** On a day with a committed plan that
//! is a ledger — one verdict per planned ticket, plus the work that was not planned
//! and is usually the more interesting half. On a day without one it is what the
//! day turned out to be about, grouped from the workstreams themselves.
//!
//! The plan used to be withheld from the model on the grounds that comparing intent
//! to outcome makes a scorecard. The risk was real; withholding was the wrong fix.
//! What keeps this from grading anyone is the prompt's tone contract and a ledger
//! that credits partial work, not ignorance of what they set out to do.
//!
//! # The division of labour
//! The model owns judgement — the prose, which tickets the day's work plausibly
//! advanced, how to group an unplanned day. **Code owns every number.** The model is
//! never asked for a percentage, a count, or a duration, and where the DATABASE
//! settles an outcome (a posted worklog, a linked ticket, a closed ticket) its
//! opinion is not consulted at all. See
//! [`meridian_core::day_evidence::adherence`]. That is
//! `worklog_pipeline::workstream`'s house rule applied here.
//!
//! # The screen always renders
//! An unparseable answer or a failed call degrades to `fallback = 1`: no prose, but
//! the plan ledger and its arithmetic are still shown, because those never needed
//! the model in the first place. A feature whose job is to make someone feel good
//! about their day must never greet them with an error.
//!
//! # There are no charts here any more
//! This screen once stored model-authored Vega-Lite specs and rendered them. It
//! read as a monitoring dashboard, which is the one thing it must not be. Migration
//! 068 dropped the specs, the vendored 1.8 MB grammar, the two-layer spec validator
//! and the chart runtime with them.
//!
//! # Layout
//! - [`generate`] — the LLM call, the fold, the persist.
//!
//! The evidence itself lives in [`meridian_core::day_evidence`] rather than here:
//! it is a pure DB read the tray also serves directly, and one definition of "what
//! happened that day" is worth more than co-location.
//!
//! # Related
//! - [`meridian_core::day_summaries`] — the DB shape and the wire contract.
//! - [`crate::pm_worklog::generate`] — the sibling generate flow this mirrors.

pub mod generate;
