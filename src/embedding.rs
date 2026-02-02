//! Audio embedding using Wav2Vec2 via ONNX Runtime
//!
//! Provides semantic audio embeddings that capture both phonetic and semantic similarity.
//! "thanks for calling" and "thank you for calling" will have high similarity scores.

use anyhow::{Context, Result};
use ndarray::Axis;
use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;
use tracing::{debug, info};

/// Default similarity threshold for phrase matching
/// Values above this indicate the audio matches the reference
pub const DEFAULT_SIMILARITY_THRESHOLD: f32 = 0.80;

/// Audio embedding model using Wav2Vec2
pub struct AudioEmbedder {
    session: Session,
    model_path: String,
}

impl Drop for AudioEmbedder {
    fn drop(&mut self) {
        tracing::debug!("Releasing Wav2Vec2 ONNX model resources: {}", self.model_path);
        // Session is dropped automatically by ort crate
    }
}

impl AudioEmbedder {
    /// Load the Wav2Vec2 ONNX model
    pub fn new<P: AsRef<Path>>(model_path: P) -> Result<Self> {
        let path = model_path.as_ref();
        info!("Loading Wav2Vec2 model from: {:?}", path);

        if !path.exists() {
            anyhow::bail!(
                "Wav2Vec2 model not found at '{:?}'. Run:\n\
                 uv run --python 3.13 scripts/export_wav2vec2.py",
                path
            );
        }

        let session = Session::builder()?
            .with_intra_threads(4)?
            .commit_from_file(path)
            .with_context(|| format!("Failed to load ONNX model from {:?}", path))?;

        info!("Wav2Vec2 model loaded successfully");
        Ok(Self {
            session,
            model_path: path.to_string_lossy().to_string(),
        })
    }

    /// Compute audio embedding from f32 samples (16kHz mono)
    /// Returns a 768-dimensional embedding vector (mean pooled across time)
    pub fn embed(&mut self, audio: &[f32]) -> Result<Vec<f32>> {
        if audio.is_empty() {
            return Ok(vec![0.0; 768]);
        }

        // Create input tensor [1, audio_len]
        let audio_len = audio.len();
        let input_array = ndarray::Array2::from_shape_vec((1, audio_len), audio.to_vec())?;
        let input_tensor = Tensor::from_array(input_array)?;

        // Run inference
        let outputs = self.session.run(ort::inputs![input_tensor])?;

        // Get output tensor [1, time, 768]
        let output = outputs[0]
            .try_extract_array::<f32>()
            .context("Failed to extract output tensor")?;

        let shape = output.shape();
        debug!(
            "Wav2Vec2 output shape: [{}, {}, {}]",
            shape[0], shape[1], shape[2]
        );

        // Mean pool across time dimension (axis 1)
        let mean_embedding = output
            .index_axis(Axis(0), 0) // Remove batch dimension -> [time, 768]
            .mean_axis(Axis(0)) // Mean across time -> [768]
            .context("Failed to compute mean")?;

        let (mut embedding, _offset) = mean_embedding.into_raw_vec_and_offset();

        // L2 normalize
        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 1e-8 {
            for x in &mut embedding {
                *x /= norm;
            }
        }

        Ok(embedding)
    }

    /// Compute cosine similarity between two embeddings
    pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() || a.is_empty() {
            return 0.0;
        }

        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm_a > 1e-8 && norm_b > 1e-8 {
            dot / (norm_a * norm_b)
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((AudioEmbedder::cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!(AudioEmbedder::cosine_similarity(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![-1.0, 0.0, 0.0];
        assert!((AudioEmbedder::cosine_similarity(&a, &b) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_normalized() {
        let a = vec![0.6, 0.8, 0.0];
        let b = vec![0.6, 0.8, 0.0];
        assert!((AudioEmbedder::cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
    }
}
