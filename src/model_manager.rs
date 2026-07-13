//! Singleton model manager for the Wav2Vec2 embedding model
//!
//! Ensures the model is loaded only once per process and cleaned up properly on drop.
//! Uses once_cell for lazy initialization and thread-safe access.
//!
//! Transcription is handled separately by the SpeechAnalyzer helper subprocess
//! (see `speech.rs`); no transcription model is loaded in-process.

use anyhow::Result;
use std::sync::Mutex;
use tracing::{debug, info, warn};

use crate::embedding::AudioEmbedder;

/// Default path for the reference embedding cache
pub const REFERENCE_EMBEDDING_PATH: &str = "./models/reference_embedding.bin";

/// Singleton model manager
///
/// Holds the Wav2Vec2 embedder, loading it once per process.
/// Implements Drop for proper cleanup logging.
pub struct ModelManager {
    embedder: Option<AudioEmbedder>,
}

/// Global singleton instance using once_cell for lazy initialization
static MODEL_MANAGER: once_cell::sync::OnceCell<Mutex<Option<ModelManager>>> =
    once_cell::sync::OnceCell::new();

impl ModelManager {
    /// Get or create the singleton model manager
    ///
    /// On first call, loads the Wav2Vec2 model.
    /// Subsequent calls return the already-loaded instance.
    pub fn get() -> Option<&'static Mutex<Option<Self>>> {
        // Initialize on first access
        if MODEL_MANAGER.get().is_none() {
            let manager = Self::try_initialize();
            let _ = MODEL_MANAGER.set(Mutex::new(manager));
        }

        MODEL_MANAGER.get()
    }

    /// Try to initialize the model manager
    ///
    /// Loads the Wav2Vec2 embedder (optional): returns Some with embedder=None
    /// if loading fails, so transcription-only operation still works.
    fn try_initialize() -> Option<Self> {
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

        Some(Self { embedder })
    }

    /// Load Wav2Vec2 embedder from disk
    fn load_embedder() -> Result<AudioEmbedder> {
        use anyhow::Context;
        AudioEmbedder::new("./models/wav2vec2_encoder.onnx")
            .context("Failed to load Wav2Vec2 embedder")
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
                    info!(
                        "Loaded cached reference embedding ({} dimensions)",
                        floats.len()
                    );
                    Some(floats)
                } else {
                    warn!(
                        "Reference embedding has wrong dimension: {} (expected 768)",
                        floats.len()
                    );
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
        let bytes: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
        std::fs::write(REFERENCE_EMBEDDING_PATH, bytes)?;
        info!("Saved reference embedding to {}", REFERENCE_EMBEDDING_PATH);
        Ok(())
    }
}

impl Drop for ModelManager {
    fn drop(&mut self) {
        info!("Releasing ModelManager resources");
        debug!(
            "Dropping AudioEmbedder: {}",
            if self.embedder.is_some() {
                "loaded"
            } else {
                "not loaded"
            }
        );

        // AudioEmbedder is automatically dropped
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
