//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! The candle BERT embedder — model construction, forward pass, pooling, normalization.
//!
//! Loads BGE small (BERT arch) from the provisioned safetensors + tokenizer and produces
//! one L2-normalized 384-dim vector per input string (CLS pooling — BGE's intended
//! pooling), so downstream cosine similarity is a plain dot product. Sequences are
//! truncated to the model's 512-token limit.
//!
//! Compute runs on the selected device ([`super::device`]); the caller serializes access
//! (one embedder, one hour at a time).

use anyhow::{Context, Result};
use candle_core::{DType, Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config, DTYPE};
use tokenizers::Tokenizer;

use super::provision;

/// Max sequence length (BGE / BERT-base positional limit).
const MAX_LEN: usize = 512;

/// A loaded embedding model: the BERT weights, its tokenizer, and the device they live
/// on. Construct once via [`Embedder::load`]; reuse for every hour.
pub struct Embedder {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
}

impl Embedder {
    /// Load the model from the provisioned files (see [`provision::model_dir`]). Errors if
    /// the weights are absent (caller gates on [`super::is_ready`]) or malformed.
    pub fn load() -> Result<Self> {
        let dir = provision::model_dir();
        let (device, backend) = super::device::select();

        let config: Config = {
            let text = std::fs::read_to_string(dir.join("config.json"))
                .with_context(|| format!("reading {}/config.json", dir.display()))?;
            serde_json::from_str(&text).context("parsing bert config.json")?
        };

        let mut tokenizer = Tokenizer::from_file(dir.join("tokenizer.json"))
            .map_err(|e| anyhow::anyhow!("loading tokenizer.json: {e}"))?;
        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: MAX_LEN,
                ..Default::default()
            }))
            .map_err(|e| anyhow::anyhow!("configuring tokenizer truncation: {e}"))?;
        // BatchLongest (the default): pad every sequence in a batch up to the longest one
        // in THAT batch, not to MAX_LEN — an hour's spans are mostly short OCR lines, so
        // padding to the batch max keeps the tensor small instead of paying for 512 columns
        // on every call.
        tokenizer.with_padding(Some(tokenizers::PaddingParams::default()));

        let weights = dir.join("model.safetensors");
        // SAFETY: the file is our own downloaded, verified-present safetensors blob.
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[weights], DTYPE, &device)
                .context("mmapping model.safetensors")?
        };
        let model = BertModel::load(vb, &config).context("constructing BertModel")?;

        tracing::info!(
            dir = %dir.display(),
            backend,
            dim = config.hidden_size,
            "embedder.load: model ready"
        );
        Ok(Self {
            model,
            tokenizer,
            device,
        })
    }

    /// Embed a batch of strings in ONE forward pass → one L2-normalized vector per input,
    /// same order. CLS-pooled (first token) — correct only because CLS is real content in
    /// every row (the tokenizer never truncates it away) and, being right-padded, position
    /// 0 is identical whether or not a row also carries trailing pad tokens.
    ///
    /// Batching (rather than one forward pass per string) is what makes this cheap on an
    /// hour's worth of spans: the model's cost is dominated by the matmuls, which scale
    /// with the PADDED batch shape, not with `texts.len()` separate calls.
    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let encodings = self
            .tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|e| anyhow::anyhow!("tokenizing batch: {e}"))?;

        // Padding gives every encoding the same length (BatchLongest, set in `load`), so a
        // single rectangular tensor holds the whole batch.
        let seq_len = encodings.first().map(|e| e.get_ids().len()).unwrap_or(0);
        let mut ids = Vec::with_capacity(texts.len() * seq_len);
        let mut mask = Vec::with_capacity(texts.len() * seq_len);
        for enc in &encodings {
            ids.extend_from_slice(enc.get_ids());
            mask.extend(enc.get_attention_mask().iter().map(|&m| m as f32));
        }

        let input_ids =
            Tensor::new(ids.as_slice(), &self.device)?.reshape((texts.len(), seq_len))?; // [B, seq]
        let token_type_ids = input_ids.zeros_like()?;
        let attention_mask =
            Tensor::new(mask.as_slice(), &self.device)?.reshape((texts.len(), seq_len))?;
        let hidden = self
            .model
            .forward(&input_ids, &token_type_ids, Some(&attention_mask))?; // [B, seq, H]

        // CLS pooling per row, then L2-normalize each — same math as the single-item path,
        // just applied per row of the batch instead of once.
        let mut out = Vec::with_capacity(texts.len());
        for b in 0..texts.len() {
            let cls = hidden.i((b, 0))?.to_dtype(DType::F32)?; // [H]
            let norm = cls.sqr()?.sum_all()?.sqrt()?.to_scalar::<f32>()?;
            let inv = if norm > 0.0 { 1.0 / norm } else { 0.0 };
            out.push(cls.to_vec1::<f32>()?.into_iter().map(|x| x * inv).collect());
        }
        Ok(out)
    }
}
