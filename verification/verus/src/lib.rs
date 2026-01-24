//! Verus Formal Verification for PhoneCheck
//!
//! This module contains Verus specifications and proofs for critical functions.
//! To verify, install Verus and run: verus verification/verus/src/lib.rs
//!
//! Verus installation: https://github.com/verus-lang/verus

use vstd::prelude::*;

verus! {

// ============================================================================
// RESAMPLING: Output length is exactly 2x input length
// ============================================================================

/// Specification: defines the expected output length for resampling
#[spec]
pub fn resample_output_len(input_len: nat) -> nat {
    if input_len == 0 {
        0
    } else {
        input_len * 2
    }
}

/// Proof: resampling length property
#[proof]
pub fn lemma_resample_length(input_len: nat)
    ensures
        resample_output_len(input_len) == if input_len == 0 { 0 } else { input_len * 2 },
{
    // Trivially true by definition
}

/// Proof: resampling preserves non-negativity
#[proof]
pub fn lemma_resample_nonnegative(input_len: nat)
    ensures
        resample_output_len(input_len) >= 0,
{
    // nat is always non-negative
}

/// Executable resampling function with verified length
/// Note: This is a simplified version for verification purposes
#[exec]
pub fn resample_8k_to_16k_verified(samples: &Vec<f32>) -> (result: Vec<f32>)
    ensures
        result.len() == resample_output_len(samples.len() as nat) as usize,
{
    if samples.len() == 0 {
        return Vec::new();
    }

    let mut output = Vec::with_capacity(samples.len() * 2);

    let mut i: usize = 0;
    while i < samples.len()
        invariant
            i <= samples.len(),
            output.len() == i * 2,
    {
        let sample = samples[i];
        output.push(sample);

        // Interpolate: average with next sample, or duplicate if last
        if i + 1 < samples.len() {
            let next = samples[i + 1];
            output.push((sample + next) / 2.0);
        } else {
            output.push(sample);
        }

        i = i + 1;
    }

    output
}

// ============================================================================
// SEQUENCE NUMBER WRAPAROUND: Trichotomy property
// ============================================================================

/// Specification: is_before for RTP sequence numbers with wraparound
/// Returns true if seq_a is "before" seq_b in the circular sequence space
#[spec]
pub fn seq_is_before(seq_a: u16, seq_b: u16) -> bool {
    let diff = seq_b.wrapping_sub(seq_a);
    diff > 0 && diff < 0x8000
}

/// Specification: is_after (inverse of is_before)
#[spec]
pub fn seq_is_after(seq_a: u16, seq_b: u16) -> bool {
    seq_is_before(seq_b, seq_a)
}

/// Specification: sequence equality
#[spec]
pub fn seq_equal(seq_a: u16, seq_b: u16) -> bool {
    seq_a == seq_b
}

/// Proof: Trichotomy - exactly one of <, >, == holds for any two sequence numbers
#[proof]
pub fn lemma_seq_trichotomy(seq_a: u16, seq_b: u16)
    ensures
        // Exactly one is true
        (seq_is_before(seq_a, seq_b) as int)
            + (seq_is_after(seq_a, seq_b) as int)
            + (seq_equal(seq_a, seq_b) as int) == 1,
{
    let diff_ab = seq_b.wrapping_sub(seq_a);
    let diff_ba = seq_a.wrapping_sub(seq_b);

    // Case analysis on diff_ab
    if seq_a == seq_b {
        // Equal case: diff_ab == 0
        assert(diff_ab == 0);
        assert(!seq_is_before(seq_a, seq_b));
        assert(!seq_is_after(seq_a, seq_b));
        assert(seq_equal(seq_a, seq_b));
    } else if diff_ab > 0 && diff_ab < 0x8000 {
        // a is before b
        assert(seq_is_before(seq_a, seq_b));
        // diff_ba = 2^16 - diff_ab, which is >= 0x8000
        assert(diff_ba >= 0x8000);
        assert(!seq_is_after(seq_a, seq_b));
        assert(!seq_equal(seq_a, seq_b));
    } else {
        // a is after b (diff_ab >= 0x8000)
        assert(!seq_is_before(seq_a, seq_b));
        // diff_ba < 0x8000
        assert(diff_ba > 0 && diff_ba < 0x8000);
        assert(seq_is_after(seq_a, seq_b));
        assert(!seq_equal(seq_a, seq_b));
    }
}

/// Proof: Antisymmetry - if a < b then not b < a
#[proof]
pub fn lemma_seq_antisymmetric(seq_a: u16, seq_b: u16)
    requires
        seq_is_before(seq_a, seq_b),
    ensures
        !seq_is_before(seq_b, seq_a),
{
    // From trichotomy, if a < b, then exactly one of the three is true,
    // so b < a must be false
    lemma_seq_trichotomy(seq_a, seq_b);
}

/// Proof: Wraparound boundary - 65535 is before 0
#[proof]
pub fn lemma_wraparound_boundary()
    ensures
        seq_is_before(65535u16, 0u16),
        !seq_is_before(0u16, 65535u16),
{
    // 0.wrapping_sub(65535) = 1, which is > 0 and < 0x8000
    assert(0u16.wrapping_sub(65535u16) == 1u16);
    assert(seq_is_before(65535u16, 0u16));

    // 65535.wrapping_sub(0) = 65535, which is >= 0x8000
    assert(65535u16.wrapping_sub(0u16) == 65535u16);
    assert(!seq_is_before(0u16, 65535u16));
}

/// Executable is_before with verified contract
#[exec]
pub fn is_before_verified(seq_a: u16, seq_b: u16) -> (result: bool)
    ensures
        result == seq_is_before(seq_a, seq_b),
{
    let diff = seq_b.wrapping_sub(seq_a);
    diff > 0 && diff < 0x8000
}

// ============================================================================
// SMS TRUNCATION: Output bounded by MAX_SMS_LENGTH
// ============================================================================

/// Constant for SMS length limit
pub const MAX_SMS_LENGTH: usize = 160;

/// Specification: truncated length is at most MAX_SMS_LENGTH
#[spec]
pub fn truncate_bounded(input_len: nat, max_len: nat) -> nat {
    if input_len <= max_len {
        input_len
    } else {
        max_len
    }
}

/// Proof: truncation is bounded
#[proof]
pub fn lemma_truncate_bounded(input_len: nat, max_len: nat)
    ensures
        truncate_bounded(input_len, max_len) <= max_len,
{
    // Direct from definition
}

/// Proof: short strings are preserved
#[proof]
pub fn lemma_truncate_preserves_short(input_len: nat, max_len: nat)
    requires
        input_len <= max_len,
    ensures
        truncate_bounded(input_len, max_len) == input_len,
{
    // Direct from definition
}

} // verus!
