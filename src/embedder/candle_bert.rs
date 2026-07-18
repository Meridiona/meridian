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

    /// Embed one string → an L2-normalized vector. CLS-pooled (first token).
    pub fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| anyhow::anyhow!("tokenizing: {e}"))?;
        let ids: Vec<u32> = encoding.get_ids().to_vec();

        let input_ids = Tensor::new(ids.as_slice(), &self.device)?.unsqueeze(0)?; // [1, seq]
        let token_type_ids = input_ids.zeros_like()?;
        // Single sequence, no padding → attention mask is all-ones / unnecessary.
        let hidden = self.model.forward(&input_ids, &token_type_ids, None)?; // [1, seq, H]

        // CLS pooling: the first token's hidden state.
        let cls = hidden.i((0, 0))?.to_dtype(DType::F32)?; // [H]
        let norm = cls.sqr()?.sum_all()?.sqrt()?.to_scalar::<f32>()?;
        let inv = if norm > 0.0 { 1.0 / norm } else { 0.0 };
        let v: Vec<f32> = cls.to_vec1::<f32>()?.into_iter().map(|x| x * inv).collect();
        Ok(v)
    }
}
