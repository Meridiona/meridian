//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! First-run weight provisioning for the embedder — a slim, self-contained Rust
//! weight downloader.
//!
//! The BGE model is three files (`config.json`, `tokenizer.json`, `model.safetensors`,
//! ~130 MB total) fetched from the HuggingFace CDN via the existing `reqwest` (rustls) —
//! no `hf-hub`, no C++/TLS surprises, works the same on macOS and Windows. Weights live
//! under `~/.meridian/models/<repo>/` (override with `MERIDIAN_EMBEDDER_DIR`), resolved
//! cross-platform via `dirs`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use tokio::io::AsyncWriteExt;

/// The embedding model repo — BGE small (BERT, 384-dim). `MERIDIAN_EMBEDDER_REPO`
/// overrides it for experiments.
pub fn repo() -> String {
    std::env::var("MERIDIAN_EMBEDDER_REPO").unwrap_or_else(|_| "BAAI/bge-small-en-v1.5".to_string())
}

/// The three files that make up the model.
pub const FILES: [&str; 3] = ["config.json", "tokenizer.json", "model.safetensors"];

/// The local directory the model's files live in. `MERIDIAN_EMBEDDER_DIR` overrides it;
/// otherwise `~/.meridian/models/<repo-basename>/` (cross-platform home via `dirs`).
pub fn model_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MERIDIAN_EMBEDDER_DIR") {
        return PathBuf::from(dir);
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let basename = repo().rsplit('/').next().unwrap_or("bge").to_string();
    home.join(".meridian").join("models").join(basename)
}

/// Whether every model file is present and the weights blob is non-empty. Cheap (a stat
/// per file) — drives [`super::is_ready`].
pub fn files_present() -> bool {
    let dir = model_dir();
    FILES.iter().all(|f| {
        let p = dir.join(f);
        match std::fs::metadata(&p) {
            Ok(m) => m.len() > 0,
            Err(_) => false,
        }
    })
}

/// Download any missing model files into [`model_dir`]. Idempotent — a file already
/// present is skipped, so re-running after a partial download resumes the remainder.
/// Streams to a `.part` file and renames on completion so an interrupted download never
/// leaves a truncated blob that would pass [`files_present`].
#[tracing::instrument(name = "embedder.provision")]
pub async fn ensure() -> Result<()> {
    let dir = model_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let repo = repo();
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .build()
        .context("building embedder download client")?;

    for file in FILES {
        let dest = dir.join(file);
        if std::fs::metadata(&dest)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
        {
            continue;
        }
        let url = format!("https://huggingface.co/{repo}/resolve/main/{file}");
        tracing::info!(%url, "embedder: downloading model file");
        download_to(&client, &url, &dest)
            .await
            .with_context(|| format!("downloading {file}"))?;
    }
    tracing::info!(dir = %dir.display(), repo = %repo, "embedder: weights ready");
    Ok(())
}

/// Stream one URL to `dest` via a `.part` temp then rename.
async fn download_to(client: &reqwest::Client, url: &str, dest: &std::path::Path) -> Result<()> {
    let resp = client.get(url).send().await.context("GET failed")?;
    if !resp.status().is_success() {
        anyhow::bail!("{} returned {}", url, resp.status());
    }
    // One-time ~130 MB fetch — read the whole body then write. (Incremental
    // progress-bar streaming can be added with reqwest's `stream` feature later.)
    let bytes = resp.bytes().await.context("reading response body")?;
    let part = dest.with_extension("part");
    let mut file = tokio::fs::File::create(&part)
        .await
        .with_context(|| format!("creating {}", part.display()))?;
    file.write_all(&bytes).await.context("writing file")?;
    file.flush().await.ok();
    drop(file);
    tokio::fs::rename(&part, dest)
        .await
        .with_context(|| format!("finalising {}", dest.display()))?;
    tracing::debug!(bytes = bytes.len(), "embedder: file downloaded");
    Ok(())
}
