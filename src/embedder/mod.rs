//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! On-device sentence embedder — the one model Meridian still runs locally.
//!
//! # What this is
//! A small, self-contained embedding subsystem used by the session distiller
//! ([`crate::worklog_pipeline::distiller`]) for **semantic dedup** (SemDeDup) and
//! facility-location diversity selection of on-screen text spans. It is the sole
//! survivor of the Python/MLX removal: every *generative* task now goes through the
//! CLI provider layer ([`crate::llm`]), but no third-party CLI offers embeddings, so
//! this stays on-device — now in pure Rust (candle) instead of the old MLX server.
//!
//! # Degradation contract
//! When the model is not yet provisioned (first-run weights still downloading) or
//! fails to load, [`is_ready`] returns `false` and the distiller **skips** the vector
//! stages, degrading to lexical dedup + longest-first selection. Capture and the hour
//! pipeline never block on this — a missing embedder means a lower-reduction body, not
//! an error.
//!
//! # Who calls this
//! [`crate::worklog_pipeline::distiller::dedup`] (SemDeDup) and `::select`
//! (facility-location). Weight provisioning is driven at setup time by the tray wizard.
//!
//! NOTE: the candle-backed implementation (`candle_bert.rs`, `provision.rs`, `device.rs`)
//! lands in the next P1 sub-step; today this reports not-ready so the distiller's
//! lexical-degrade path is the one exercised.

use anyhow::Result;

/// Whether the embedding weights are present and the model is loadable. The distiller
/// gates every vector stage on this: `false` → skip SemDeDup and the vector path of
/// facility-location, degrade to lexical dedup + longest-first (always available).
pub fn is_ready() -> bool {
    false
}

/// L2-normalized sentence embeddings — one row vector per input string, so downstream
/// cosine similarity is a plain dot product. Batched internally to bound peak GPU
/// memory. Errors when the model is unavailable; callers MUST gate on [`is_ready`]
/// first and take the lexical-degrade path on `false`.
pub async fn embed_batch(_texts: &[String]) -> Result<Vec<Vec<f32>>> {
    anyhow::bail!("embedder not provisioned (candle backend not yet wired)")
}
