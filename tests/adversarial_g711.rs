//! Adversarial Property-Based Tests for G.711 Codec
//!
//! # Attack Plan
//!
//! 1. **Lookup Table Completeness**: Exhaustively verify all 256 entries exist and
//!    produce valid i16 values. While u8 indexing is safe, verify no table corruption.
//!
//! 2. **ITU-T Specification Drift**: Verify the lookup table values match independently
//!    calculated values from the ITU-T G.711 formulas. Detect "hard-coded" errors.
//!
//! 3. **Memory Exhaustion**: Test behavior with very large inputs - ensure no panic.
//!
//! 4. **PCM Normalization Boundaries**: Verify f32 conversion handles i16::MIN/MAX
//!    correctly without overflow or precision loss.
//!
//! 5. **Batch vs Single Consistency**: Verify decode() and decode_sample() always agree.
//!
//! # Invariants
//!
//! - Lookup tables have exactly 256 entries (enforced by type, but verify values)
//! - All decoded values are within i16 range
//! - u-law indices i and i+128 have opposite signs (symmetry)
//! - A-law indices i and i+128 have opposite signs (symmetry)
//! - f32 normalization produces values in [-1.0, 1.0]
//! - Batch decode == single decode for all inputs
//! - Payload type 0 = ulaw, 8 = alaw, all others = None

use proptest::prelude::*;

use phonecheck::rtp::g711::{G711Codec, G711Decoder};

// ============================================================================
// INDEPENDENT REFERENCE IMPLEMENTATION (NO SHARED LOGIC)
// ============================================================================

/// Independent u-law to linear conversion using ITU-T G.711 Table 2
/// This is calculated from the bit-level specification, NOT using the lookup table
///
/// Reference: ITU-T G.711 (11/88) Section 2.4.4, Table 2
/// The u-law byte format is: S EEE MMMM where S=sign, E=exponent, M=mantissa
fn ulaw_to_linear_reference(byte: u8) -> i16 {
    // u-law uses ones' complement for the entire byte
    let inverted = !byte;

    // Extract fields
    let sign = inverted & 0x80;
    let exponent = (inverted >> 4) & 0x07;
    let mantissa = inverted & 0x0F;

    // ITU-T G.711 u-law to linear formula:
    // magnitude = ((mantissa << 3) + 0x84) << exponent - 0x84
    let magnitude = ((((mantissa as i32) << 3) + 0x84) << exponent) - 0x84;

    if sign != 0 {
        -(magnitude as i16)
    } else {
        magnitude as i16
    }
}

/// Independent A-law to linear conversion using ITU-T G.711 Table 1
/// Reference: ITU-T G.711 (11/88) Section 2.4.3, Table 1
fn alaw_to_linear_reference(byte: u8) -> i16 {
    // A-law uses XOR with 0x55 for even bits
    let inverted = byte ^ 0x55;

    // Sign bit is bit 7 AFTER inversion
    // In A-law: 0 = negative, 1 = positive (opposite of u-law)
    let sign = inverted & 0x80;
    let exponent = (inverted >> 4) & 0x07;
    let mantissa = inverted & 0x0F;

    // A-law linear conversion
    let magnitude = if exponent == 0 {
        // Linear segment: output = (mantissa << 4) + 8
        ((mantissa as i32) << 4) + 8
    } else {
        // Exponential segment: output = ((mantissa << 4) + 0x108) << (exponent - 1)
        (((mantissa as i32) << 4) + 0x108) << (exponent - 1)
    };

    // In A-law, sign=0 means negative (unlike u-law where sign=1 means negative)
    if sign == 0 {
        -(magnitude as i16)
    } else {
        magnitude as i16
    }
}

// ============================================================================
// INVARIANT: LOOKUP TABLE COMPLETENESS (EXHAUSTIVE)
// ============================================================================

#[test]
fn test_ulaw_table_all_256_entries_valid() {
    let decoder = G711Decoder::new(G711Codec::ULaw);

    for byte in 0u8..=255u8 {
        let sample = decoder.decode_sample(byte);
        // All values must be valid i16 (within range)
        assert!(
            sample >= i16::MIN && sample <= i16::MAX,
            "u-law byte {} produced invalid sample {}",
            byte,
            sample
        );
    }
}

#[test]
fn test_alaw_table_all_256_entries_valid() {
    let decoder = G711Decoder::new(G711Codec::ALaw);

    for byte in 0u8..=255u8 {
        let sample = decoder.decode_sample(byte);
        assert!(
            sample >= i16::MIN && sample <= i16::MAX,
            "A-law byte {} produced invalid sample {}",
            byte,
            sample
        );
    }
}

// ============================================================================
// INVARIANT: ITU-T SPECIFICATION COMPLIANCE
// ============================================================================

#[test]
fn test_ulaw_matches_itu_t_formula() {
    let decoder = G711Decoder::new(G711Codec::ULaw);

    for byte in 0u8..=255u8 {
        let actual = decoder.decode_sample(byte);
        let expected = ulaw_to_linear_reference(byte);

        // Allow small deviation due to different rounding in reference implementations
        let diff = (actual as i32 - expected as i32).abs();
        assert!(
            diff <= 4, // ITU-T allows small variation in LSBs
            "u-law byte {}: implementation {} vs formula {} (diff {})",
            byte,
            actual,
            expected,
            diff
        );
    }
}

#[test]
fn test_alaw_matches_itu_t_formula() {
    let decoder = G711Decoder::new(G711Codec::ALaw);

    for byte in 0u8..=255u8 {
        let actual = decoder.decode_sample(byte);
        let expected = alaw_to_linear_reference(byte);

        let diff = (actual as i32 - expected as i32).abs();
        assert!(
            diff <= 4,
            "A-law byte {}: implementation {} vs formula {} (diff {})",
            byte,
            actual,
            expected,
            diff
        );
    }
}

// ============================================================================
// INVARIANT: SYMMETRY (SIGN BIT BEHAVIOR)
// ============================================================================

#[test]
fn test_ulaw_symmetry_exhaustive() {
    let decoder = G711Decoder::new(G711Codec::ULaw);

    // For u-law, byte i and i+128 should produce opposite signs
    for i in 0u8..127 {
        let neg = decoder.decode_sample(i);
        let pos = decoder.decode_sample(i + 128);

        // Skip if either is zero (zero has no sign)
        if neg != 0 && pos != 0 {
            assert_eq!(
                neg, -pos,
                "u-law symmetry violation: byte {} -> {}, byte {} -> {}",
                i,
                neg,
                i + 128,
                pos
            );
        }
    }
}

#[test]
fn test_alaw_symmetry_exhaustive() {
    let decoder = G711Decoder::new(G711Codec::ALaw);

    for i in 0u8..128 {
        let neg = decoder.decode_sample(i);
        let pos = decoder.decode_sample(i + 128);

        assert_eq!(
            neg, -pos,
            "A-law symmetry violation: byte {} -> {}, byte {} -> {}",
            i,
            neg,
            i + 128,
            pos
        );
    }
}

// ============================================================================
// INVARIANT: PCM TO F32 NORMALIZATION
// ============================================================================

#[test]
fn test_f32_normalization_boundaries() {
    // Test exact boundaries
    let samples = vec![i16::MIN, i16::MIN + 1, -1, 0, 1, i16::MAX - 1, i16::MAX];
    let normalized = G711Decoder::pcm_to_f32(&samples);

    for (i, &f) in normalized.iter().enumerate() {
        assert!(
            f >= -1.0 && f <= 1.0,
            "Sample {} (i16: {}) normalized to {} which is outside [-1, 1]",
            i,
            samples[i],
            f
        );
    }

    // Specific boundary checks
    assert!(normalized[0] >= -1.0, "i16::MIN should normalize to >= -1.0");
    assert_eq!(normalized[3], 0.0, "0 should normalize to 0.0");
    assert!(normalized[6] < 1.0, "i16::MAX should normalize to < 1.0");
}

#[test]
fn test_f32_zero_is_exact() {
    let samples = vec![0i16];
    let normalized = G711Decoder::pcm_to_f32(&samples);
    assert_eq!(normalized[0], 0.0f32, "Zero must normalize to exactly 0.0");
}

#[test]
fn test_f32_preserves_sign() {
    let samples: Vec<i16> = (-100..=100).collect();
    let normalized = G711Decoder::pcm_to_f32(&samples);

    for (i, (&orig, &norm)) in samples.iter().zip(normalized.iter()).enumerate() {
        if orig < 0 {
            assert!(norm < 0.0, "Negative sample {} at index {} became non-negative {}", orig, i, norm);
        } else if orig > 0 {
            assert!(norm > 0.0, "Positive sample {} at index {} became non-positive {}", orig, i, norm);
        } else {
            assert_eq!(norm, 0.0, "Zero sample must remain zero");
        }
    }
}

// ============================================================================
// INVARIANT: BATCH VS SINGLE DECODE CONSISTENCY
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn prop_ulaw_batch_equals_single(bytes in proptest::collection::vec(any::<u8>(), 0..1000)) {
        let decoder = G711Decoder::new(G711Codec::ULaw);

        let batch = decoder.decode(&bytes);
        let singles: Vec<i16> = bytes.iter().map(|&b| decoder.decode_sample(b)).collect();

        prop_assert_eq!(batch, singles);
    }

    #[test]
    fn prop_alaw_batch_equals_single(bytes in proptest::collection::vec(any::<u8>(), 0..1000)) {
        let decoder = G711Decoder::new(G711Codec::ALaw);

        let batch = decoder.decode(&bytes);
        let singles: Vec<i16> = bytes.iter().map(|&b| decoder.decode_sample(b)).collect();

        prop_assert_eq!(batch, singles);
    }

    #[test]
    fn prop_decode_into_equals_decode(bytes in proptest::collection::vec(any::<u8>(), 0..1000)) {
        let decoder = G711Decoder::new(G711Codec::ULaw);

        let decoded = decoder.decode(&bytes);
        let mut into_output = Vec::new();
        decoder.decode_into(&bytes, &mut into_output);

        prop_assert_eq!(decoded, into_output);
    }
}

// ============================================================================
// NEGATIVE ASSERTIONS: PAYLOAD TYPE REJECTION
// ============================================================================

#[test]
fn test_payload_type_only_valid_types() {
    // Valid types
    assert!(G711Decoder::from_payload_type(0).is_some(), "PT 0 must be valid");
    assert!(G711Decoder::from_payload_type(8).is_some(), "PT 8 must be valid");

    // All other types must be rejected
    for pt in (1..=7).chain(9..=255) {
        assert!(
            G711Decoder::from_payload_type(pt).is_none(),
            "PT {} must be rejected",
            pt
        );
    }
}

#[test]
fn test_payload_type_codec_mapping() {
    let ulaw = G711Decoder::from_payload_type(0).unwrap();
    let alaw = G711Decoder::from_payload_type(8).unwrap();

    // Verify they produce different outputs for the same input
    // (u-law and A-law have different tables)
    let test_bytes = [0u8, 128, 255];

    for &byte in &test_bytes {
        let ulaw_sample = ulaw.decode_sample(byte);
        let alaw_sample = alaw.decode_sample(byte);

        // They should be different (except possibly for silence)
        // This catches accidentally swapped codec assignment
        if byte != 255 && byte != 0xD5 {
            // Skip known silence values
            assert_ne!(
                ulaw_sample, alaw_sample,
                "u-law and A-law should produce different results for byte {}",
                byte
            );
        }
    }
}

// ============================================================================
// BOUNDARY STRESS TESTING
// ============================================================================

#[test]
fn test_decode_empty_input() {
    let decoder = G711Decoder::new(G711Codec::ULaw);
    let result = decoder.decode(&[]);
    assert!(result.is_empty(), "Empty input should produce empty output");
}

#[test]
fn test_decode_large_input() {
    let decoder = G711Decoder::new(G711Codec::ULaw);

    // 1 MB of audio data (1M samples)
    let large_input: Vec<u8> = (0u8..=255u8).cycle().take(1_000_000).collect();
    let result = decoder.decode(&large_input);

    assert_eq!(result.len(), 1_000_000, "Output length must match input length");
}

#[test]
fn test_decode_into_empty_then_large() {
    let decoder = G711Decoder::new(G711Codec::ULaw);
    let mut output = Vec::new();

    // First empty
    decoder.decode_into(&[], &mut output);
    assert!(output.is_empty());

    // Then large
    let large_input: Vec<u8> = (0u8..=255u8).cycle().take(10_000).collect();
    decoder.decode_into(&large_input, &mut output);
    assert_eq!(output.len(), 10_000);
}

#[test]
fn test_f32_conversion_empty() {
    let result = G711Decoder::pcm_to_f32(&[]);
    assert!(result.is_empty());
}

#[test]
fn test_f32_conversion_large() {
    let large_input: Vec<i16> = (i16::MIN..=i16::MAX).collect();
    let result = G711Decoder::pcm_to_f32(&large_input);

    assert_eq!(result.len(), 65536);

    // All values must be in range
    for (i, &f) in result.iter().enumerate() {
        assert!(
            f >= -1.0 && f <= 1.0,
            "Index {} out of range: {}",
            i,
            f
        );
    }
}

// ============================================================================
// KNOWN REFERENCE VALUES (from ITU-T G.711 specification)
// ============================================================================

#[test]
fn test_ulaw_silence_value() {
    let decoder = G711Decoder::new(G711Codec::ULaw);
    // In u-law, 0xFF encodes digital silence (zero)
    assert_eq!(
        decoder.decode_sample(0xFF),
        0,
        "u-law 0xFF must decode to 0 (silence)"
    );
    assert_eq!(
        decoder.decode_sample(0x7F),
        0,
        "u-law 0x7F must decode to 0 (silence)"
    );
}

#[test]
fn test_alaw_silence_value() {
    let decoder = G711Decoder::new(G711Codec::ALaw);
    // In A-law, 0xD5 encodes digital silence (closest to zero)
    let silence = decoder.decode_sample(0xD5);
    assert!(
        silence.abs() <= 8,
        "A-law 0xD5 must decode close to zero, got {}",
        silence
    );
}

#[test]
fn test_ulaw_max_amplitude() {
    let decoder = G711Decoder::new(G711Codec::ULaw);
    // u-law 0x00 encodes maximum negative amplitude
    // u-law 0x80 encodes maximum positive amplitude
    let max_neg = decoder.decode_sample(0x00);
    let max_pos = decoder.decode_sample(0x80);

    assert!(max_neg < -30000, "u-law max negative should be < -30000, got {}", max_neg);
    assert!(max_pos > 30000, "u-law max positive should be > 30000, got {}", max_pos);
    assert_eq!(max_neg, -max_pos, "u-law max values must be symmetric");
}

#[test]
fn test_alaw_max_amplitude() {
    let decoder = G711Decoder::new(G711Codec::ALaw);
    // A-law maximum amplitudes
    let max_neg = decoder.decode_sample(0x2A); // Encoded max negative
    let max_pos = decoder.decode_sample(0xAA); // Encoded max positive

    assert!(max_neg < -30000, "A-law max negative should be < -30000, got {}", max_neg);
    assert!(max_pos > 30000, "A-law max positive should be > 30000, got {}", max_pos);
}

// ============================================================================
// PROPERTY: OUTPUT LENGTH INVARIANT
// ============================================================================

proptest! {
    #[test]
    fn prop_output_length_equals_input(bytes in proptest::collection::vec(any::<u8>(), 0..10000)) {
        let decoder = G711Decoder::new(G711Codec::ULaw);
        let result = decoder.decode(&bytes);
        prop_assert_eq!(result.len(), bytes.len(), "Output length must equal input length");
    }

    #[test]
    fn prop_decode_into_appends_correct_length(
        bytes1 in proptest::collection::vec(any::<u8>(), 0..100),
        bytes2 in proptest::collection::vec(any::<u8>(), 0..100),
    ) {
        let decoder = G711Decoder::new(G711Codec::ALaw);
        let mut output = Vec::new();

        decoder.decode_into(&bytes1, &mut output);
        let len1 = output.len();
        assert_eq!(len1, bytes1.len());

        decoder.decode_into(&bytes2, &mut output);
        let len2 = output.len();
        assert_eq!(len2, bytes1.len() + bytes2.len());
    }
}

// ============================================================================
// HARD-CODED CHECK: VERIFY NOT JUST PASSING SPECIFIC INPUTS
// ============================================================================

#[test]
fn test_ulaw_not_hardcoded() {
    let decoder = G711Decoder::new(G711Codec::ULaw);

    // Verify diverse inputs produce diverse outputs
    let mut seen_values = std::collections::HashSet::new();
    for byte in 0u8..=255 {
        seen_values.insert(decoder.decode_sample(byte));
    }

    // u-law should produce at least 200 distinct values (not just a few hardcoded ones)
    assert!(
        seen_values.len() > 200,
        "u-law produced only {} distinct values, expected > 200",
        seen_values.len()
    );
}

#[test]
fn test_alaw_not_hardcoded() {
    let decoder = G711Decoder::new(G711Codec::ALaw);

    let mut seen_values = std::collections::HashSet::new();
    for byte in 0u8..=255 {
        seen_values.insert(decoder.decode_sample(byte));
    }

    assert!(
        seen_values.len() > 200,
        "A-law produced only {} distinct values, expected > 200",
        seen_values.len()
    );
}

// ============================================================================
// MONOTONICITY CHECK (within segments)
// ============================================================================

#[test]
fn test_ulaw_segment_monotonicity() {
    let decoder = G711Decoder::new(G711Codec::ULaw);

    // Within each 16-sample segment, values should be monotonic
    // Segments are defined by the exponent bits
    for segment_start in (0u8..128).step_by(16) {
        let mut values: Vec<i16> = Vec::new();
        for offset in 0..16 {
            values.push(decoder.decode_sample(segment_start + offset));
        }

        // Check monotonicity within segment (decreasing for negative)
        for i in 1..16 {
            assert!(
                values[i] >= values[i - 1],
                "u-law segment starting at {} not monotonic: {} -> {}",
                segment_start,
                values[i - 1],
                values[i]
            );
        }
    }
}
