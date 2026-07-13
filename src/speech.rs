/// Speech recognition and audio matching
///
/// Uses Apple's SpeechAnalyzer/SpeechTranscriber (macOS 26 Speech framework)
/// for transcription (logging/debugging) via a small helper subprocess
/// (`native/speech_helper`), and Wav2Vec2 embeddings for semantic audio
/// similarity matching.
///
/// The Wav2Vec2 model is loaded via the singleton ModelManager to ensure it is
/// only loaded once per process and properly cleaned up. Transcription happens
/// out-of-process: the helper takes a WAV path and prints a JSON transcript,
/// keeping the Rust binary free of Swift FFI linkage.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use crate::embedding::{AudioEmbedder, DEFAULT_SIMILARITY_THRESHOLD};
use crate::model_manager::{ModelManager, REFERENCE_EMBEDDING_PATH};

/// Default similarity threshold for embedding-based matching
const SIMILARITY_THRESHOLD: f32 = DEFAULT_SIMILARITY_THRESHOLD;

/// Sample rate of the audio handed to the transcription helper (Hz)
const HELPER_SAMPLE_RATE: u32 = crate::rtp::TARGET_SAMPLE_RATE;

/// Maximum time the transcription helper may run before being killed
const HELPER_TIMEOUT: Duration = Duration::from_secs(60);

/// Poll interval while waiting for the helper to exit
const HELPER_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Type alias for the singleton mutex type
type ModelManagerMutex = &'static std::sync::Mutex<Option<ModelManager>>;

/// Temp file that removes itself on drop (even on error paths)
struct TempWav(PathBuf);

impl Drop for TempWav {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Write 16 kHz mono f32 samples to a temporary WAV file (i16 PCM)
fn write_temp_wav(audio_samples: &[f32]) -> Result<TempWav> {
    let path = std::env::temp_dir().join(format!(
        "phonecheck_transcribe_{}_{:x}.wav",
        std::process::id(),
        rand::random::<u64>()
    ));

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: HELPER_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let temp = TempWav(path.clone());
    let mut writer = hound::WavWriter::create(&path, spec).context("Failed to create temp WAV")?;
    for &sample in audio_samples {
        let clamped = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
        writer.write_sample(clamped)?;
    }
    writer.finalize().context("Failed to finalize temp WAV")?;
    Ok(temp)
}

/// Transcribe 16 kHz mono f32 samples by invoking the SpeechAnalyzer helper.
///
/// The helper is a small Swift binary (built by `scripts/build_speech_helper.sh`)
/// that reads a WAV file and prints `{"transcript": "..."}` on stdout. Errors
/// (missing OS speech assets, unsupported OS, bad audio) arrive on stderr with
/// a non-zero exit code and are propagated here.
pub fn transcribe_with_helper(helper_path: &Path, audio_samples: &[f32]) -> Result<String> {
    if !helper_path.exists() {
        anyhow::bail!(
            "Speech helper not found at '{}'. Build it with: sh scripts/build_speech_helper.sh \
             (requires macOS 26+ and the Xcode command line tools)",
            helper_path.display()
        );
    }

    let temp_wav = write_temp_wav(audio_samples)?;

    let mut child = Command::new(helper_path)
        .arg(&temp_wav.0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to spawn speech helper '{}'", helper_path.display()))?;

    // Bounded wait: poll with a hard iteration cap, then kill on timeout.
    let max_polls = (HELPER_TIMEOUT.as_millis() / HELPER_POLL_INTERVAL.as_millis()).max(1);
    let mut status = None;
    for _ in 0..max_polls {
        if let Some(s) = child.try_wait().context("Failed to wait on speech helper")? {
            status = Some(s);
            break;
        }
        std::thread::sleep(HELPER_POLL_INTERVAL);
    }

    let status = match status {
        Some(s) => s,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!(
                "Speech helper timed out after {}s and was killed",
                HELPER_TIMEOUT.as_secs()
            );
        }
    };

    // The helper's output is tiny (one JSON line), so reading after exit is safe.
    let mut stdout = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut stdout);
    }
    let mut stderr = String::new();
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr);
    }

    if !status.success() {
        anyhow::bail!(
            "Speech helper failed (exit: {}): {}",
            status.code().map_or("signal".into(), |c| c.to_string()),
            stderr.trim()
        );
    }

    if !stderr.trim().is_empty() {
        info!("Speech helper: {}", stderr.trim());
    }

    parse_helper_output(&stdout)
}

/// Parse the helper's JSON output (`{"transcript": "..."}`)
fn parse_helper_output(stdout: &str) -> Result<String> {
    let value: serde_json::Value = serde_json::from_str(stdout.trim())
        .with_context(|| format!("Speech helper produced invalid JSON: {:?}", stdout.trim()))?;
    let transcript = value
        .get("transcript")
        .and_then(|t| t.as_str())
        .context("Speech helper JSON missing 'transcript' field")?;
    Ok(transcript.trim().to_string())
}

pub struct SpeechRecognizer {
    /// Path to the SpeechAnalyzer helper binary
    helper_path: PathBuf,
    /// Pre-computed reference embedding for expected phrase audio
    reference_embedding: Option<Vec<f32>>,
}

impl SpeechRecognizer {
    pub fn new(helper_path: &str) -> Result<Self> {
        info!("Initializing SpeechRecognizer (SpeechAnalyzer helper + Wav2Vec2 singleton)");

        let helper_path = PathBuf::from(helper_path);
        if !helper_path.exists() {
            anyhow::bail!(
                "Speech helper not found at '{}'. Build it with: sh scripts/build_speech_helper.sh \
                 (requires macOS 26+ and the Xcode command line tools)",
                helper_path.display()
            );
        }

        // Initialize singleton model manager (loads Wav2Vec2 on first call)
        if ModelManager::get().is_none() {
            anyhow::bail!("Failed to initialize ModelManager - check model files");
        }

        // Load cached reference embedding if available
        let reference_embedding = Self::load_cached_reference();

        if reference_embedding.is_some() {
            info!("Using cached reference embedding for phrase matching");
        }

        Ok(Self {
            helper_path,
            reference_embedding,
        })
    }

    /// Load cached reference embedding from disk
    fn load_cached_reference() -> Option<Vec<f32>> {
        // Access singleton only for loading the reference (no models needed)
        ModelManager::get()?;
        ModelManager::load_reference_embedding()
    }

    /// Transcribe audio using the SpeechAnalyzer helper subprocess
    fn transcribe_audio(&self, audio_samples: &[f32]) -> Result<String> {
        transcribe_with_helper(&self.helper_path, audio_samples)
    }

    /// Check if embedder is available
    fn has_embedder(&self) -> Result<bool> {
        let guard = ModelManager::get()
            .and_then(|m: ModelManagerMutex| m.lock().ok())
            .context("Failed to access ModelManager")?;

        let model_manager = guard.as_ref().context("ModelManager not initialized")?;

        Ok(model_manager.has_embedder())
    }

    /// Compute embedding using Wav2Vec2 (mutable access)
    fn compute_embedding(&mut self, audio_samples: &[f32]) -> Result<Vec<f32>> {
        let mut guard = ModelManager::get()
            .and_then(|m: ModelManagerMutex| m.lock().ok())
            .context("Failed to access ModelManager for embedding")?;

        let model_manager = guard.as_mut().context("ModelManager not initialized")?;

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

        // First, transcribe with SpeechAnalyzer for logging/debugging
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
    fn check_embedding_similarity(&mut self, audio_samples: &[f32]) -> Result<(bool, Option<f32>)> {
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
        info!(
            "Reloaded reference embedding from {}",
            REFERENCE_EMBEDDING_PATH
        );
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

    #[test]
    fn test_helper_missing_is_clear_error() {
        let err = transcribe_with_helper(Path::new("/nonexistent/speech_helper"), &[0.0; 160])
            .expect_err("missing helper must error");
        let msg = err.to_string();
        assert!(msg.contains("Speech helper not found"), "got: {msg}");
        assert!(msg.contains("build_speech_helper.sh"), "got: {msg}");
    }

    #[test]
    fn test_recognizer_new_missing_helper_is_clear_error() {
        let err = match SpeechRecognizer::new("/nonexistent/speech_helper") {
            Ok(_) => panic!("missing helper must error"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("Speech helper not found"));
    }

    #[test]
    fn test_parse_helper_output_valid() {
        let t = parse_helper_output("{\"transcript\": \"hello world\"}\n").unwrap();
        assert_eq!(t, "hello world");
    }

    #[test]
    fn test_parse_helper_output_invalid_json() {
        assert!(parse_helper_output("not json").is_err());
        assert!(parse_helper_output("{\"other\": 1}").is_err());
    }
}
