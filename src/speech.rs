/// Speech recognition and greeting matching
///
/// Uses Apple's SpeechAnalyzer/SpeechTranscriber (macOS 26 Speech framework)
/// for transcription via a small helper subprocess (`native/speech_helper`),
/// then compares the transcript against the expected greeting text with a
/// normalized token-level similarity score (see `greeting.rs`).
///
/// Transcription happens out-of-process: the helper takes a WAV path and
/// prints a JSON transcript, keeping the Rust binary free of Swift FFI
/// linkage.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::{debug, info};

use crate::greeting::greeting_similarity;

/// Sample rate of the audio handed to the transcription helper (Hz)
const HELPER_SAMPLE_RATE: u32 = crate::rtp::TARGET_SAMPLE_RATE;

/// Maximum time the transcription helper may run before being killed
const HELPER_TIMEOUT: Duration = Duration::from_secs(60);

/// Poll interval while waiting for the helper to exit
const HELPER_POLL_INTERVAL: Duration = Duration::from_millis(100);

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
    /// Expected greeting text (EXPECTED_GREETING)
    expected_greeting: String,
    /// Similarity threshold in (0.0, 1.0] (GREETING_MATCH_THRESHOLD)
    match_threshold: f32,
}

impl SpeechRecognizer {
    pub fn new(helper_path: &str, expected_greeting: &str, match_threshold: f32) -> Result<Self> {
        info!("Initializing SpeechRecognizer (SpeechAnalyzer helper + transcript matching)");

        let helper_path = PathBuf::from(helper_path);
        if !helper_path.exists() {
            anyhow::bail!(
                "Speech helper not found at '{}'. Build it with: sh scripts/build_speech_helper.sh \
                 (requires macOS 26+ and the Xcode command line tools)",
                helper_path.display()
            );
        }

        Ok(Self {
            helper_path,
            expected_greeting: expected_greeting.to_string(),
            match_threshold,
        })
    }

    /// Transcribe audio and check whether the expected greeting was heard.
    /// Audio should be 16 kHz mono f32 samples.
    pub fn check_audio(&self, audio_samples: &[f32]) -> Result<CheckResult> {
        if audio_samples.is_empty() {
            return Ok(CheckResult {
                transcript: String::new(),
                greeting_found: false,
                similarity: 0.0,
            });
        }

        let transcript = transcribe_with_helper(&self.helper_path, audio_samples)?;
        debug!("Transcribed: {}", transcript);

        let similarity = greeting_similarity(&self.expected_greeting, &transcript);
        info!(
            "Transcript similarity: {:.3} (threshold: {:.2})",
            similarity, self.match_threshold
        );

        Ok(CheckResult {
            greeting_found: similarity >= self.match_threshold,
            transcript,
            similarity,
        })
    }
}

#[derive(Debug)]
pub struct CheckResult {
    pub transcript: String,
    pub greeting_found: bool,
    /// Normalized token-level similarity between transcript and expected greeting
    pub similarity: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPECTED: &str = "thank you for calling cubic machinery";

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
        let err = match SpeechRecognizer::new("/nonexistent/speech_helper", EXPECTED, 0.75) {
            Ok(_) => panic!("missing helper must error"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("Speech helper not found"));
    }

    #[test]
    fn test_empty_audio_is_not_found_without_helper_call() {
        // Empty capture short-circuits before invoking the helper, so a bogus
        // helper path must not matter.
        let recognizer = SpeechRecognizer {
            helper_path: PathBuf::from("/nonexistent/speech_helper"),
            expected_greeting: EXPECTED.to_string(),
            match_threshold: 0.75,
        };
        let result = recognizer.check_audio(&[]).expect("empty audio is ok");
        assert!(!result.greeting_found);
        assert_eq!(result.similarity, 0.0);
        assert!(result.transcript.is_empty());
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
