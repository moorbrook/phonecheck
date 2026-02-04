/// Speech recognition and audio matching
///
/// Uses Whisper for transcription (logging/debugging) and Wav2Vec2 embeddings
/// for semantic audio similarity matching.
///
/// Both Whisper and Wav2Vec2 models are loaded via the singleton ModelManager
/// to ensure they are only loaded once per process and properly cleaned up.

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use crate::embedding::{AudioEmbedder, DEFAULT_SIMILARITY_THRESHOLD};
use crate::model_manager::{ModelManager, REFERENCE_EMBEDDING_PATH};

/// Default similarity threshold for embedding-based matching
const SIMILARITY_THRESHOLD: f32 = DEFAULT_SIMILARITY_THRESHOLD;

/// Type alias for the singleton mutex type
type ModelManagerMutex = &'static std::sync::Mutex<Option<ModelManager>>;

pub struct SpeechRecognizer {
    /// Model path (stored for singleton access)
    model_path: String,
    /// Pre-computed reference embedding for expected phrase audio
    reference_embedding: Option<Vec<f32>>,
}

impl SpeechRecognizer {
    pub fn new(model_path: &str) -> Result<Self> {
        info!("Initializing SpeechRecognizer (using singleton models)");

        // Initialize singleton model manager
        // This loads Whisper and Wav2Vec2 models on first call
        if ModelManager::get(model_path).is_none() {
            anyhow::bail!("Failed to initialize ModelManager - check model files");
        }

        // Load cached reference embedding if available
        let reference_embedding = Self::load_cached_reference(model_path);

        if reference_embedding.is_some() {
            info!("Using cached reference embedding for phrase matching");
        }

        Ok(Self {
            model_path: model_path.to_string(),
            reference_embedding,
        })
    }

    /// Load cached reference embedding from disk
    fn load_cached_reference(model_path: &str) -> Option<Vec<f32>> {
        // Access singleton only for loading the reference (no models needed)
        ModelManager::get(model_path)?;
        ModelManager::load_reference_embedding()
    }

    /// Transcribe audio using Whisper (immutable access)
    fn transcribe_audio(&self, audio_samples: &[f32]) -> Result<String> {
        let guard = ModelManager::get(&self.model_path)
            .and_then(|m: ModelManagerMutex| m.lock().ok())
            .context("Failed to access ModelManager")?;

        let model_manager = guard
            .as_ref()
            .context("ModelManager not initialized")?;

        model_manager.transcribe(audio_samples)
    }

    /// Check if embedder is available
    fn has_embedder(&self) -> Result<bool> {
        let guard = ModelManager::get(&self.model_path)
            .and_then(|m: ModelManagerMutex| m.lock().ok())
            .context("Failed to access ModelManager")?;

        let model_manager = guard
            .as_ref()
            .context("ModelManager not initialized")?;

        Ok(model_manager.has_embedder())
    }

    /// Compute embedding using Wav2Vec2 (mutable access)
    fn compute_embedding(&mut self, audio_samples: &[f32]) -> Result<Vec<f32>> {
        let mut guard = ModelManager::get(&self.model_path)
            .and_then(|m: ModelManagerMutex| m.lock().ok())
            .context("Failed to access ModelManager for embedding")?;

        let model_manager = guard
            .as_mut()
            .context("ModelManager not initialized")?;

        model_manager.embed(audio_samples)
    }

    /// Transcribe audio and check if expected phrase is present using embedding similarity
    /// Audio should be 16kHz mono f32 samples
    pub fn check_audio(&mut self, audio_samples: &[f32]) -> Result<CheckResult> {
        if audio_samples.is_empty() {
            return Ok(CheckResult {
                transcript: String::new(),
                phrase_found: false,
                similarity: None,
            });
        }

        // First, transcribe with Whisper for logging/debugging
        let transcript = self.transcribe_audio(audio_samples)?;
        debug!("Transcribed: {}", transcript);

        // Check if embedder is available
        let has_embedder = self.has_embedder()?;
        if !has_embedder {
            warn!("No Wav2Vec2 embedder available - phrase matching will not work!");
            return Ok(CheckResult {
                transcript,
                phrase_found: false,
                similarity: None,
            });
        }

        // Use embedding-based matching
        let (phrase_found, similarity) = self.check_embedding_similarity(audio_samples)?;

        Ok(CheckResult {
            transcript,
            phrase_found,
            similarity,
        })
    }

    /// Check audio similarity using Wav2Vec2 embeddings
    fn check_embedding_similarity(
        &mut self,
        audio_samples: &[f32],
    ) -> Result<(bool, Option<f32>)> {
        // Compute embedding for current audio
        let current_embedding = self.compute_embedding(audio_samples)?;

        // Check against reference embedding
        if let Some(ref reference) = self.reference_embedding {
            let similarity = AudioEmbedder::cosine_similarity(reference, &current_embedding);
            info!(
                "Audio embedding similarity: {:.4} (threshold: {:.2})",
                similarity, SIMILARITY_THRESHOLD
            );

            let phrase_found = similarity >= SIMILARITY_THRESHOLD;

            // If match found and this is a better reference, update it
            if phrase_found && similarity > 0.95 {
                self.reference_embedding = Some(current_embedding.clone());
                if let Err(e) = ModelManager::save_reference_embedding(&current_embedding) {
                    warn!("Failed to update reference embedding: {}", e);
                }
            }

            Ok((phrase_found, Some(similarity)))
        } else {
            // No reference yet - save this as the reference (bootstrap)
            info!("No reference embedding found, saving current audio as reference");
            self.reference_embedding = Some(current_embedding.clone());
            if let Err(e) = ModelManager::save_reference_embedding(&current_embedding) {
                warn!("Failed to save reference embedding: {}", e);
            }
            // Assume first capture is correct (user should verify)
            Ok((true, Some(1.0)))
        }
    }

    /// Load a new reference embedding from disk
    pub fn reload_reference(&mut self) -> Result<()> {
        let new_ref = ModelManager::load_reference_embedding()
            .context("No reference embedding file found")?;

        self.reference_embedding = Some(new_ref);
        info!("Reloaded reference embedding from {}", REFERENCE_EMBEDDING_PATH);
        Ok(())
    }
}

#[derive(Debug)]
pub struct CheckResult {
    pub transcript: String,
    pub phrase_found: bool,
    pub similarity: Option<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_result_default() {
        let result = CheckResult {
            transcript: "test".to_string(),
            phrase_found: true,
            similarity: Some(0.95),
        };
        assert!(result.phrase_found);
        assert_eq!(result.similarity, Some(0.95));
    }
}
