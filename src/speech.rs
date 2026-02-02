/// Speech recognition and audio matching
///
/// Uses Whisper for transcription (logging/debugging) and Wav2Vec2 embeddings
/// for semantic audio similarity matching.

use anyhow::{Context, Result};
use tracing::{debug, info, warn};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::embedding::{AudioEmbedder, DEFAULT_SIMILARITY_THRESHOLD};

/// Default path for the Wav2Vec2 ONNX model
const WAV2VEC2_MODEL_PATH: &str = "./models/wav2vec2_encoder.onnx";

/// Default path for the reference embedding cache
const REFERENCE_EMBEDDING_PATH: &str = "./models/reference_embedding.bin";

pub struct SpeechRecognizer {
    ctx: WhisperContext,
    /// Wav2Vec2 embedder for semantic audio matching
    embedder: Option<AudioEmbedder>,
    /// Pre-computed reference embedding for expected phrase audio
    reference_embedding: Option<Vec<f32>>,
    /// Similarity threshold for embedding-based matching
    similarity_threshold: f32,
}

impl SpeechRecognizer {
    pub fn new(model_path: &str) -> Result<Self> {
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

        // Try to load Wav2Vec2 embedder for semantic matching
        let embedder = Self::try_load_embedder();
        let reference_embedding = Self::try_load_reference_embedding();

        if embedder.is_some() {
            if reference_embedding.is_some() {
                info!("Wav2Vec2 embedding matching enabled with cached reference");
            } else {
                info!("Wav2Vec2 embedding matching enabled (reference will be captured on first successful check)");
            }
        } else {
            warn!("Wav2Vec2 embedder not available - phrase matching will not work!");
        }

        Ok(Self {
            ctx,
            embedder,
            reference_embedding,
            similarity_threshold: DEFAULT_SIMILARITY_THRESHOLD,
        })
    }

    /// Try to load the Wav2Vec2 embedder, returns None if unavailable
    fn try_load_embedder() -> Option<AudioEmbedder> {
        let model_path = std::path::Path::new(WAV2VEC2_MODEL_PATH);
        if !model_path.exists() {
            debug!("Wav2Vec2 model not found at {:?}", model_path);
            return None;
        }

        match AudioEmbedder::new(model_path) {
            Ok(embedder) => Some(embedder),
            Err(e) => {
                warn!("Failed to load Wav2Vec2 embedder: {}", e);
                None
            }
        }
    }

    /// Try to load cached reference embedding
    fn try_load_reference_embedding() -> Option<Vec<f32>> {
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

    /// Save reference embedding to cache
    fn save_reference_embedding(embedding: &[f32]) -> Result<()> {
        let bytes: Vec<u8> = embedding
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        std::fs::write(REFERENCE_EMBEDDING_PATH, bytes)?;
        info!("Saved reference embedding to {}", REFERENCE_EMBEDDING_PATH);
        Ok(())
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
        let transcript = self.transcribe(audio_samples)?;
        debug!("Transcribed: {}", transcript);

        // Use embedding-based matching
        let (phrase_found, similarity) = self.check_embedding_similarity(audio_samples)?;

        Ok(CheckResult {
            transcript,
            phrase_found,
            similarity,
        })
    }

    /// Transcribe audio using Whisper
    fn transcribe(&self, audio_samples: &[f32]) -> Result<String> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

        params.set_n_threads(4);
        params.set_language(Some("en"));
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_nst(true);

        let mut state = self
            .ctx
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

    /// Check audio similarity using Wav2Vec2 embeddings
    fn check_embedding_similarity(&mut self, audio_samples: &[f32]) -> Result<(bool, Option<f32>)> {
        let embedder = match &mut self.embedder {
            Some(e) => e,
            None => {
                warn!("No Wav2Vec2 embedder available for matching");
                return Ok((false, None));
            }
        };

        // Compute embedding for current audio
        let current_embedding = embedder.embed(audio_samples)?;

        // Check against reference embedding
        if let Some(ref reference) = self.reference_embedding {
            let similarity = AudioEmbedder::cosine_similarity(reference, &current_embedding);
            info!(
                "Audio embedding similarity: {:.4} (threshold: {:.2})",
                similarity, self.similarity_threshold
            );

            let phrase_found = similarity >= self.similarity_threshold;

            // If match found and this is a better reference, update it
            if phrase_found && similarity > 0.95 {
                self.reference_embedding = Some(current_embedding.clone());
                if let Err(e) = Self::save_reference_embedding(&current_embedding) {
                    warn!("Failed to update reference embedding: {}", e);
                }
            }

            Ok((phrase_found, Some(similarity)))
        } else {
            // No reference yet - save this as the reference (bootstrap)
            info!("No reference embedding found, saving current audio as reference");
            self.reference_embedding = Some(current_embedding.clone());
            if let Err(e) = Self::save_reference_embedding(&current_embedding) {
                warn!("Failed to save reference embedding: {}", e);
            }
            // Assume first capture is correct (user should verify)
            Ok((true, Some(1.0)))
        }
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
