//! End-to-end transcription tests for the SpeechAnalyzer helper backend.
//!
//! These run the real helper binary (built by build.rs into ./native/speech_helper)
//! against the checked-in fixture test_audio.wav, exercising the same code path
//! the orchestrator uses: 16 kHz f32 samples -> temp WAV -> helper subprocess ->
//! JSON transcript.

use std::path::Path;

use phonecheck::greeting::greeting_similarity;
use phonecheck::speech::{transcribe_with_helper, SpeechRecognizer};

const HELPER_PATH: &str = "./native/speech_helper";
const FIXTURE: &str = "test_audio.wav";

/// Load the 16 kHz mono i16 fixture as normalized f32 samples
fn load_fixture() -> Vec<f32> {
    let mut reader = hound::WavReader::open(FIXTURE).expect("fixture test_audio.wav must exist");
    assert_eq!(reader.spec().sample_rate, 16000, "fixture must be 16 kHz");
    assert_eq!(reader.spec().channels, 1, "fixture must be mono");
    reader
        .samples::<i16>()
        .map(|s| s.expect("valid sample") as f32 / 32768.0)
        .collect()
}

#[test]
fn transcribes_test_audio_fixture() {
    let samples = load_fixture();
    let transcript = transcribe_with_helper(Path::new(HELPER_PATH), &samples)
        .expect("transcription must succeed");

    // Semantic assertion: the greeting must be recognized. Exact wording of the
    // truncated tail may vary between OS model versions, so pin only the
    // stable, meaningful prefix (verified against SpeechAnalyzer on macOS 26.5:
    // "Thank you for calling cubic machinery. If you know your parties.")
    println!("SpeechAnalyzer transcript: {transcript:?}");
    let lower = transcript.to_lowercase();
    assert!(
        lower.contains("thank you for calling cubic machinery"),
        "transcript should contain the greeting, got: {transcript:?}"
    );
}

/// Full check path on real audio: transcribe the fixture, then score the
/// transcript against the configured expected greeting. This is the
/// calibration anchor for GREETING_MATCH_THRESHOLD (default 0.75) on real
/// 8 kHz G.711-degraded audio.
#[test]
fn fixture_transcript_matches_expected_greeting() {
    let samples = load_fixture();

    // The greeting configured in .env (fixture-supported portion; the fixture
    // audio is truncated mid-word after "party's")
    let expected = "Thank you for calling Cubic Machinery. If you know your party's";
    // The shorter default from config.rs must also match
    let expected_default = "thank you for calling cubic machinery";

    let recognizer =
        SpeechRecognizer::new(HELPER_PATH, expected, 0.75).expect("helper must exist");
    let result = recognizer.check_audio(&samples).expect("check must succeed");

    println!(
        "calibration (real audio): similarity {:.3} for transcript {:?}",
        result.similarity, result.transcript
    );
    assert!(
        result.greeting_found,
        "fixture must match expected greeting, similarity {:.3}, transcript {:?}",
        result.similarity, result.transcript
    );
    assert!(result.similarity >= 0.85, "expect comfortable margin over 0.75");

    let sim_default = greeting_similarity(expected_default, &result.transcript);
    println!("calibration (real audio, default greeting): similarity {sim_default:.3}");
    assert!(sim_default >= 0.95, "default greeting is a clean fuzzy substring");
}

#[test]
fn helper_error_propagates_for_unreadable_audio() {
    // Empty sample buffer still produces a valid (silent, zero-length) WAV;
    // the helper should either return an empty transcript or a clean error,
    // never hang or panic.
    let result = transcribe_with_helper(Path::new(HELPER_PATH), &[]);
    match result {
        Ok(t) => assert!(t.is_empty(), "silence should produce empty transcript, got {t:?}"),
        Err(e) => {
            let msg = e.to_string();
            assert!(!msg.is_empty(), "error must carry a message");
        }
    }
}
