//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! On-device sentence embedder — the one model Meridian still runs locally.
//!
//! # What this is
//! A small, self-contained embedding subsystem (candle + BGE) used by the session
//! distiller ([`crate::worklog_pipeline::distiller`]) for **semantic dedup** (SemDeDup)
//! and facility-location diversity selection of on-screen text spans. Every *generative*
//! task goes through the CLI provider layer ([`crate::llm`]), but no third-party CLI
//! offers embeddings, so this one model stays on-device — in pure Rust (candle).
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
use std::time::Instant;

use candle_bert::Embedder;

/// The lazily-loaded model, shared across the process. `None` until first successful
/// load; the `Mutex` also serializes the single device's forward passes (replacing the
/// old cross-subsystem GPU gate). Never held across an `.await` — all compute runs inside
/// [`embed_batch`]'s blocking task.
static EMBEDDER: Lazy<Mutex<Option<Embedder>>> = Lazy::new(|| Mutex::new(None));

/// Max sequences per forward pass. Self-attention memory is `O(batch * seq_len^2)`, so
/// one hour's whole span list in a single batch (the pre-chunking behaviour) made peak
/// memory scale with how much on-screen text that hour had — multiple GB on a heavy hour.
/// The Python predecessor's actual last-shipped embedder (MLX-native, in
/// `session_distiller.py`, not the earlier sentence-transformers version) chunked at
/// `batch = 16` and called `mx.clear_cache()` after each one — a starting point, not the
/// tuned value: measured against the heaviest real hour on record (856 spans), peak RSS
/// AND wall-clock both kept improving as chunk size shrank further (16 → 1.43GB/27s,
/// 8 → 918MB/22s, 4 → 586MB/20s), most likely because a larger chunk is more likely to
/// contain one unusually long span, forcing BatchLongest to pad every other span in that
/// chunk up to it — wasted compute and memory that a smaller chunk mostly avoids. `4`
/// landed comfortably under the ~1 GB Python was observed to stay under, so that's the
/// default; the mimalloc global allocator in `lib.rs` covers giving the freed pages back
/// to the OS between chunks, since candle frees each one's tensors via RAII regardless.
/// Overridable for benchmarking.
static EMBED_CHUNK_SIZE: Lazy<usize> = Lazy::new(|| {
    std::env::var("EMBEDDER_BATCH_SIZE")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n: &usize| n > 0)
        .unwrap_or(4)
});

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
///
/// Takes ownership rather than `&[String]`: `spawn_blocking` needs a `'static` closure, so
/// a borrowed slice would still have to be cloned here. The one caller
/// ([`crate::worklog_pipeline::distiller`]) already builds a fresh owned `Vec<String>`
/// immediately before calling, so taking it directly saves that clone.
pub async fn embed_batch(texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
    tokio::task::spawn_blocking(move || embed_blocking(&texts))
        .await
        .map_err(|e| anyhow::anyhow!("embedder task panicked: {e}"))?
}

/// Drop the cached model, freeing its weights/tensors. The distiller calls this
/// right after its one `embed_batch` call for the hour (see
/// [`crate::worklog_pipeline::distiller::embed_lex`]) — the model is used once
/// per hour, so there is nothing gained by keeping ~130 MB of weights resident in
/// a background daemon for the other 55-ish minutes until the next hour. The next
/// call to [`embed_batch`] reloads it from disk exactly like the first-ever call.
pub fn unload() {
    let mut guard = EMBEDDER.lock().unwrap_or_else(|e| e.into_inner());
    *guard = None;
}

/// The blocking body: lazily load the model under the mutex, then embed each text in
/// fixed-size chunks (see [`EMBED_CHUNK_SIZE`]) rather than one all-at-once forward pass.
///
/// Recovers from a poisoned lock rather than propagating the poison forever: a panic
/// during one embed (a candle shape mismatch, say) must not permanently strand the
/// distiller on its lexical-only degrade path for the rest of the daemon's lifetime.
/// Matches [`crate::llm::rate_limit`]'s `NEXT_SLOT` handling of the same hazard.
fn embed_blocking(texts: &[String]) -> Result<Vec<Vec<f32>>> {
    let mut guard = EMBEDDER.lock().unwrap_or_else(|e| {
        tracing::warn!("embedder: recovered from poisoned mutex (a prior embed likely panicked)");
        e.into_inner()
    });
    if guard.is_none() {
        let _span = tracing::debug_span!("embedder.load").entered();
        let start = Instant::now();
        match Embedder::load() {
            Ok(e) => {
                tracing::info!(
                    elapsed_ms = start.elapsed().as_millis() as u64,
                    "embedder: model loaded"
                );
                *guard = Some(e);
            }
            Err(e) => {
                tracing::error!(error = %e, "embedder: model load failed");
                return Err(e);
            }
        }
    }
    let embedder = guard.as_ref().expect("just loaded");

    let chunk_size = *EMBED_CHUNK_SIZE;
    let n_chunks = texts.len().div_ceil(chunk_size.max(1));
    let _span = tracing::debug_span!(
        "embedder.batch",
        n_texts = texts.len(),
        chunk_size,
        n_chunks
    )
    .entered();
    let start = Instant::now();
    let mut out = Vec::with_capacity(texts.len());
    for chunk in texts.chunks(chunk_size) {
        let refs: Vec<&str> = chunk.iter().map(String::as_str).collect();
        out.extend(embedder.embed_batch(&refs)?);
    }
    tracing::debug!(
        n_texts = texts.len(),
        elapsed_ms = start.elapsed().as_millis() as u64,
        "embedder: batch embedded"
    );
    Ok(out)
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
        let v = embed_batch(texts).await.expect("embed");
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
