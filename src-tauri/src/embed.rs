//! The sentence embedder: multilingual-e5-small, on the CPU, in pure Rust.
//!
//! This module is used from BOTH sides of the corpus — the offline builder embeds every
//! passage with it, and the running app embeds every query with it. That is deliberate.
//! Two implementations of "the same" embedding drift on details nobody notices (a pooling
//! choice, a missing prefix, a normalisation step) and the symptom is not a crash, it is
//! search that is quietly a bit wrong forever. One implementation cannot disagree with
//! itself, and [`crate::corpus`] refuses an index whose model id is not ours.
//!
//! Two e5-specific rules, both load-bearing:
//!
//! * **The prefixes are part of the model.** e5 was trained with `query: ` on one side and
//!   `passage: ` on the other; dropping them costs real accuracy, and mixing them up costs
//!   more. [`Embedder::query`] and [`Embedder::passage`] are separate methods so a call
//!   site cannot pick the wrong one by forgetting an argument.
//! * **Pooling is a mask-weighted mean, then L2.** Not the CLS token — the model card's
//!   `1_Pooling/config.json` says `pooling_mode_mean_tokens: true`. Normalising here is
//!   what lets the index store unit vectors and score with a plain dot product.

use anyhow::{Context, Result};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use std::path::Path;

/// e5 truncates at 512; forms are short and the builder chunks to ~900 characters, so this
/// is a ceiling that protects against a pathological spreadsheet rather than a normal one.
const MAX_TOKENS: usize = 512;

/// How many passages to push through the model at once when building. Larger batches are
/// faster per item but hold more activations; 16 keeps a 791-document build inside a few
/// hundred MB.
pub const BATCH: usize = 16;

pub struct Embedder {
    model: BertModel,
    tokenizer: tokenizers::Tokenizer,
    device: Device,
}

impl Embedder {
    /// Load from a directory holding `model.safetensors`, `config.json` and
    /// `tokenizer.json` — the layout [`crate::provision`] downloads into.
    pub fn load(dir: &Path) -> Result<Embedder> {
        let weights = dir.join("model.safetensors");
        let config = dir.join("config.json");
        let tokenizer = dir.join("tokenizer.json");
        for p in [&weights, &config, &tokenizer] {
            if !p.is_file() {
                anyhow::bail!("the model file {} is missing — run `gturag sync`", p.display());
            }
        }

        let cfg: Config = serde_json::from_slice(&std::fs::read(&config)?)
            .context("model config.json will not parse")?;
        let device = Device::Cpu;
        // Memory-mapped: the weights are ~450 MB and the OS can page them, so a search
        // that touches a fraction of the model does not fault all of it in.
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[weights.clone()], DType::F32, &device)
                .with_context(|| format!("cannot read weights from {}", weights.display()))?
        };
        let model = BertModel::load(vb, &cfg).context("the weights do not fit a BERT of this shape")?;
        let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer)
            .map_err(|e| anyhow::anyhow!("cannot load tokenizer.json: {e}"))?;

        Ok(Embedder { model, tokenizer, device })
    }

    /// Embed one search query. Returns a unit vector of [`crate::corpus::DIM`] floats.
    pub fn query(&self, text: &str) -> Result<Vec<f32>> {
        Ok(self.encode(&[format!("query: {text}")])?.remove(0))
    }

    /// Embed a batch of corpus passages.
    pub fn passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let prefixed: Vec<String> = texts.iter().map(|t| format!("passage: {t}")).collect();
        self.encode(&prefixed)
    }

    /// The shared path: tokenise → pad to the batch's longest → forward → mask-weighted
    /// mean → L2 normalise.
    fn encode(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut encodings = Vec::with_capacity(texts.len());
        for t in texts {
            let e = self
                .tokenizer
                .encode(t.as_str(), true)
                .map_err(|e| anyhow::anyhow!("tokenizer failed: {e}"))?;
            let mut ids = e.get_ids().to_vec();
            ids.truncate(MAX_TOKENS);
            encodings.push(ids);
        }
        let width = encodings.iter().map(Vec::len).max().unwrap_or(1).max(1);

        let mut ids = Vec::with_capacity(texts.len() * width);
        let mut mask = Vec::with_capacity(texts.len() * width);
        for e in &encodings {
            for i in 0..width {
                // pad_token_id is 0 for this model, and the mask is what stops the padding
                // from reaching the mean — padding that is pooled is padding that changes
                // the answer depending on what else was in the batch.
                ids.push(*e.get(i).unwrap_or(&0));
                mask.push(if i < e.len() { 1u32 } else { 0u32 });
            }
        }

        let shape = (texts.len(), width);
        let input = Tensor::from_vec(ids, shape, &self.device)?;
        let attn = Tensor::from_vec(mask, shape, &self.device)?;
        let types = input.zeros_like()?;

        let hidden = self.model.forward(&input, &types, Some(&attn.to_dtype(DType::F32)?))?;
        let pooled = mean_pool(&hidden, &attn)?;
        Ok(pooled.to_vec2::<f32>()?)
    }
}

/// Mask-weighted mean over the token axis, then L2 normalise each row.
fn mean_pool(hidden: &Tensor, mask: &Tensor) -> candle_core::Result<Tensor> {
    let m = mask.unsqueeze(2)?.to_dtype(hidden.dtype())?;
    let summed = hidden.broadcast_mul(&m)?.sum(1)?;
    // clamp: an all-padding row would divide by zero and poison the whole batch with NaN.
    let counts = m.sum(1)?.clamp(1e-9, f64::INFINITY)?;
    let mean = summed.broadcast_div(&counts)?;
    let norm = mean.sqr()?.sum_keepdim(1)?.sqrt()?.clamp(1e-12, f64::INFINITY)?;
    mean.broadcast_div(&norm)
}

/// Cosine similarity of two unit vectors — a dot product, because both sides are
/// normalised at the point they are produced.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_is_a_dot_product_on_unit_vectors() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert!((cosine(&[1.0, 0.0], &[-1.0, 0.0]) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn mean_pooling_ignores_padding() {
        // Two tokens of signal, two of padding. The mean must be the mean of the SIGNAL,
        // so a batch's answer cannot depend on how long its longest neighbour was.
        let d = Device::Cpu;
        let hidden = Tensor::from_vec(
            vec![1.0f32, 0.0, 3.0, 0.0, 99.0, 99.0, 99.0, 99.0],
            (1, 4, 2),
            &d,
        )
        .unwrap();
        let mask = Tensor::from_vec(vec![1u32, 1, 0, 0], (1, 4), &d).unwrap();
        let out = mean_pool(&hidden, &mask).unwrap().to_vec2::<f32>().unwrap();
        // mean of (1,0) and (3,0) is (2,0); normalised that is (1,0).
        assert!((out[0][0] - 1.0).abs() < 1e-5, "{:?}", out[0]);
        assert!(out[0][1].abs() < 1e-5, "{:?}", out[0]);
    }

    #[test]
    fn pooling_a_fully_padded_row_is_not_nan() {
        let d = Device::Cpu;
        let hidden = Tensor::from_vec(vec![0.0f32; 4], (1, 2, 2), &d).unwrap();
        let mask = Tensor::from_vec(vec![0u32, 0], (1, 2), &d).unwrap();
        let out = mean_pool(&hidden, &mask).unwrap().to_vec2::<f32>().unwrap();
        assert!(out[0].iter().all(|v| v.is_finite()), "{:?}", out[0]);
    }
}
