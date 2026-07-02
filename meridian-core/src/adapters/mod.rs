//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Provider adapters — Step 2 of the CDM migration (DRAFT sketch).
//!
//! Each tracker implements [`ProviderAdapter`] to map its raw API payload (one
//! issue / card / work-item, exactly as the tracker returned it) onto the
//! canonical [`CanonicalTask`] shape. This is the single enforcement point for
//! the `canonical_id` invariant: adapters MUST derive `canonical_id` via
//! [`CanonicalTask::canonical_id_for`] rather than setting it freely, so a
//! `CanonicalTask` produced by an adapter can never carry a mismatched key.
//!
//! Adapters are **pure transformations** — no HTTP, no DB, no I/O. Fetching the
//! payload stays in the daemon (which owns `reqwest`); the adapter only maps an
//! already-fetched `serde_json::Value`. That keeps this layer in `meridian-core`
//! (DB-free, like [`crate::hygiene`]) and trivially unit-testable.
//!
//! # Who calls this
//! The daemon's ingestion connectors (`src/intelligence/providers/*`): each
//! one's `cdm_columns()` helper calls [`ProviderAdapter::to_canonical`] on the
//! raw fetched payload at upsert time to derive the CDM columns
//! (migration 056) alongside the legacy typed-struct path.
//!
//! # Related
//! - [`crate::canonical_task`] — the output shape these produce.
//! - [`jira`] — numeric id as the stable key, 3-native-category status
//!   resolution, GDPR-hidden emails.
//! - [`linear`] — UUID id, inverted Int priority lookup, `WorkflowState.type`
//!   → category (custom "In Review" folds into In-Progress).
//! - [`azure_devops`] — org-namespaced id (`{org}:{id}`), Int 1–4 priority,
//!   state→category by name, semicolon-delimited tags.
//! - [`github`] — global node id, board Status column verbatim (no category
//!   while OPEN; CLOSED derives Done/Cancelled from `stateReason`).

use crate::canonical_task::{CanonicalTask, Provider};
use serde_json::Value;

pub mod azure_devops;
pub mod github;
pub mod jira;
pub mod linear;

/// Maps a tracker's raw API payload onto the canonical task shape.
///
/// Implementors are responsible for the per-tracker normalisation traps
/// (stable-id selection, status-category derivation, priority lookup tables,
/// hierarchy flattening) documented in each adapter module. The contract:
///
/// - `canonical_id` MUST be built with [`CanonicalTask::canonical_id_for`] from
///   the same `provider`/`provider_id` the adapter sets — never freely assigned.
/// - The full input is preserved verbatim in `raw_payload`, and any
///   tracker-specific field that doesn't map to the core goes in `custom_fields`
///   — normalisation is never lossy.
/// - Best-effort fields (`status_category`, `kind`, `priority`) may degrade to
///   `None`/`Other`/`Priority::None`; the verbatim `*_raw` companions still
///   carry the original.
pub trait ProviderAdapter {
    /// The provider this adapter handles. Used to namespace canonical ids.
    fn provider(&self) -> Provider;

    /// Map a single raw payload into the canonical shape.
    ///
    /// Returns an error only when the payload is structurally unusable (e.g.
    /// missing the stable id) — recoverable gaps map to best-effort defaults
    /// rather than failing.
    fn to_canonical(&self, raw: &Value) -> anyhow::Result<CanonicalTask>;

    /// Map a batch, isolating per-item failures so one malformed payload can't
    /// sink the whole ingestion run. Provided default; adapters rarely override.
    fn to_canonical_many(&self, raws: &[Value]) -> Vec<anyhow::Result<CanonicalTask>> {
        raws.iter().map(|raw| self.to_canonical(raw)).collect()
    }
}
