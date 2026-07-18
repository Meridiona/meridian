//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Compute-device selection for the embedder.
//!
//! The embedder runs on **CPU**. candle 0.8's Metal backend has no `layer-norm` kernel,
//! which BERT (BGE) requires, so Metal can't run this model; and at the distiller's
//! volume (a few hundred short spans once an hour) CPU is sub-second, so a GPU buys
//! nothing. Keeping it CPU-only also makes the build and behaviour identical on macOS,
//! Windows, and Linux.
//!
//! If a future candle gains the needed Metal kernels (or a CUDA build is wanted), this is
//! the one place to reintroduce device selection with a CPU fallback.

use candle_core::Device;

/// The device the embedder runs on, with a name for span attributes. Always CPU today.
pub fn select() -> (Device, &'static str) {
    (Device::Cpu, "cpu")
}
