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

        // Check if model file exists before attempting to load
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

    // === Edge case tests for speech recognition artifacts ===

    #[test]
    fn test_fuzzy_match_repeated_words() {
        // Whisper sometimes stutters/repeats words
        assert!(fuzzy_match_phrase(
            "thank you for calling",
            "thank thank you for for calling"
        ));
    }

    #[test]
    fn test_fuzzy_match_filler_words() {
        // Common filler words in transcriptions
        assert!(fuzzy_match_phrase(
            "thank you for calling",
            "um thank you uh for calling"
        ));
    }

    #[test]
    #[ignore = "Known limitation: numbers as words not yet supported"]
    fn test_fuzzy_match_numbers_as_words() {
        // Phone system might read numbers
        // KNOWN LIMITATION: "1" and "one" are treated as different words
        assert!(fuzzy_match_phrase(
            "press one",
            "press 1"
        ));
    }

    #[test]
    #[ignore = "Known limitation: contractions not expanded"]
    fn test_fuzzy_match_contractions() {
        // Contractions might be transcribed differently
        // KNOWN LIMITATION: "we're" treated as single word, doesn't match "we are"
        assert!(fuzzy_match_phrase(
            "we are closed",
            "we're closed"
        ));
    }

    #[test]
    #[ignore = "Known limitation: hyphenated words not split"]
    fn test_fuzzy_match_hyphenated_words() {
        // Some company names have hyphens
        // KNOWN LIMITATION: "cubic-machinery" treated as single word
        assert!(fuzzy_match_phrase(
            "cubic machinery",
            "cubic-machinery speaking"
        ));
    }

    #[test]
    fn test_fuzzy_match_very_long_phrase() {
        // Ensure performance is reasonable with longer phrases
        let expected = "thank you for calling cubic machinery incorporated our office hours are nine to five monday through friday";
        let transcript = "hello thank you for calling cubic machinery incorporated our office hours are nine to five monday through friday please leave a message";
        assert!(fuzzy_match_phrase(expected, transcript));
    }

    #[test]
    fn test_fuzzy_match_single_word_expected() {
        // Single word phrases
        assert!(fuzzy_match_phrase("hello", "hello there"));
        assert!(fuzzy_match_phrase("hello", "well hello"));
        assert!(!fuzzy_match_phrase("goodbye", "hello there"));
    }

    #[test]
    fn test_words_similar_common_transcription_errors() {
        // Common ASR mistakes
        assert!(words_similar("machinery", "machinary")); // common misspelling
        assert!(words_similar("cubic", "cubik"));         // phonetic
        assert!(words_similar("office", "offise"));       // phonetic
        assert!(words_similar("hours", "ours"));          // homophone - may pass due to 1 edit
    }

    #[test]
    fn test_fuzzy_match_whitespace_variations() {
        // Extra whitespace shouldn't matter
        assert!(fuzzy_match_phrase(
            "thank you",
            "thank   you"  // extra space
        ));
        assert!(fuzzy_match_phrase(
            "thank you",
            "  thank you  "  // leading/trailing
        ));
    }

    #[test]
    fn test_fuzzy_match_punctuation_in_expected() {
        // Punctuation in expected phrase
        assert!(fuzzy_match_phrase(
            "hello, how are you?",
            "hello how are you"
        ));
    }

    // === Negative tests: verify dissimilar phrases do NOT match ===

    #[test]
    fn test_fuzzy_match_completely_different() {
        assert!(!fuzzy_match_phrase(
            "thank you for calling cubic machinery",
            "goodbye we are closed please call back tomorrow"
        ));
    }

    #[test]
    fn test_fuzzy_match_similar_but_different_company() {
        // Similar structure but different company name
        assert!(!fuzzy_match_phrase(
            "thank you for calling cubic machinery",
            "thank you for calling acme corporation"
        ));
    }

    #[test]
    fn test_fuzzy_match_partial_overlap_insufficient() {
        // First few words match but rest doesn't
        assert!(!fuzzy_match_phrase(
            "thank you for calling cubic machinery",
            "thank you for your order"
        ));
    }

    #[test]
    fn test_fuzzy_match_same_words_wrong_context() {
        // Contains the words but in wrong order/context
        assert!(!fuzzy_match_phrase(
            "press one for sales",
            "sales one press for"
        ));
    }

    #[test]
    fn test_fuzzy_match_empty_transcript_nonempty_expected() {
        assert!(!fuzzy_match_phrase("hello world", ""));
    }

    #[test]
    fn test_fuzzy_match_only_filler_words() {
        // Transcript is only filler words, expected is actual content
        assert!(!fuzzy_match_phrase(
            "thank you for calling",
            "um uh er ah"
        ));
    }

    #[test]
    fn test_fuzzy_match_whisper_hallucination_patterns() {
        // Whisper sometimes hallucinates repeated punctuation or music notes
        assert!(!fuzzy_match_phrase(
            "thank you for calling",
            "... ... ... ..."
        ));
        assert!(!fuzzy_match_phrase(
            "thank you for calling",
            "[Music] [Music] [Music]"
        ));
    }

    #[test]
    fn test_fuzzy_match_completely_unrelated_long() {
        assert!(!fuzzy_match_phrase(
            "welcome to our customer service line",
            "the quick brown fox jumps over the lazy dog near the riverbank"
        ));
    }

    #[test]
    fn test_fuzzy_match_numbers_dont_match_words() {
        // Numbers should not match their word equivalents (known limitation)
        // This documents current behavior
        assert!(!fuzzy_match_phrase("press one", "press 1"));
        assert!(!fuzzy_match_phrase("dial two", "dial 2"));
    }

    #[test]
    fn test_levenshtein_empty_strings() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("a", ""), 1);
        assert_eq!(levenshtein("", "a"), 1);
        assert_eq!(levenshtein("abc", ""), 3);
    }

    #[test]
    fn test_words_similar_empty() {
        assert!(words_similar("", ""));
        // Empty vs non-empty: only punctuation stripped
        assert!(!words_similar("", "hello"));
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

        /// fuzzy_match_phrase is reflexive: phrase always matches itself
        #[test]
        fn fuzzy_match_reflexive(phrase in "[a-z]{1,5}( [a-z]{1,5}){0,4}") {
            prop_assert!(fuzzy_match_phrase(&phrase, &phrase));
        }

        /// fuzzy_match_phrase: phrase matches when transcript has extra prefix/suffix
        #[test]
        fn fuzzy_match_with_extra_context(
            phrase in "[a-z]{3,6}( [a-z]{3,6}){1,3}",
            prefix in "[a-z]{0,3}( [a-z]{0,3}){0,2}",
            suffix in "( [a-z]{0,3}){0,2}"
        ) {
            let phrase = phrase.trim();
            if !phrase.is_empty() {
                let transcript = format!("{} {} {}", prefix, phrase, suffix);
                prop_assert!(fuzzy_match_phrase(phrase, &transcript),
                    "phrase '{}' should match transcript '{}'", phrase, transcript);
            }
        }

        /// Random short strings rarely match specific phrases
        #[test]
        fn random_strings_rarely_match_specific_phrase(
            random in "[a-z]{1,4}( [a-z]{1,4}){0,5}"
        ) {
            // This specific phrase should rarely match random strings
            let specific = "thank you for calling cubic machinery";
            // We can't assert it never matches (false positives possible)
            // but we log for manual inspection
            let matches = fuzzy_match_phrase(specific, &random);
            if matches {
                // This should be rare - log it
                eprintln!("WARNING: random '{}' matched '{}'", random, specific);
            }
        }

        /// Words with 2+ edit distance should not be similar (for words > 3 chars)
        #[test]
        fn distant_words_not_similar(
            base in "[a-z]{5,10}",
            changes in 2u8..4u8
        ) {
            // Apply multiple changes to make a distant word
            let mut distant = base.clone();
            for _ in 0..changes {
                if !distant.is_empty() {
                    // Remove a character
                    let pos = distant.len() / 2;
                    distant.remove(pos.min(distant.len() - 1));
                }
            }
            // Words with 2+ edits should not be similar
            if levenshtein(&base, &distant) >= 2 {
                prop_assert!(!words_similar(&base, &distant),
                    "'{}' and '{}' should not be similar", base, distant);
            }
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
