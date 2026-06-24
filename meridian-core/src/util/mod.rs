//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Pure, DB-free helpers shared across the readers — wall-clock interval math,
//! local-day boundaries, and board-hygiene reason mapping.
//!
//! These modules are re-exported at the crate root (`meridian_core::intervals`,
//! `::date`, `::hygiene`) so the public API is unchanged; `util` is internal
//! organization only.

/// Wall-clock interval math shared by the dashboard routes (ported from intervals.ts).
pub mod intervals;

/// Local-day boundary helpers for the dashboard routes (ported from date-utils.ts).
pub mod date;

/// Board-hygiene reason → hint/fix mapping (ported from lib/hygiene.ts).
pub mod hygiene;
