//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! On-device sentence embedder — the one model Meridian still runs locally.
//!
//! # What this is
//! A small, self-contained embedding subsystem (candle + BGE) used by the session
//! distiller ([`crate::worklog_pipeline::distiller`]) for **semantic dedup** (SemDeDup)
//! and facility-location diversity selection of on-screen text spans. It is the sole
//! survivor of the Python/MLX removal: every *generative* task goes through the CLI
//! provider layer ([`crate::llm`]), but no third-party CLI offers embeddings, so this
//! stays on-device — now in pure Rust instead of the old MLX server.
//!
//! # Degradation contract
//! When the model is not yet provisioned (first-run weights still downloading) or fails
//! to load, [`is_ready`] returns `false` and the distiller **skips** the vector stages,
//! degrading to lexical dedup + longest-first selection. The hour pipeline never blocks
//! on this — a missing embedder means a lower-reduction body, not an error.
//!
//! # Shape
//! - [`is_ready`] — cheap file-presence check (drives the distiller's gate + UI readiness).
//! - [`embed_batch`] — L2-normalized vectors; lazily loads the model on first use and
//!   caches it, running compute on a blocking thread so it never stalls the async runtime.
//! - [`ensure_weights`] — first-run download, driven by the setup wizard.
//!
//! # Who calls this
//! [`crate::worklog_pipeline::distiller`] (SemDeDup + facility-location). Provisioning is
//! driven at setup time by the tray wizard.

mod candle_bert;
mod device;
mod provision;

use anyhow::Result;
use once_cell::sync::Lazy;
use std::sync::Mutex;

use candle_bert::Embedder;

/// The lazily-loaded model, shared across the process. `None` until first successful
/// load; the `Mutex` also serializes the single device's forward passes (replacing the
/// old cross-subsystem GPU gate). Never held across an `.await` — all compute runs inside
/// [`embed_batch`]'s blocking task.
static EMBEDDER: Lazy<Mutex<Option<Embedder>>> = Lazy::new(|| Mutex::new(None));

/// Whether the embedding weights are present on disk (so the model can load). The
/// distiller gates every vector stage on this; `false` → lexical-only degrade.
pub fn is_ready() -> bool {
    provision::files_present()
}

/// Download the model weights if missing (first-run provisioning). Idempotent; driven by
/// the setup wizard. See [`provision::ensure`].
pub async fn ensure_weights() -> Result<()> {
    provision::ensure().await
}

/// L2-normalized sentence embeddings — one row vector per input string, so downstream
/// cosine similarity is a plain dot product. Lazily loads (and caches) the model on first
/// use; compute runs on a blocking thread. Errors when the model can't load; callers MUST
/// gate on [`is_ready`] and take the lexical-degrade path on failure.
pub async fn embed_batch(texts: &[String]) -> Result<Vec<Vec<f32>>> {
    let owned: Vec<String> = texts.to_vec();
    tokio::task::spawn_blocking(move || embed_blocking(&owned))
        .await
        .map_err(|e| anyhow::anyhow!("embedder task panicked: {e}"))?
}

/// The blocking body: lazily load the model under the mutex, then embed each text.
fn embed_blocking(texts: &[String]) -> Result<Vec<Vec<f32>>> {
    let mut guard = EMBEDDER
        .lock()
        .map_err(|_| anyhow::anyhow!("embedder mutex poisoned"))?;
    if guard.is_none() {
        *guard = Some(Embedder::load()?);
    }
    let embedder = guard.as_ref().expect("just loaded");
    texts.iter().map(|t| embedder.embed_one(t)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dot(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    /// End-to-end candle check: downloads the weights (~130 MB) and confirms a
    /// near-duplicate pair scores higher cosine than an unrelated one. Ignored by default
    /// (network + model load); run manually:
    /// `cargo test -p meridian embedder::tests -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn embedder_cosine_sanity() {
        ensure_weights().await.expect("provision weights");
        assert!(is_ready(), "weights should be present after ensure");

        let texts = vec![
            "Implemented the Rust session distiller today".to_string(),
            "Wrote the new session distiller in Rust".to_string(),
            "Cooked pasta and watched a film in the evening".to_string(),
        ];
        let v = embed_batch(&texts).await.expect("embed");
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].len(), 384, "bge-small is 384-dim");

        let near = dot(&v[0], &v[1]);
        let far = dot(&v[0], &v[2]);
        println!("near-dup cosine = {near:.3}, unrelated cosine = {far:.3}");
        assert!(
            near > far,
            "near-duplicate pair must be more similar than unrelated"
        );
        assert!(
            near > 0.7,
            "near-duplicate cosine should be high, got {near}"
        );
    }
}
