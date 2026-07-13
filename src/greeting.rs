//! Transcript-based greeting matching
//!
//! The health check compares the SpeechAnalyzer transcript of the captured
//! call audio against the expected greeting text (`EXPECTED_GREETING`) using
//! a normalized token-level similarity score in `[0.0, 1.0]`.
//!
//! Why token-level Levenshtein (not character-level, not exact substring):
//! the audio is 8 kHz G.711 telephone audio, so ASR output jitters — misheard
//! words ("parties" for "party's"), dropped words, punctuation and casing
//! differences. Both texts are normalized (lowercase, punctuation stripped,
//! whitespace collapsed) and compared as word sequences, so one misheard word
//! costs exactly one edit regardless of its length, keeping the score
//! proportional to "how many words differ".
//!
//! Partial-capture semantics: the call records for `LISTEN_DURATION_SECS`,
//! which may cover only part of the greeting — and conversely the greeting
//! may continue past the configured expected text. Two alignments are scored
//! and the better one wins:
//!
//! 1. **Expected-inside-transcript** (fuzzy substring): the full expected
//!    token sequence found anywhere in the transcript. Handles captures
//!    longer than the expected text (greeting continues, leading noise words).
//! 2. **Transcript-as-prefix** (truncated capture): the transcript matches a
//!    leading prefix of the expected text. Only prefixes covering at least
//!    half of the expected tokens are considered, so hearing just "thank you"
//!    never passes for a twelve-word greeting. A capture covering less than
//!    roughly half the expected text therefore fails — set
//!    `EXPECTED_GREETING` to what is typically heard within the listen window.

/// Normalize text into comparable tokens: lowercase, strip all
/// non-alphanumeric characters (punctuation, apostrophes), drop empty tokens.
pub fn normalize_tokens(text: &str) -> Vec<String> {
    text.split_whitespace()
        .filter_map(|word| {
            let token: String = word
                .chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(|c| c.to_lowercase())
                .collect();
            (!token.is_empty()).then_some(token)
        })
        .collect()
}

/// Similarity between the expected greeting and a transcript, in `[0.0, 1.0]`.
///
/// Returns 0.0 if either side normalizes to no tokens (e.g. empty or
/// punctuation-only transcript).
pub fn greeting_similarity(expected: &str, transcript: &str) -> f32 {
    let e = normalize_tokens(expected);
    let t = normalize_tokens(transcript);
    if e.is_empty() || t.is_empty() {
        return 0.0;
    }
    let (m, n) = (e.len(), t.len());

    // Alignment 1: fuzzy substring — edit distance of the full expected
    // sequence against the best-matching window of the transcript. Skipping
    // transcript tokens before/after the window is free (d[0][j] = 0; take
    // the min over the last row), so extra captured speech is not penalized.
    let mut d = vec![vec![0usize; n + 1]; m + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for i in 1..=m {
        for j in 1..=n {
            let sub = usize::from(e[i - 1] != t[j - 1]);
            d[i][j] = (d[i - 1][j - 1] + sub) // substitute / match
                .min(d[i - 1][j] + 1) // drop an expected token
                .min(d[i][j - 1] + 1); // extra transcript token
        }
    }
    let substring_dist = d[m].iter().copied().min().unwrap_or(m);
    let substring_score = 1.0 - substring_dist as f32 / m as f32;

    // Alignment 2: truncated capture — the transcript against each prefix of
    // the expected text (standard Levenshtein init: d[0][j] = j), taking the
    // best prefix that still covers at least half the expected tokens.
    let mut p = vec![vec![0usize; n + 1]; m + 1];
    for (i, row) in p.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in p[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=m {
        for j in 1..=n {
            let sub = usize::from(e[i - 1] != t[j - 1]);
            p[i][j] = (p[i - 1][j - 1] + sub)
                .min(p[i - 1][j] + 1)
                .min(p[i][j - 1] + 1);
        }
    }
    let min_prefix = m.div_ceil(2);
    let prefix_score = (min_prefix..=m)
        .map(|i| 1.0 - p[i][n] as f32 / i.max(n) as f32)
        .fold(0.0f32, f32::max);

    substring_score.max(prefix_score).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPECTED: &str = "Thank you for calling Cubic Machinery. If you know your party's";

    #[test]
    fn normalize_strips_punctuation_and_case() {
        assert_eq!(
            normalize_tokens("Thank you, for CALLING... (Cubic) Machinery!"),
            vec!["thank", "you", "for", "calling", "cubic", "machinery"]
        );
        assert_eq!(normalize_tokens("party's"), vec!["partys"]);
        assert_eq!(normalize_tokens("  ...  !!  "), Vec::<String>::new());
    }

    #[test]
    fn exact_match_scores_one() {
        assert_eq!(greeting_similarity(EXPECTED, EXPECTED), 1.0);
    }

    #[test]
    fn case_and_punctuation_insensitive() {
        let sim = greeting_similarity(
            EXPECTED,
            "thank you for calling cubic machinery if you know your partys",
        );
        assert_eq!(sim, 1.0);
    }

    #[test]
    fn typo_tolerance_one_misheard_word() {
        // "parties" for "party's" — the actual SpeechAnalyzer jitter on the fixture
        let sim = greeting_similarity(
            EXPECTED,
            "Thank you for calling cubic machinery. If you know your parties.",
        );
        assert!(sim > 0.9, "one misheard word of 11 should score ~0.91, got {sim}");
    }

    #[test]
    fn word_drop_tolerated() {
        let sim = greeting_similarity(
            EXPECTED,
            "Thank you for calling machinery if you know your party's",
        );
        assert!(sim >= 0.75, "one dropped word should still pass, got {sim}");
    }

    #[test]
    fn truncated_capture_over_half_passes() {
        // 7 of 11 expected tokens captured cleanly
        let sim = greeting_similarity(EXPECTED, "Thank you for calling cubic machinery if");
        assert!(sim >= 0.9, "clean truncated prefix should pass, got {sim}");
    }

    #[test]
    fn tiny_capture_fails() {
        // 2 of 11 tokens is not evidence the greeting is playing
        let sim = greeting_similarity(EXPECTED, "Thank you");
        assert!(sim < 0.75, "two-word capture must not pass, got {sim}");
    }

    #[test]
    fn transcript_longer_than_expected_passes() {
        // Greeting continues past the configured expected text
        let sim = greeting_similarity(
            "Thank you for calling Cubic Machinery",
            "Thank you for calling cubic machinery. If you know your party's extension, dial it now.",
        );
        assert_eq!(sim, 1.0);
    }

    #[test]
    fn leading_extra_words_tolerated() {
        let sim = greeting_similarity(
            "Thank you for calling Cubic Machinery",
            "Hello. Thank you for calling cubic machinery today",
        );
        assert_eq!(sim, 1.0);
    }

    #[test]
    fn empty_transcript_scores_zero() {
        assert_eq!(greeting_similarity(EXPECTED, ""), 0.0);
        assert_eq!(greeting_similarity(EXPECTED, "  ... "), 0.0);
        assert_eq!(greeting_similarity("", "anything"), 0.0);
    }

    #[test]
    fn completely_wrong_text_fails() {
        let sim = greeting_similarity(
            EXPECTED,
            "The number you have dialed is not in service. Please check the number and try again.",
        );
        assert!(sim < 0.4, "wrong greeting must score low, got {sim}");
    }

    #[test]
    fn wrong_company_greeting_fails() {
        let sim = greeting_similarity(
            EXPECTED,
            "Thank you for calling Acme Corporation, please hold",
        );
        assert!(sim < 0.75, "different company greeting must fail, got {sim}");
    }

    /// Calibration against the fixture transcript and synthetic degradations.
    /// Run with `cargo test calibration -- --nocapture` to see the scores.
    #[test]
    fn calibration_synthetic_degradations() {
        // Real SpeechAnalyzer output for test_audio.wav (macOS 26.5)
        let fixture = "Thank you for calling cubic machinery. If you know your parties.";
        let cases: &[(&str, &str, bool)] = &[
            ("fixture transcript (real ASR output)", fixture, true),
            ("fixture minus last word", "Thank you for calling cubic machinery. If you know your", true),
            ("fixture with one word dropped mid-sentence", "Thank you for calling machinery. If you know your parties.", true),
            // 3 edits in 11 tokens (two drops + "parties") scores 0.727 — just
            // below threshold, so a badly degraded line does alert.
            ("fixture with two words dropped", "Thank you calling machinery. If you know your parties.", false),
            ("two misheard words", "Thanks you for calling cubic machinery. If you know your parties.", true),
            ("first half only", "Thank you for calling cubic", true),
            ("first third only", "Thank you for", false),
            ("empty", "", false),
            ("dead-air ASR noise", "you you the", false),
            ("carrier intercept", "The number you have dialed is not in service", false),
        ];
        for (name, transcript, should_pass) in cases {
            let sim = greeting_similarity(EXPECTED, transcript);
            println!("calibration: {sim:.3}  (pass={}) {name}", sim >= 0.75);
            assert_eq!(
                sim >= 0.75,
                *should_pass,
                "{name}: score {sim:.3} vs threshold 0.75"
            );
        }
    }
}
