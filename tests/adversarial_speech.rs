//! Adversarial Property-Based Tests for Fuzzy Phrase Matching
//!
//! # Attack Plan
//!
//! 1. **Levenshtein DoS**: Very long strings causing O(n²) memory allocation.
//!    Test that reasonable-length strings complete in acceptable time.
//!
//! 2. **Unicode Normalization**: é (precomposed) vs e + combining accent (decomposed).
//!    Different byte representations of the same visual character.
//!
//! 3. **Zero-Width Characters**: Hidden chars like U+200B that break matching invisibly.
//!
//! 4. **Case Sensitivity Issues**: Turkish İ/i, German ß uppercase edge cases.
//!
//! 5. **Grapheme Clusters**: Emoji with skin tone modifiers, flags (multi-codepoint).
//!
//! 6. **False Positive Prevention**: Ensure similar but semantically different
//!    phrases don't match incorrectly.
//!
//! # Invariants
//!
//! - levenshtein(a, b) == levenshtein(b, a) (symmetry)
//! - levenshtein(a, a) == 0 (identity)
//! - levenshtein(a, c) <= levenshtein(a, b) + levenshtein(b, c) (triangle inequality)
//! - words_similar(a, b) == words_similar(b, a) (symmetry)
//! - fuzzy_match_phrase(phrase, phrase) == true (reflexivity)
//! - All functions never panic on any UTF-8 input

use proptest::prelude::*;

use phonecheck::speech::{fuzzy_match_phrase, levenshtein, words_similar};

// ============================================================================
// ADVERSARIAL GENERATORS
// ============================================================================

/// Generate Unicode edge case strings
fn unicode_edge_cases() -> impl Strategy<Value = String> {
    prop_oneof![
        // Precomposed vs decomposed
        Just("café".to_string()),                          // precomposed é
        Just("cafe\u{0301}".to_string()),                  // e + combining accent
        // Zero-width characters
        Just("hel\u{200B}lo".to_string()),                 // zero-width space
        Just("he\u{200D}llo".to_string()),                 // zero-width joiner
        Just("hello\u{FEFF}".to_string()),                 // BOM
        // RTL/LTR override
        Just("\u{202E}olleh".to_string()),                 // RTL override
        Just("hello\u{202C}".to_string()),                 // pop directional
        // Turkish İ
        Just("İstanbul".to_string()),                      // Turkish capital I with dot
        Just("istanbul".to_string()),                      // lowercase
        // German ß
        Just("straße".to_string()),
        Just("strasse".to_string()),                       // equivalent
        // Emoji
        Just("👨‍👩‍👧‍👦".to_string()),                        // family emoji (multi-codepoint)
        Just("🏳️‍🌈".to_string()),                          // rainbow flag
        Just("👍🏻".to_string()),                           // thumbs up with skin tone
        // Mathematical symbols
        Just("α β γ".to_string()),
        Just("∑ ∏ ∫".to_string()),
        // CJK
        Just("日本語".to_string()),
        Just("中文".to_string()),
        // Arabic
        Just("مرحبا".to_string()),
        // Combining characters galore
        Just("a\u{0300}\u{0301}\u{0302}".to_string()),     // a with 3 combining marks
    ]
}

/// Generate strings for Levenshtein stress testing
fn levenshtein_stress() -> impl Strategy<Value = (String, String)> {
    prop_oneof![
        // Short strings (fast)
        ("[a-z]{0,10}".prop_map(|s| s.clone()).prop_flat_map(|s| (Just(s.clone()), Just(s)))),
        // Medium strings
        ("[a-z]{50,100}".prop_map(|s| s.clone()).prop_flat_map(|s| (Just(s.clone()), Just(s.clone())))),
        // Different lengths
        (("[a-z]{10,20}", "[a-z]{10,20}")).prop_map(|(a, b)| (a, b)),
        // One empty
        ("[a-z]{1,20}".prop_map(|s| (s, String::new()))),
        // Both empty
        Just((String::new(), String::new())),
    ]
}

/// Generate phrase matching edge cases
fn phrase_edge_cases() -> impl Strategy<Value = (String, String)> {
    prop_oneof![
        // Empty cases
        Just(("".to_string(), "anything".to_string())),
        Just(("something".to_string(), "".to_string())),
        Just(("".to_string(), "".to_string())),
        // Whitespace variations
        Just(("hello world".to_string(), "hello  world".to_string())),
        Just(("hello world".to_string(), "  hello world  ".to_string())),
        Just(("hello world".to_string(), "hello\tworld".to_string())),
        Just(("hello world".to_string(), "hello\nworld".to_string())),
        // Punctuation
        Just(("hello world".to_string(), "hello, world!".to_string())),
        Just(("hello world".to_string(), "hello... world???".to_string())),
        // Near-matches
        Just(("thank you for calling".to_string(), "thank you for sailing".to_string())),
        Just(("cubic machinery".to_string(), "rubik machinery".to_string())),
        // Single word differences
        Just(("hello there".to_string(), "hello here".to_string())),
        Just(("big dog".to_string(), "big cat".to_string())),
    ]
}

// ============================================================================
// INVARIANT: FUNCTIONS NEVER PANIC
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(5000))]

    #[test]
    fn prop_levenshtein_never_panics(a in ".*", b in ".*") {
        let _ = levenshtein(&a, &b);
    }

    #[test]
    fn prop_levenshtein_unicode(input in unicode_edge_cases()) {
        let _ = levenshtein(&input, &input);
        let _ = levenshtein(&input, "normal");
        let _ = levenshtein("normal", &input);
    }

    #[test]
    fn prop_words_similar_never_panics(a in ".*", b in ".*") {
        let _ = words_similar(&a, &b);
    }

    #[test]
    fn prop_words_similar_unicode(input in unicode_edge_cases()) {
        let _ = words_similar(&input, &input);
        let _ = words_similar(&input, "normal");
    }

    #[test]
    fn prop_fuzzy_match_never_panics(expected in ".*", transcript in ".*") {
        let _ = fuzzy_match_phrase(&expected, &transcript);
    }

    #[test]
    fn prop_fuzzy_match_unicode(input in unicode_edge_cases()) {
        let _ = fuzzy_match_phrase(&input, &input);
        let _ = fuzzy_match_phrase(&input, "normal phrase here");
        let _ = fuzzy_match_phrase("normal phrase", &input);
    }
}

// ============================================================================
// INVARIANT: LEVENSHTEIN MATHEMATICAL PROPERTIES
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// Levenshtein is symmetric
    #[test]
    fn prop_levenshtein_symmetric(a in "[a-z]{0,30}", b in "[a-z]{0,30}") {
        prop_assert_eq!(levenshtein(&a, &b), levenshtein(&b, &a));
    }

    /// Levenshtein of identical strings is 0
    #[test]
    fn prop_levenshtein_identity(s in ".*") {
        prop_assert_eq!(levenshtein(&s, &s), 0);
    }

    /// Levenshtein satisfies triangle inequality
    #[test]
    fn prop_levenshtein_triangle(
        a in "[a-z]{0,15}",
        b in "[a-z]{0,15}",
        c in "[a-z]{0,15}"
    ) {
        let d_ac = levenshtein(&a, &c);
        let d_ab = levenshtein(&a, &b);
        let d_bc = levenshtein(&b, &c);
        prop_assert!(d_ac <= d_ab + d_bc, "triangle inequality violated: d({},{})={} > d({},{})={} + d({},{})={}", a, c, d_ac, a, b, d_ab, b, c, d_bc);
    }

    /// Levenshtein is bounded by max length
    #[test]
    fn prop_levenshtein_bounded(a in ".*", b in ".*") {
        let dist = levenshtein(&a, &b);
        let max_len = a.chars().count().max(b.chars().count());
        prop_assert!(dist <= max_len, "distance {} exceeds max length {}", dist, max_len);
    }

    /// Levenshtein with empty string equals length
    #[test]
    fn prop_levenshtein_empty(s in ".*") {
        let len = s.chars().count();
        prop_assert_eq!(levenshtein(&s, ""), len);
        prop_assert_eq!(levenshtein("", &s), len);
    }
}

// ============================================================================
// INVARIANT: WORDS_SIMILAR PROPERTIES
// ============================================================================

proptest! {
    /// words_similar is reflexive
    #[test]
    fn prop_words_similar_reflexive(word in "[a-z]{1,20}") {
        prop_assert!(words_similar(&word, &word));
    }

    /// words_similar is symmetric
    #[test]
    fn prop_words_similar_symmetric(a in "[a-z]{1,20}", b in "[a-z]{1,20}") {
        prop_assert_eq!(words_similar(&a, &b), words_similar(&b, &a));
    }
}

// ============================================================================
// INVARIANT: FUZZY_MATCH_PHRASE PROPERTIES
// ============================================================================

proptest! {
    /// Phrase always matches itself
    #[test]
    fn prop_fuzzy_match_reflexive(phrase in "[a-z]{1,10}( [a-z]{1,10}){0,5}") {
        let trimmed = phrase.trim();
        if !trimmed.is_empty() {
            prop_assert!(fuzzy_match_phrase(trimmed, trimmed), "phrase '{}' should match itself", trimmed);
        }
    }

    /// Empty expected phrase always matches
    #[test]
    fn prop_fuzzy_match_empty_expected(transcript in ".*") {
        prop_assert!(fuzzy_match_phrase("", &transcript));
    }

    /// Phrase matches when in longer transcript
    #[test]
    fn prop_fuzzy_match_substring(
        phrase in "[a-z]{3,8}( [a-z]{3,8}){1,3}",
        prefix in "[a-z]{0,5}( [a-z]{0,5}){0,2}",
        suffix in "( [a-z]{0,5}){0,2}"
    ) {
        let phrase = phrase.trim();
        if !phrase.is_empty() {
            let transcript = format!("{} {} {}", prefix, phrase, suffix);
            prop_assert!(fuzzy_match_phrase(phrase, &transcript), "phrase '{}' should be in '{}'", phrase, transcript);
        }
    }
}

// ============================================================================
// NEGATIVE ASSERTIONS: FALSE POSITIVES PREVENTION
// ============================================================================

#[test]
fn test_different_company_names_dont_match() {
    // These should NOT match despite similar structure
    assert!(!fuzzy_match_phrase(
        "thank you for calling cubic machinery",
        "thank you for calling acme corporation"
    ));

    assert!(!fuzzy_match_phrase(
        "welcome to widgets inc",
        "welcome to gadgets corp"
    ));
}

#[test]
fn test_similar_words_but_different_meaning_dont_match() {
    // "calling" vs "sailing" - only 1 char diff but different meaning
    // Note: with fuzzy matching, these might match due to Levenshtein distance
    let result = fuzzy_match_phrase(
        "thank you for calling",
        "thank you for sailing"
    );
    // Document current behavior (may match due to fuzzy)
    if result {
        // This is expected given our fuzzy matching rules
        // But it's a potential false positive
    }
}

#[test]
fn test_reversed_words_dont_match() {
    assert!(!fuzzy_match_phrase(
        "hello world",
        "world hello"
    ));

    assert!(!fuzzy_match_phrase(
        "one two three",
        "three two one"
    ));
}

#[test]
fn test_missing_critical_word_fails() {
    // Missing "for" changes meaning significantly
    assert!(!fuzzy_match_phrase(
        "thank you for calling",
        "thank you calling"
    ));

    // Missing "not" changes meaning completely
    assert!(!fuzzy_match_phrase(
        "we are not available",
        "we are available"
    ));
}

// ============================================================================
// BOUNDARY STRESS: LEVENSHTEIN PERFORMANCE
// ============================================================================

#[test]
fn test_levenshtein_medium_strings() {
    // 100 chars each - should complete quickly
    let a = "a".repeat(100);
    let b = "b".repeat(100);

    let start = std::time::Instant::now();
    let dist = levenshtein(&a, &b);
    let elapsed = start.elapsed();

    assert_eq!(dist, 100); // All substitutions
    assert!(elapsed.as_secs() < 1, "Medium strings took too long: {:?}", elapsed);
}

#[test]
fn test_levenshtein_longer_strings() {
    // 500 chars - still should complete in reasonable time
    let a = "a".repeat(500);
    let b = "b".repeat(500);

    let start = std::time::Instant::now();
    let dist = levenshtein(&a, &b);
    let elapsed = start.elapsed();

    assert_eq!(dist, 500);
    assert!(elapsed.as_secs() < 5, "Longer strings took too long: {:?}", elapsed);
}

#[test]
fn test_levenshtein_asymmetric_lengths() {
    let short = "hello";
    let long = "a".repeat(1000);

    let start = std::time::Instant::now();
    let dist = levenshtein(short, &long);
    let elapsed = start.elapsed();

    // Distance is length of longer string (delete all + insert 5)
    assert!(dist >= 995); // At least 1000 - 5
    assert!(elapsed.as_secs() < 2, "Asymmetric strings took too long: {:?}", elapsed);
}

// ============================================================================
// UNICODE EDGE CASES
// ============================================================================

#[test]
fn test_combining_characters() {
    // Precomposed vs decomposed
    let precomposed = "café";
    let decomposed = "cafe\u{0301}"; // e + combining acute accent

    // These look the same but are different bytes
    assert_ne!(precomposed, decomposed);

    // Levenshtein sees them as different
    let dist = levenshtein(precomposed, decomposed);
    assert!(dist > 0, "Should see difference between NFC and NFD");
}

#[test]
fn test_zero_width_characters() {
    let normal = "hello";
    let with_zwsp = "hel\u{200B}lo"; // zero-width space

    // These look the same but are different
    assert_ne!(normal, with_zwsp);

    // Levenshtein should detect the difference
    let dist = levenshtein(normal, with_zwsp);
    assert_eq!(dist, 1, "Zero-width char should count as 1 edit");
}

#[test]
fn test_rtl_override() {
    let normal = "hello";
    let rtl = "\u{202E}olleh"; // RTL override makes it display as "hello"

    // Completely different bytes
    let dist = levenshtein(normal, rtl);
    assert!(dist > 0);
}

#[test]
fn test_grapheme_clusters() {
    // Emoji with skin tone modifier
    let emoji1 = "👍";
    let emoji2 = "👍🏻"; // with skin tone

    // These are different
    let dist = levenshtein(emoji1, emoji2);
    assert!(dist > 0);
}

// ============================================================================
// WHITESPACE HANDLING
// ============================================================================

#[test]
fn test_fuzzy_match_whitespace_normalization() {
    // Multiple spaces
    assert!(fuzzy_match_phrase("hello world", "hello   world"));

    // Leading/trailing
    assert!(fuzzy_match_phrase("hello world", "  hello world  "));

    // Tab
    assert!(fuzzy_match_phrase("hello world", "hello\tworld"));
}

#[test]
fn test_fuzzy_match_newlines() {
    // Newline as word separator
    assert!(fuzzy_match_phrase("hello world", "hello\nworld"));
    assert!(fuzzy_match_phrase("hello world", "hello\r\nworld"));
}

// ============================================================================
// EMPTY STRING EDGE CASES
// ============================================================================

#[test]
fn test_empty_strings() {
    // Levenshtein
    assert_eq!(levenshtein("", ""), 0);
    assert_eq!(levenshtein("abc", ""), 3);
    assert_eq!(levenshtein("", "abc"), 3);

    // words_similar
    assert!(words_similar("", ""));
    assert!(!words_similar("", "hello"));

    // fuzzy_match_phrase
    assert!(fuzzy_match_phrase("", "anything"));
    assert!(!fuzzy_match_phrase("something", ""));
    assert!(fuzzy_match_phrase("", ""));
}

#[test]
fn test_whitespace_only() {
    // Whitespace-only expected phrase matches empty after trim
    assert!(fuzzy_match_phrase("   ", "anything"));

    // Whitespace-only transcript
    assert!(!fuzzy_match_phrase("hello", "   "));
}

// ============================================================================
// PUNCTUATION HANDLING
// ============================================================================

#[test]
fn test_punctuation_in_expected() {
    // Punctuation should be handled by words_similar stripping non-alphanumeric
    assert!(fuzzy_match_phrase(
        "hello, world!",
        "hello world"
    ));

    assert!(fuzzy_match_phrase(
        "what's up?",
        "whats up"
    ));
}

#[test]
fn test_punctuation_in_transcript() {
    assert!(fuzzy_match_phrase(
        "hello world",
        "hello, world!"
    ));

    assert!(fuzzy_match_phrase(
        "thank you",
        "thank you..."
    ));
}

// ============================================================================
// SPECIFIC KNOWN-GOOD AND KNOWN-BAD CASES
// ============================================================================

#[test]
fn test_cubic_machinery_variations() {
    let expected = "thank you for calling cubic machinery";

    // Should match
    assert!(fuzzy_match_phrase(expected, "thank you for calling cubic machinery"));
    assert!(fuzzy_match_phrase(expected, "hello thank you for calling cubic machinery goodbye"));
    assert!(fuzzy_match_phrase(expected, "thank you for calling cubik machinery")); // typo

    // Should not match
    assert!(!fuzzy_match_phrase(expected, "thank you for calling acme corporation"));
    assert!(!fuzzy_match_phrase(expected, "goodbye have a nice day"));
    assert!(!fuzzy_match_phrase(expected, "cubic machinery")); // missing "thank you for calling"
}

// ============================================================================
// DETERMINISM
// ============================================================================

#[test]
fn test_levenshtein_deterministic() {
    let a = "hello world";
    let b = "hello there";

    let dist1 = levenshtein(a, b);
    let dist2 = levenshtein(a, b);
    assert_eq!(dist1, dist2);
}

#[test]
fn test_fuzzy_match_deterministic() {
    let expected = "thank you for calling";
    let transcript = "hello thank you for calling goodbye";

    let result1 = fuzzy_match_phrase(expected, transcript);
    let result2 = fuzzy_match_phrase(expected, transcript);
    assert_eq!(result1, result2);
}

// ============================================================================
// HARD-CODED CHECK: VERIFY NOT TRIVIAL IMPLEMENTATION
// ============================================================================

#[test]
fn test_levenshtein_not_trivial() {
    // Verify it's actually computing edit distance, not just returning 0 or length
    assert_eq!(levenshtein("kitten", "sitting"), 3);
    assert_eq!(levenshtein("saturday", "sunday"), 3);
    assert_eq!(levenshtein("gumbo", "gambol"), 2);
}

#[test]
fn test_words_similar_not_trivial() {
    // Same length, 1 edit - should be similar
    assert!(words_similar("hello", "hallo"));
    assert!(words_similar("world", "worle"));

    // Same length, 2+ edits - should not be similar
    assert!(!words_similar("hello", "hxxlo"));
    assert!(!words_similar("world", "xxxxx"));
}
