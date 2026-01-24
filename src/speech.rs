/// Speech recognition using whisper-rs (whisper.cpp bindings)
/// https://github.com/tazz4843/whisper-rs

use anyhow::{Context, Result};
use tracing::{debug, info};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub struct SpeechRecognizer {
    ctx: WhisperContext,
    expected_phrase: String,
}

impl SpeechRecognizer {
    pub fn new(model_path: &str, expected_phrase: String) -> Result<Self> {
        info!("Loading Whisper model from: {}", model_path);

        let ctx = WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
            .context("Failed to load Whisper model")?;

        info!("Whisper model loaded successfully");

        Ok(Self {
            ctx,
            expected_phrase: expected_phrase.to_lowercase(),
        })
    }

    /// Transcribe audio and check if expected phrase is present
    /// Audio should be 16kHz mono f32 samples
    pub fn check_audio(&self, audio_samples: &[f32]) -> Result<CheckResult> {
        if audio_samples.is_empty() {
            return Ok(CheckResult {
                transcript: String::new(),
                phrase_found: false,
            });
        }

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

        // Optimize for speed
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

        let transcript = full_text.trim().to_lowercase();
        debug!("Transcribed: {}", transcript);

        // Fuzzy match: check if expected phrase appears in transcript
        let phrase_found = self.fuzzy_match(&transcript);

        Ok(CheckResult {
            transcript: full_text.trim().to_string(),
            phrase_found,
        })
    }

    fn fuzzy_match(&self, transcript: &str) -> bool {
        fuzzy_match_phrase(&self.expected_phrase, transcript)
    }
}

/// Fuzzy match expected phrase against transcript (public for testing)
/// Returns true if all words from expected phrase appear in transcript in order
pub fn fuzzy_match_phrase(expected_phrase: &str, transcript: &str) -> bool {
    let expected_words: Vec<&str> = expected_phrase.split_whitespace().collect();
    let transcript_words: Vec<&str> = transcript.split_whitespace().collect();

    if expected_words.is_empty() {
        return true;
    }

    let mut expected_idx = 0;
    for word in &transcript_words {
        // Allow for minor transcription errors by checking similarity
        if words_similar(word, expected_words[expected_idx]) {
            expected_idx += 1;
            if expected_idx >= expected_words.len() {
                return true;
            }
        }
    }

    // Also try direct substring match as fallback
    transcript.contains(expected_phrase)
}

/// Check if two words are similar (public for testing)
pub fn words_similar(a: &str, b: &str) -> bool {
    // Exact match
    if a == b {
        return true;
    }

    // Allow for common transcription variations
    let a_clean: String = a.chars().filter(|c| c.is_alphanumeric()).collect();
    let b_clean: String = b.chars().filter(|c| c.is_alphanumeric()).collect();

    if a_clean == b_clean {
        return true;
    }

    // For short words (3 chars or less), require exact match (already checked above)
    if a_clean.len() <= 3 || b_clean.len() <= 3 {
        return false;
    }

    // Allow 1 character difference for longer words
    if a_clean.len().abs_diff(b_clean.len()) <= 1 {
        let distance = levenshtein(&a_clean, &b_clean);
        return distance <= 1;
    }

    false
}

/// Compute Levenshtein edit distance between two strings (public for testing)
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();

    let mut matrix = vec![vec![0usize; b_chars.len() + 1]; a_chars.len() + 1];

    for (i, row) in matrix.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=b_chars.len() {
        matrix[0][j] = j;
    }

    for i in 1..=a_chars.len() {
        for j in 1..=b_chars.len() {
            let cost = if a_chars[i - 1] == b_chars[j - 1] { 0 } else { 1 };
            matrix[i][j] = (matrix[i - 1][j] + 1)
                .min(matrix[i][j - 1] + 1)
                .min(matrix[i - 1][j - 1] + cost);
        }
    }

    matrix[a_chars.len()][b_chars.len()]
}

#[derive(Debug)]
pub struct CheckResult {
    pub transcript: String,
    pub phrase_found: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    // === Levenshtein distance tests ===

    #[test]
    fn test_levenshtein_identical() {
        assert_eq!(levenshtein("hello", "hello"), 0);
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("a", "a"), 0);
    }

    #[test]
    fn test_levenshtein_insertions() {
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("ac", "abc"), 1);
        assert_eq!(levenshtein("abc", "abcd"), 1);
    }

    #[test]
    fn test_levenshtein_deletions() {
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("abc", "ac"), 1);
        assert_eq!(levenshtein("abcd", "abc"), 1);
    }

    #[test]
    fn test_levenshtein_substitutions() {
        assert_eq!(levenshtein("abc", "axc"), 1);
        assert_eq!(levenshtein("abc", "xyz"), 3);
    }

    #[test]
    fn test_levenshtein_mixed() {
        assert_eq!(levenshtein("hello", "helo"), 1);   // deletion
        assert_eq!(levenshtein("hello", "world"), 4); // mixed
        assert_eq!(levenshtein("kitten", "sitting"), 3);
    }

    #[test]
    fn test_levenshtein_unicode() {
        assert_eq!(levenshtein("café", "cafe"), 1);
        assert_eq!(levenshtein("世界", "世界"), 0);
        assert_eq!(levenshtein("hello", "héllo"), 1);
    }

    // === words_similar tests ===

    #[test]
    fn test_words_similar_exact_match() {
        assert!(words_similar("hello", "hello"));
        assert!(words_similar("HELLO", "HELLO"));
        assert!(words_similar("", ""));
    }

    #[test]
    fn test_words_similar_punctuation_ignored() {
        assert!(words_similar("cubic", "cubic!"));
        assert!(words_similar("hello.", "hello"));
        assert!(words_similar("world!", "world?"));
        assert!(words_similar("it's", "its"));
    }

    #[test]
    fn test_words_similar_one_char_diff() {
        assert!(words_similar("hello", "helo"));     // deletion
        assert!(words_similar("hello", "helloo"));   // insertion
        assert!(words_similar("hello", "hallo"));    // substitution
        assert!(words_similar("machinery", "machinary")); // common typo
    }

    #[test]
    fn test_words_similar_two_char_diff_fails() {
        assert!(!words_similar("hello", "heo"));     // 2 deletions
        assert!(!words_similar("hello", "hellooo")); // 2 insertions
        // Note: "hello" → "yello" is only 1 substitution, so it passes
        assert!(words_similar("hello", "yello"));
    }

    #[test]
    fn test_words_similar_short_words_exact_only() {
        // Short words (3 chars or less) require exact match after punctuation removal
        assert!(words_similar("the", "the"));
        assert!(!words_similar("the", "teh"));  // typo not allowed for short words
        assert!(words_similar("a", "a"));
        assert!(!words_similar("a", "an"));
    }

    #[test]
    fn test_words_similar_different_words() {
        assert!(!words_similar("hello", "world"));
        assert!(!words_similar("cubic", "machine"));
        // Note: "thank" → "thanks" is only 1 char diff (insertion), so it passes
        assert!(words_similar("thank", "thanks"));
    }

    // === fuzzy_match_phrase tests ===

    #[test]
    fn test_fuzzy_match_exact() {
        assert!(fuzzy_match_phrase("thank you for calling", "thank you for calling"));
    }

    #[test]
    fn test_fuzzy_match_with_extra_words() {
        assert!(fuzzy_match_phrase(
            "thank you for calling",
            "hello thank you for calling cubic machinery"
        ));
    }

    #[test]
    fn test_fuzzy_match_with_typos() {
        assert!(fuzzy_match_phrase(
            "thank you for calling cubic machinery",
            "thank you for calling cubik machinery"  // typo in "cubic"
        ));
    }

    #[test]
    fn test_fuzzy_match_empty_expected() {
        assert!(fuzzy_match_phrase("", "anything goes here"));
    }

    #[test]
    fn test_fuzzy_match_empty_transcript() {
        assert!(!fuzzy_match_phrase("expected phrase", ""));
    }

    #[test]
    fn test_fuzzy_match_missing_word() {
        // Missing "for" - should fail since words must appear in order
        assert!(!fuzzy_match_phrase(
            "thank you for calling",
            "thank you calling"
        ));
    }

    #[test]
    fn test_fuzzy_match_wrong_order() {
        // Words out of order
        assert!(!fuzzy_match_phrase(
            "thank you for calling",
            "calling for you thank"
        ));
    }

    #[test]
    fn test_fuzzy_match_partial() {
        // Only first two words match
        assert!(!fuzzy_match_phrase(
            "thank you for calling cubic",
            "thank you goodbye"
        ));
    }

    #[test]
    fn test_fuzzy_match_substring_fallback() {
        // Direct substring match works as fallback
        assert!(fuzzy_match_phrase(
            "cubic machinery",
            "this is cubic machinery speaking"
        ));
    }

    #[test]
    fn test_fuzzy_match_case_sensitivity() {
        // The function expects lowercase input (as per check_audio behavior)
        assert!(fuzzy_match_phrase(
            "thank you",
            "thank you for calling"
        ));
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Levenshtein distance is symmetric: edit(a,b) == edit(b,a)
        #[test]
        fn levenshtein_symmetric(a in "[a-z]{0,20}", b in "[a-z]{0,20}") {
            prop_assert_eq!(levenshtein(&a, &b), levenshtein(&b, &a));
        }

        /// Levenshtein distance with itself is always 0
        #[test]
        fn levenshtein_identity(s in "[a-z]{0,30}") {
            prop_assert_eq!(levenshtein(&s, &s), 0);
        }

        /// Levenshtein distance is at most the length of the longer string
        #[test]
        fn levenshtein_bounded(a in "[a-z]{0,20}", b in "[a-z]{0,20}") {
            let dist = levenshtein(&a, &b);
            let max_len = a.len().max(b.len());
            prop_assert!(dist <= max_len);
        }

        /// Levenshtein satisfies triangle inequality: d(a,c) <= d(a,b) + d(b,c)
        #[test]
        fn levenshtein_triangle_inequality(
            a in "[a-z]{0,10}",
            b in "[a-z]{0,10}",
            c in "[a-z]{0,10}"
        ) {
            let d_ac = levenshtein(&a, &c);
            let d_ab = levenshtein(&a, &b);
            let d_bc = levenshtein(&b, &c);
            prop_assert!(d_ac <= d_ab + d_bc);
        }

        /// words_similar is reflexive: word is always similar to itself
        #[test]
        fn words_similar_reflexive(word in "[a-z]{1,20}") {
            prop_assert!(words_similar(&word, &word));
        }

        /// words_similar is symmetric
        #[test]
        fn words_similar_symmetric(a in "[a-z]{1,20}", b in "[a-z]{1,20}") {
            prop_assert_eq!(words_similar(&a, &b), words_similar(&b, &a));
        }

        /// fuzzy_match_phrase with identical strings always returns true
        #[test]
        fn fuzzy_match_identical(phrase in "[a-z ]{1,50}") {
            let trimmed = phrase.trim();
            if !trimmed.is_empty() {
                prop_assert!(fuzzy_match_phrase(trimmed, trimmed));
            }
        }

        /// Empty expected phrase always matches
        #[test]
        fn fuzzy_match_empty_expected_always_true(transcript in "[a-z ]{0,50}") {
            prop_assert!(fuzzy_match_phrase("", &transcript));
        }

        /// levenshtein never panics on any UTF-8 input
        #[test]
        fn levenshtein_never_panics(a in ".*", b in ".*") {
            let _ = levenshtein(&a, &b);
        }

        /// words_similar never panics on any UTF-8 input
        #[test]
        fn words_similar_never_panics(a in ".*", b in ".*") {
            let _ = words_similar(&a, &b);
        }
    }
}

/// Kani formal verification proofs
#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    #[kani::unwind(10)]
    fn levenshtein_never_panics_small() {
        // Test with small strings to keep verification tractable
        let a: [u8; 4] = kani::any();
        let b: [u8; 4] = kani::any();

        if let (Ok(s1), Ok(s2)) = (std::str::from_utf8(&a), std::str::from_utf8(&b)) {
            let _ = levenshtein(s1, s2);
        }
    }

    #[kani::proof]
    fn words_similar_never_panics() {
        let a: [u8; 8] = kani::any();
        let b: [u8; 8] = kani::any();

        if let (Ok(s1), Ok(s2)) = (std::str::from_utf8(&a), std::str::from_utf8(&b)) {
            let _ = words_similar(s1, s2);
        }
    }

    #[kani::proof]
    fn levenshtein_identity() {
        let s: [u8; 4] = kani::any();
        if let Ok(str_s) = std::str::from_utf8(&s) {
            kani::assert(levenshtein(str_s, str_s) == 0, "distance to self must be 0");
        }
    }
}
