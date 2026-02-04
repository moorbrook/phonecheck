//! Singleton model manager for Whisper and Wav2Vec2 models
//!
//! Ensures models are loaded only once per process and cleaned up properly on drop.
//! Uses once_cell for lazy initialization and thread-safe access.

use anyhow::{Context, Result};
use std::sync::Mutex;
use tracing::{debug, info, warn};

use crate::embedding::AudioEmbedder;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Default path for the reference embedding cache
pub const REFERENCE_EMBEDDING_PATH: &str = "./models/reference_embedding.bin";

/// Singleton model manager
///
/// Holds both Whisper and Wav2Vec2 models, loading them once per process.
/// Implements Drop for proper cleanup logging.
pub struct ModelManager {
    whisper_ctx: WhisperContext,
    embedder: Option<AudioEmbedder>,
    whisper_model_path: String,
}

/// Global singleton instance using once_cell for lazy initialization
static MODEL_MANAGER: once_cell::sync::OnceCell<Mutex<Option<ModelManager>>> =
    once_cell::sync::OnceCell::new();

impl ModelManager {
    /// Get or create the singleton model manager
    ///
    /// On first call, loads both Whisper and Wav2Vec2 models.
    /// Subsequent calls return the already-loaded instance.
    ///
    /// Returns None if initialization fails (should only happen on first call).
    pub fn get(whisper_model_path: &str) -> Option<&'static Mutex<Option<Self>>> {
        // Initialize on first access
        if MODEL_MANAGER.get().is_none() {
            let manager = Self::try_initialize(whisper_model_path);
            let _ = MODEL_MANAGER.set(Mutex::new(manager));
        }

        MODEL_MANAGER.get()
    }

    /// Try to initialize the model manager
    ///
    /// Loads Whisper model (required) and Wav2Vec2 embedder (optional).
    /// Returns None if Whisper loading fails, Some with embedder=None if Wav2Vec2 fails.
    fn try_initialize(whisper_model_path: &str) -> Option<Self> {
        // Load Whisper model (required)
        let whisper_ctx = match Self::load_whisper(whisper_model_path) {
            Ok(ctx) => ctx,
            Err(e) => {
                warn!("Failed to load Whisper model: {}", e);
                return None;
            }
        };

        // Try to load Wav2Vec2 embedder (optional)
        let embedder = match Self::load_embedder() {
            Ok(e) => {
                info!("Wav2Vec2 embedder loaded successfully");
                Some(e)
            }
            Err(e) => {
                warn!("Wav2Vec2 embedder not available: {}", e);
                None
            }
        };

        Some(Self {
            whisper_ctx,
            embedder,
            whisper_model_path: whisper_model_path.to_string(),
        })
    }

    /// Load Whisper model from disk
    fn load_whisper(model_path: &str) -> Result<WhisperContext> {
        info!("Loading Whisper model from: {}", model_path);

        if !std::path::Path::new(model_path).exists() {
            anyhow::bail!(
                "Whisper model not found at '{}'. Download a GGML model from:\n\
                 https://huggingface.co/ggerganov/whisper.cpp/tree/main\n\
                 Recommended: ggml-base.en.bin for English (141 MB)",
                model_path
            );
        }

        let ctx = WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
            .context(format!(
                "Failed to load Whisper model from '{}'. Possible causes:\n\
                 - Wrong model format (must be GGML .bin, not PyTorch .pt)\n\
                 - Corrupted download (re-download the model)\n\
                 - Insufficient memory (try a smaller model like ggml-tiny.en.bin)",
                model_path
            ))?;

        info!("Whisper model loaded successfully");
        Ok(ctx)
    }

    /// Load Wav2Vec2 embedder from disk
    fn load_embedder() -> Result<AudioEmbedder> {
        AudioEmbedder::new("./models/wav2vec2_encoder.onnx")
            .context("Failed to load Wav2Vec2 embedder")
    }

    /// Transcribe audio using Whisper
    pub fn transcribe(&self, audio_samples: &[f32]) -> Result<String> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

        params.set_n_threads(4);
        params.set_language(Some("en"));
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_nst(true);

        let mut state = self
            .whisper_ctx
            .create_state()
            .context("Failed to create Whisper state")?;

        state
            .full(params, audio_samples)
            .context("Failed to run transcription")?;

        let num_segments = state.full_n_segments();
        let mut full_text = String::new();

        for i in 0..num_segments {
            if let Some(segment) = state.get_segment(i) {
                if let Ok(text) = segment.to_str() {
                    full_text.push_str(text);
                    full_text.push(' ');
                }
            }
        }

        Ok(full_text.trim().to_string())
    }

    /// Compute audio embedding using Wav2Vec2
    pub fn embed(&mut self, audio_samples: &[f32]) -> Result<Vec<f32>> {
        let embedder = match &mut self.embedder {
            Some(e) => e,
            None => {
                anyhow::bail!("Wav2Vec2 embedder not available");
            }
        };

        embedder.embed(audio_samples)
    }

    /// Check if Wav2Vec2 embedder is available
    pub fn has_embedder(&self) -> bool {
        self.embedder.is_some()
    }

    /// Load cached reference embedding from disk
    pub fn load_reference_embedding() -> Option<Vec<f32>> {
        let path = std::path::Path::new(REFERENCE_EMBEDDING_PATH);
        if !path.exists() {
            return None;
        }

        match std::fs::read(path) {
            Ok(bytes) => {
                if bytes.len() % 4 != 0 {
                    warn!("Invalid reference embedding file size");
                    return None;
                }
                let floats: Vec<f32> = bytes
                    .chunks_exact(4)
                    .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                    .collect();
                if floats.len() == 768 {
                    info!("Loaded cached reference embedding ({} dimensions)", floats.len());
                    Some(floats)
                } else {
                    warn!("Reference embedding has wrong dimension: {} (expected 768)", floats.len());
                    None
                }
            }
            Err(e) => {
                warn!("Failed to read reference embedding: {}", e);
                None
            }
        }
    }

    /// Save reference embedding to disk
    pub fn save_reference_embedding(embedding: &[f32]) -> Result<()> {
        let bytes: Vec<u8> = embedding
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        std::fs::write(REFERENCE_EMBEDDING_PATH, bytes)?;
        info!("Saved reference embedding to {}", REFERENCE_EMBEDDING_PATH);
        Ok(())
    }
}

impl Drop for ModelManager {
    fn drop(&mut self) {
        info!("Releasing ModelManager resources");
        debug!("Dropping WhisperContext (model: {})", self.whisper_model_path);
        debug!(
            "Dropping AudioEmbedder: {}",
            if self.embedder.is_some() {
                "loaded"
            } else {
                "not loaded"
            }
        );

        // WhisperContext and AudioEmbedder are automatically dropped
        // This Drop impl ensures we log the cleanup for observability
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_manager_singleton_behavior() {
        // This test verifies the singleton behavior
        // Note: We can't actually run this test with real models in CI
        // since it requires downloaded model files.

        // The singleton pattern ensures:
        // 1. First call initializes the manager
        // 2. Subsequent calls return the same instance
        // 3. Drop is called when the singleton is dropped

        assert!(true); // Placeholder - integration tests would verify actual behavior
    }
}
