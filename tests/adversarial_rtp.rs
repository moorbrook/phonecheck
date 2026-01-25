//! Adversarial Property-Based Tests for RTP Packet Handling
//!
//! # Attack Plan
//!
//! 1. **Malformed RTP Headers**: Truncated packets, wrong version, invalid CSRC counts
//!    that could cause out-of-bounds reads.
//!
//! 2. **Extension Header Overflow**: Extension length field that exceeds actual packet
//!    size, causing potential buffer over-read.
//!
//! 3. **Sequence Number Wraparound**: 65535 -> 0 transition must be handled correctly
//!    for packet ordering.
//!
//! 4. **Memory Exhaustion**: Many packets inserted into jitter buffer without consuming,
//!    or packets with huge payloads.
//!
//! 5. **Jitter Buffer Attacks**: Massive out-of-order delivery, late packet floods,
//!    duplicate packet storms.
//!
//! 6. **Resampling Edge Cases**: Empty input, single sample, maximum f32 values.
//!
//! # Invariants
//!
//! - parse_rtp_header never panics on any input
//! - parse_rtp_header rejects non-v2 packets
//! - Jitter buffer never panics on any sequence of operations
//! - Jitter buffer enforces max_size limit
//! - Sequence number wraparound is handled correctly
//! - Resampled output length is exactly 2x input length

use proptest::prelude::*;

use phonecheck::rtp::jitter::{BufferedPacket, JitterBuffer, JitterBufferConfig};
use phonecheck::rtp::receiver::{parse_rtp_header, resample_8k_to_16k};

// ============================================================================
// ADVERSARIAL GENERATORS
// ============================================================================

/// Generate malformed RTP packets
fn malformed_rtp_packet() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        // Too short
        Just(vec![]),
        Just(vec![0x80]),
        Just(vec![0x80, 0x00]),
        Just(vec![0x80, 0x00, 0x00, 0x01]),
        Just(vec![0x80, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x10]),
        Just(vec![0x80, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00]), // 11 bytes
        // Wrong version (0, 1, 3)
        Just(vec![0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01]),
        Just(vec![0x40, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01]),
        Just(vec![0xC0, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01]),
        // Max CSRC count (15) but no CSRC data
        Just(vec![0x8F, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01]),
        // Extension bit set but no extension data
        Just(vec![0x90, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01]),
        // Extension with bogus length (claims 65535 32-bit words)
        Just(vec![
            0x90, 0x00, // V=2, X=1
            0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01,
            0xBE, 0xDE, // Extension header ID
            0xFF, 0xFF, // Extension length = 65535 words = 262140 bytes
        ]),
        // All bits set
        Just(vec![0xFF; 100]),
        // All zeros
        Just(vec![0x00; 100]),
    ]
}

/// Generate valid RTP packet structure
fn valid_rtp_packet(payload_size: usize) -> impl Strategy<Value = Vec<u8>> {
    (0u8..128u8, any::<u16>(), any::<u32>(), any::<u32>()).prop_map(
        move |(pt, seq, ts, ssrc)| {
            let mut packet = vec![0x80, pt]; // V=2, PT
            packet.extend_from_slice(&seq.to_be_bytes());
            packet.extend_from_slice(&ts.to_be_bytes());
            packet.extend_from_slice(&ssrc.to_be_bytes());
            packet.extend(vec![0u8; payload_size]);
            packet
        },
    )
}

/// Generate sequence numbers that test wraparound
fn wraparound_sequences() -> impl Strategy<Value = Vec<u16>> {
    prop_oneof![
        // Normal ascending
        Just((0..100).collect::<Vec<u16>>()),
        // Around wraparound point
        Just((65530..=65535).chain(0..10).collect::<Vec<u16>>()),
        // Just after wraparound
        Just((0..10).collect::<Vec<u16>>()),
        // High numbers
        Just((65500..65535).collect::<Vec<u16>>()),
        // Random with wraparound
        proptest::collection::vec(any::<u16>(), 10..50),
    ]
}

// ============================================================================
// INVARIANT: PARSERS NEVER PANIC
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(10000))]

    #[test]
    fn prop_parse_rtp_header_never_panics(data in proptest::collection::vec(any::<u8>(), 0..200)) {
        let _ = parse_rtp_header(&data);
    }

    #[test]
    fn prop_parse_rtp_header_malformed(data in malformed_rtp_packet()) {
        let _ = parse_rtp_header(&data);
    }
}

// Resampling is expensive (FFT), so use fewer cases and smaller samples
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn prop_resample_never_panics(samples in proptest::collection::vec(any::<f32>(), 0..100)) {
        let _ = resample_8k_to_16k(&samples);
    }
}

// ============================================================================
// INVARIANT: VERSION CHECK
// ============================================================================

#[test]
fn test_rejects_all_non_v2_versions() {
    let base_packet = [0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01];

    for version in [0u8, 1, 3] {
        let mut packet = base_packet;
        packet[0] = (version << 6) | (packet[0] & 0x3F);
        assert!(
            parse_rtp_header(&packet).is_none(),
            "Version {} should be rejected",
            version
        );
    }
}

#[test]
fn test_accepts_v2() {
    let packet = [0x80, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01];
    assert!(parse_rtp_header(&packet).is_some());
}

// ============================================================================
// INVARIANT: PAYLOAD OFFSET CALCULATION
// ============================================================================

#[test]
fn test_csrc_offset_calculation() {
    // CC=0 -> offset=12
    let packet = [0x80, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01];
    let (_, _, _, _, offset) = parse_rtp_header(&packet).unwrap();
    assert_eq!(offset, 12);

    // CC=1 -> offset=16
    let mut packet = [0x81, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02];
    let (_, _, _, _, offset) = parse_rtp_header(&packet).unwrap();
    assert_eq!(offset, 16);

    // CC=15 (max) -> offset=72
    packet[0] = 0x8F;
    // Need a larger packet for this
    let mut big_packet = vec![0x8F, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01];
    big_packet.extend(vec![0u8; 60]); // 15 CSRCs * 4 bytes
    let (_, _, _, _, offset) = parse_rtp_header(&big_packet).unwrap();
    assert_eq!(offset, 12 + 15 * 4); // 72
}

#[test]
fn test_extension_offset_calculation() {
    // Extension with length=1 (4 bytes of extension data)
    let packet = [
        0x90, 0x00, // V=2, X=1, CC=0
        0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01,
        0xBE, 0xDE, // Extension ID
        0x00, 0x01, // Extension length = 1 (4 bytes)
        0x00, 0x00, 0x00, 0x00, // Extension data
        0xAA, // Payload
    ];
    let (_, _, _, _, offset) = parse_rtp_header(&packet).unwrap();
    assert_eq!(offset, 12 + 4 + 4); // 20
}

#[test]
fn test_extension_length_overflow_safe() {
    // Extension claims to be longer than packet
    // Need > 16 bytes for the extension to be read (condition is data.len() > offset + 4)
    let mut packet = vec![
        0x90, 0x00, // V=2, X=1
        0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01, // 12 bytes header
        0xBE, 0xDE, // Extension ID
        0xFF, 0xFF, // Extension length = 65535 words (way more than packet)
        0x00, // Extra byte to make len > offset + 4
    ];

    // Should not panic, should return an offset (even if it's past the packet)
    let result = parse_rtp_header(&packet);
    assert!(result.is_some());
    let (_, _, _, _, offset) = result.unwrap();
    // Offset will be calculated as: 12 + 4 + 65535*4 = 262160
    // This is fine - the caller must check if offset < packet.len()
    assert!(offset > packet.len(), "Offset {} should be > packet len {}", offset, packet.len());
}

// ============================================================================
// JITTER BUFFER: NEVER PANICS
// ============================================================================

fn make_packet(seq: u16) -> BufferedPacket {
    BufferedPacket {
        sequence: seq,
        timestamp: seq as u32 * 160,
        payload: vec![0u8; 160],
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn prop_jitter_buffer_never_panics(
        sequences in proptest::collection::vec(any::<u16>(), 0..200),
        pop_decisions in proptest::collection::vec(any::<bool>(), 0..200)
    ) {
        let mut buffer = JitterBuffer::new(JitterBufferConfig::default());

        for (i, seq) in sequences.iter().enumerate() {
            buffer.insert(make_packet(*seq));
            // Use proptest-generated boolean for deterministic shrinking
            if pop_decisions.get(i).copied().unwrap_or(false) {
                let _ = buffer.pop();
            }
        }

        // Drain at the end
        let _ = buffer.drain();
    }

    /// Test jitter buffer with contiguous sequences inserted in order
    /// This tests the core guarantee: packets inserted in order are all output in order
    #[test]
    fn prop_jitter_buffer_contiguous(
        base in 0u16..65000u16,
        offsets in proptest::collection::vec(0u16..100u16, 1..50)
    ) {
        let mut buffer = JitterBuffer::new(JitterBufferConfig {
            target_depth: 0,  // Don't hold packets
            max_size: 1000,   // Large enough for test
            max_gap: 200,     // Allow gaps in test data
        });

        // Generate sequences as base + offset, deduplicate, then SORT
        let mut unique_offsets: Vec<u16> = Vec::new();
        for offset in &offsets {
            if !unique_offsets.contains(offset) {
                unique_offsets.push(*offset);
            }
        }
        unique_offsets.sort();

        let sequences: Vec<u16> = unique_offsets
            .iter()
            .map(|&off| base.wrapping_add(off))
            .collect();

        // Insert in sorted order (simulating well-ordered network)
        for &seq in &sequences {
            buffer.insert(make_packet(seq));
        }

        // Extract all packets
        let mut output_seqs: Vec<u16> = Vec::new();
        while let Some(packet) = buffer.pop() {
            output_seqs.push(packet.sequence);
        }
        output_seqs.extend(buffer.drain().iter().map(|p| p.sequence));

        // Assert: no duplicates in output
        let mut seen = std::collections::HashSet::new();
        for seq in &output_seqs {
            prop_assert!(seen.insert(*seq), "Duplicate sequence {} in output", seq);
        }

        // Assert: output count matches input (packets inserted in order are never "late")
        prop_assert_eq!(
            output_seqs.len(),
            sequences.len(),
            "Expected {} packets, got {}. Input: {:?}, Output: {:?}",
            sequences.len(),
            output_seqs.len(),
            sequences,
            output_seqs
        );

        // Assert: output matches input order exactly
        prop_assert_eq!(
            output_seqs,
            sequences,
            "Output order doesn't match input order"
        );
    }

    /// Test the specific wraparound case: 65533, 65534, 65535, 0, 1, 2
    #[test]
    fn prop_jitter_buffer_wraparound_specific(
        start_offset in 0u16..10u16
    ) {
        let mut buffer = JitterBuffer::new(JitterBufferConfig {
            target_depth: 0,
            max_size: 20,
            max_gap: 10,
        });

        // Insert packets around wraparound in order
        let sequences: Vec<u16> = (65533u16..=65535)
            .chain(0..6)
            .map(|s| s.wrapping_add(start_offset))
            .collect();

        for &seq in &sequences {
            buffer.insert(make_packet(seq));
        }

        // Pop all
        let mut output = Vec::new();
        while let Some(packet) = buffer.pop() {
            output.push(packet.sequence);
        }
        output.extend(buffer.drain().iter().map(|p| p.sequence));

        // All 9 packets should be output
        prop_assert_eq!(output.len(), 9, "Expected 9 packets, got {}", output.len());

        // Should be in order
        prop_assert_eq!(output, sequences);
    }
}

// ============================================================================
// JITTER BUFFER: MAX SIZE ENFORCEMENT
// ============================================================================

#[test]
fn test_jitter_buffer_enforces_max_size() {
    let mut buffer = JitterBuffer::new(JitterBufferConfig {
        target_depth: 3,
        max_size: 10,
        max_gap: 5,
    });

    // Insert 20 packets (more than max_size)
    for seq in 0..20u16 {
        buffer.insert(make_packet(seq));
    }

    // Should have at most max_size packets
    let stats = buffer.stats();
    assert!(stats.current_depth <= 10, "Buffer exceeded max_size");
}

#[test]
fn test_jitter_buffer_drops_oldest_on_overflow() {
    let mut buffer = JitterBuffer::new(JitterBufferConfig {
        target_depth: 2,
        max_size: 5,
        max_gap: 10,
    });

    // Insert packets 0-9
    for seq in 0..10u16 {
        buffer.insert(make_packet(seq));
    }

    // Buffer should have dropped oldest packets (0-4)
    // Remaining should be 5-9
    let drained = buffer.drain();
    let sequences: Vec<u16> = drained.iter().map(|p| p.sequence).collect();

    // Should not contain 0-4 (they were dropped)
    for seq in 0..5u16 {
        assert!(!sequences.contains(&seq), "Packet {} should have been dropped", seq);
    }
}

// ============================================================================
// JITTER BUFFER: SEQUENCE WRAPAROUND
// ============================================================================

#[test]
fn test_jitter_buffer_handles_wraparound() {
    let mut buffer = JitterBuffer::new(JitterBufferConfig {
        target_depth: 2,
        max_size: 20,
        max_gap: 5,
    });

    // Insert packets around wraparound: 65533, 65534, 65535, 0, 1, 2
    for seq in [65533u16, 65534, 65535, 0, 1, 2] {
        buffer.insert(make_packet(seq));
    }

    // Pop all and verify order
    let mut output = Vec::new();
    while let Some(packet) = buffer.pop() {
        output.push(packet.sequence);
    }
    output.extend(buffer.drain().iter().map(|p| p.sequence));

    // Should be in order: 65533, 65534, 65535, 0, 1, 2
    assert_eq!(output, vec![65533, 65534, 65535, 0, 1, 2]);
}

#[test]
fn test_jitter_buffer_late_packet_after_wraparound() {
    let mut buffer = JitterBuffer::new(JitterBufferConfig {
        target_depth: 2,
        max_size: 20,
        max_gap: 5,
    });

    // Insert 0, 1, 2, 3
    for seq in 0..4u16 {
        buffer.insert(make_packet(seq));
    }

    // Pop 0, 1
    assert_eq!(buffer.pop().unwrap().sequence, 0);
    assert_eq!(buffer.pop().unwrap().sequence, 1);

    // Now insert a "late" packet from before wraparound (65535)
    // This should be rejected as late
    let accepted = buffer.insert(make_packet(65535));
    assert!(!accepted, "Late packet 65535 should be rejected after 0,1 output");
}

// ============================================================================
// JITTER BUFFER: DUPLICATE REJECTION
// ============================================================================

#[test]
fn test_jitter_buffer_rejects_duplicates() {
    let mut buffer = JitterBuffer::new(JitterBufferConfig::default());

    // Insert packet 0
    assert!(buffer.insert(make_packet(0)));

    // Insert duplicate - should be rejected
    assert!(!buffer.insert(make_packet(0)));

    // Stats should show 1 dropped
    let stats = buffer.stats();
    assert_eq!(stats.packets_dropped, 1);
}

// ============================================================================
// JITTER BUFFER: GAP HANDLING
// ============================================================================

#[test]
fn test_jitter_buffer_skips_large_gaps() {
    let mut buffer = JitterBuffer::new(JitterBufferConfig {
        target_depth: 2,
        max_size: 20,
        max_gap: 3,
    });

    // Insert 0, 1, then skip to 20
    buffer.insert(make_packet(0));
    buffer.insert(make_packet(1));
    buffer.insert(make_packet(20));
    buffer.insert(make_packet(21));

    // Pop should output 0, 1
    assert_eq!(buffer.pop().unwrap().sequence, 0);
    assert_eq!(buffer.pop().unwrap().sequence, 1);

    // Now there's a gap of 18 (2-19 missing), which exceeds max_gap=3
    // Should skip to 20
    let next = buffer.pop();
    assert!(next.is_some());
    assert_eq!(next.unwrap().sequence, 20);

    // Should report lost packets
    let stats = buffer.stats();
    assert!(stats.packets_lost > 0);
}

// ============================================================================
// RESAMPLING: ORACLE-BASED TESTS (no shadow implementations)
// ============================================================================

proptest! {
    #[test]
    fn prop_resample_doubles_length(samples in proptest::collection::vec(-1.0f32..1.0f32, 0..500)) {
        let output = resample_8k_to_16k(&samples);
        if samples.is_empty() {
            prop_assert_eq!(output.len(), 0);
        } else {
            prop_assert_eq!(output.len(), samples.len() * 2);
        }
    }

    #[test]
    fn prop_resample_preserves_range(samples in proptest::collection::vec(-1.0f32..1.0f32, 1..100)) {
        let output = resample_8k_to_16k(&samples);
        let min_in = samples.iter().cloned().fold(f32::INFINITY, f32::min);
        let max_in = samples.iter().cloned().fold(f32::NEG_INFINITY, f32::max);

        for sample in &output {
            prop_assert!(*sample >= min_in && *sample <= max_in);
        }
    }

    /// Original samples preserved at even indices (oracle property)
    #[test]
    fn prop_resample_preserves_originals(samples in proptest::collection::vec(-1.0f32..1.0f32, 1..100)) {
        let output = resample_8k_to_16k(&samples);
        for (i, &original) in samples.iter().enumerate() {
            prop_assert_eq!(output[i * 2], original, "Original sample at index {} not preserved", i);
        }
    }

    /// Interpolated samples are between adjacent originals (oracle property)
    #[test]
    fn prop_resample_interpolation_bounded(samples in proptest::collection::vec(-1.0f32..1.0f32, 2..100)) {
        let output = resample_8k_to_16k(&samples);
        for i in 0..samples.len() - 1 {
            let interpolated = output[i * 2 + 1];
            let lo = samples[i].min(samples[i + 1]);
            let hi = samples[i].max(samples[i + 1]);
            prop_assert!(
                interpolated >= lo && interpolated <= hi,
                "Interpolated value {} not between {} and {}", interpolated, lo, hi
            );
        }
    }
}

#[test]
fn test_resample_empty() {
    assert_eq!(resample_8k_to_16k(&[]).len(), 0);
}

#[test]
fn test_resample_single() {
    let output = resample_8k_to_16k(&[0.5]);
    assert_eq!(output.len(), 2);
    assert_eq!(output[0], 0.5);
    assert_eq!(output[1], 0.5);
}

/// Oracle test: known input/output pairs for linear interpolation
#[test]
fn test_resample_oracle_known_values() {
    // [0.0, 1.0] -> [0.0, 0.5, 1.0, 1.0]
    let output = resample_8k_to_16k(&[0.0, 1.0]);
    assert_eq!(output, vec![0.0, 0.5, 1.0, 1.0]);

    // [1.0, 0.0] -> [1.0, 0.5, 0.0, 0.0]
    let output = resample_8k_to_16k(&[1.0, 0.0]);
    assert_eq!(output, vec![1.0, 0.5, 0.0, 0.0]);

    // [-1.0, 1.0] -> [-1.0, 0.0, 1.0, 1.0]
    let output = resample_8k_to_16k(&[-1.0, 1.0]);
    assert_eq!(output, vec![-1.0, 0.0, 1.0, 1.0]);

    // [0.0, 0.0, 0.0] -> [0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
    let output = resample_8k_to_16k(&[0.0, 0.0, 0.0]);
    assert_eq!(output, vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);

    // Ramp: [0.0, 0.25, 0.5, 0.75, 1.0]
    let output = resample_8k_to_16k(&[0.0, 0.25, 0.5, 0.75, 1.0]);
    assert_eq!(output[0], 0.0);
    assert_eq!(output[1], 0.125);  // (0.0 + 0.25) / 2
    assert_eq!(output[2], 0.25);
    assert_eq!(output[3], 0.375);  // (0.25 + 0.5) / 2
    assert_eq!(output[4], 0.5);
    assert_eq!(output[5], 0.625);  // (0.5 + 0.75) / 2
    assert_eq!(output[6], 0.75);
    assert_eq!(output[7], 0.875);  // (0.75 + 1.0) / 2
    assert_eq!(output[8], 1.0);
    assert_eq!(output[9], 1.0);    // last duplicated
}

#[test]
fn test_resample_nan_and_inf() {
    // NaN and Inf should not panic (though output may be garbage)
    let samples = vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0];
    let output = resample_8k_to_16k(&samples);
    assert_eq!(output.len(), 8);
}

// ============================================================================
// FLOAT EDGE CASES: NaN, Inf, Subnormals
// ============================================================================

/// Generator for problematic float values
fn problematic_floats() -> impl Strategy<Value = f32> {
    prop_oneof![
        // Normal range
        (-1.0f32..1.0f32),
        // Infinities
        Just(f32::INFINITY),
        Just(f32::NEG_INFINITY),
        // NaN (multiple representations)
        Just(f32::NAN),
        Just(f32::from_bits(0x7FC00001)), // Quiet NaN with payload
        Just(f32::from_bits(0xFFC00001)), // Negative quiet NaN
        // Subnormals (denormalized numbers)
        Just(f32::MIN_POSITIVE / 2.0),
        Just(-f32::MIN_POSITIVE / 2.0),
        Just(f32::from_bits(0x00000001)), // Smallest positive subnormal
        Just(f32::from_bits(0x80000001)), // Smallest negative subnormal
        // Extremes
        Just(f32::MAX),
        Just(f32::MIN),
        Just(-0.0f32),
        Just(0.0f32),
    ]
}

proptest! {
    /// Resampling with any float values never panics
    #[test]
    fn prop_resample_problematic_floats_no_panic(
        samples in proptest::collection::vec(problematic_floats(), 0..100)
    ) {
        let output = resample_8k_to_16k(&samples);
        // Only guarantee: output length is 2x input (or 0 for empty)
        if samples.is_empty() {
            prop_assert_eq!(output.len(), 0);
        } else {
            prop_assert_eq!(output.len(), samples.len() * 2);
        }
    }

    /// NaN propagation: if input contains NaN, output contains NaN at expected positions
    #[test]
    fn prop_resample_nan_propagation(
        prefix in proptest::collection::vec(-1.0f32..1.0f32, 0..10),
        suffix in proptest::collection::vec(-1.0f32..1.0f32, 0..10)
    ) {
        // Construct input with NaN in the middle
        let mut samples = prefix.clone();
        samples.push(f32::NAN);
        samples.extend(suffix.iter().cloned());

        let output = resample_8k_to_16k(&samples);
        let nan_input_idx = prefix.len();

        // NaN should appear at output index nan_input_idx * 2
        prop_assert!(output[nan_input_idx * 2].is_nan(), "Original NaN sample not preserved");

        // Interpolated value involving NaN should also be NaN
        if nan_input_idx + 1 < samples.len() {
            prop_assert!(output[nan_input_idx * 2 + 1].is_nan(), "Interpolation with NaN should be NaN");
        }
        if nan_input_idx > 0 {
            // Check interpolation before NaN
            let interp_idx = (nan_input_idx - 1) * 2 + 1;
            prop_assert!(output[interp_idx].is_nan(), "Interpolation before NaN should be NaN");
        }
    }

    /// Infinity propagation: infinities are preserved
    #[test]
    fn prop_resample_infinity_preservation(
        prefix in proptest::collection::vec(-1.0f32..1.0f32, 1..10),
        use_neg_inf in any::<bool>()
    ) {
        let inf_val = if use_neg_inf { f32::NEG_INFINITY } else { f32::INFINITY };

        let mut samples = prefix.clone();
        samples.push(inf_val);

        let output = resample_8k_to_16k(&samples);
        let inf_input_idx = prefix.len();

        // Infinity should be preserved at the expected position
        prop_assert_eq!(output[inf_input_idx * 2], inf_val, "Infinity not preserved");
    }

    /// Subnormal handling: subnormals don't cause issues
    #[test]
    fn prop_resample_subnormals(
        subnormal_bits in 1u32..0x007FFFFFu32, // Valid subnormal bit patterns
        is_negative in any::<bool>()
    ) {
        let sign_bit = if is_negative { 0x80000000u32 } else { 0u32 };
        let subnormal = f32::from_bits(subnormal_bits | sign_bit);

        // Verify it's actually subnormal
        prop_assume!(subnormal.is_subnormal() || subnormal == 0.0);

        let samples = vec![0.0, subnormal, 0.0];
        let output = resample_8k_to_16k(&samples);

        prop_assert_eq!(output.len(), 6);
        // Subnormal should be preserved
        prop_assert_eq!(output[2], subnormal, "Subnormal not preserved");
    }
}

// ============================================================================
// BOUNDARY STRESS TESTING
// ============================================================================

#[test]
fn test_parse_exactly_12_bytes() {
    let packet = [0x80, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01];
    let result = parse_rtp_header(&packet);
    assert!(result.is_some());
}

#[test]
fn test_parse_11_bytes_rejected() {
    let packet = [0x80, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00];
    assert!(parse_rtp_header(&packet).is_none());
}

#[test]
fn test_jitter_buffer_zero_config() {
    // Edge case: zero target_depth and max_size
    let mut buffer = JitterBuffer::new(JitterBufferConfig {
        target_depth: 0,
        max_size: 0,
        max_gap: 0,
    });

    // Should still not panic
    buffer.insert(make_packet(0));
    let _ = buffer.pop();
    let _ = buffer.drain();
}

#[test]
fn test_jitter_buffer_max_config() {
    let mut buffer = JitterBuffer::new(JitterBufferConfig {
        target_depth: u16::MAX,
        max_size: u16::MAX,
        max_gap: u16::MAX,
    });

    // Insert a few packets
    for seq in 0..10u16 {
        buffer.insert(make_packet(seq));
    }

    // Should not output anything (target_depth not reached)
    assert!(buffer.pop().is_none());

    // But drain should work
    let drained = buffer.drain();
    assert_eq!(drained.len(), 10);
}

// ============================================================================
// MEMORY EXHAUSTION PROTECTION
// ============================================================================

#[test]
fn test_jitter_buffer_large_payload() {
    let mut buffer = JitterBuffer::new(JitterBufferConfig::default());

    // Insert packet with 1MB payload
    let large_packet = BufferedPacket {
        sequence: 0,
        timestamp: 0,
        payload: vec![0u8; 1_000_000],
    };

    buffer.insert(large_packet);
    let drained = buffer.drain();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].payload.len(), 1_000_000);
}

// ============================================================================
// DETERMINISM
// ============================================================================

#[test]
fn test_jitter_buffer_deterministic() {
    fn run_sequence() -> Vec<u16> {
        let mut buffer = JitterBuffer::new(JitterBufferConfig {
            target_depth: 2,
            max_size: 10,
            max_gap: 5,
        });

        for seq in [5u16, 3, 1, 4, 2, 0] {
            buffer.insert(make_packet(seq));
        }

        let mut output = Vec::new();
        while let Some(packet) = buffer.pop() {
            output.push(packet.sequence);
        }
        output.extend(buffer.drain().iter().map(|p| p.sequence));
        output
    }

    let run1 = run_sequence();
    let run2 = run_sequence();
    assert_eq!(run1, run2, "Jitter buffer should be deterministic");
}
