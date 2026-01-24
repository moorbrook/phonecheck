//! Flux Refinement Types for PhoneCheck
//!
//! This module contains Flux refinement type annotations for critical functions.
//! To verify, install Flux and run: flux-rs check verification/flux/lib.rs
//!
//! Flux installation: https://github.com/flux-rs/flux

// ============================================================================
// SMS TRUNCATION: Output length bounded by MAX_SMS_LENGTH
// ============================================================================

const MAX_SMS_LENGTH: usize = 160;

/// Truncate SMS message to fit within MAX_SMS_LENGTH
///
/// Flux signature ensures output length <= MAX_SMS_LENGTH
#[flux::sig(fn(message: &str) -> String{v: v.len() <= MAX_SMS_LENGTH})]
pub fn truncate_sms_message(message: &str) -> String {
    if message.len() <= MAX_SMS_LENGTH {
        message.to_string()
    } else {
        // Leave room for "..." (3 chars)
        let target_len = MAX_SMS_LENGTH - 3;

        // Find a valid UTF-8 char boundary at or before target_len
        let mut truncate_at = target_len;
        while truncate_at > 0 && !message.is_char_boundary(truncate_at) {
            truncate_at -= 1;
        }

        if truncate_at == 0 {
            // Edge case: couldn't find a valid boundary
            return "...".to_string();
        }

        let truncated = &message[..truncate_at];

        // Try to truncate at word boundary for cleaner output
        let truncated = truncated
            .rfind(' ')
            .filter(|&pos| pos > truncate_at / 2)
            .map(|pos| &truncated[..pos])
            .unwrap_or(truncated);

        format!("{}...", truncated)
    }
}

// ============================================================================
// PORT PARSING: Valid port range refinement
// ============================================================================

/// A valid network port (1-65535)
/// Flux refinement: value is in valid port range
#[flux::alias(type ValidPort = u16{v: v > 0})]
pub type ValidPort = u16;

/// Parse a port string into a ValidPort
///
/// Flux signature ensures output is a valid port or None
#[flux::sig(fn(s: &str) -> Option<u16{v: v > 0 && v <= 65535}>)]
pub fn parse_port(s: &str) -> Option<u16> {
    let port: u16 = s.parse().ok()?;
    if port == 0 {
        None
    } else {
        Some(port)
    }
}

/// Parse port with default value
///
/// Flux signature ensures output is always a valid port
#[flux::sig(fn(s: &str, default: u16{v: v > 0}) -> u16{v: v > 0})]
pub fn parse_port_or_default(s: &str, default: u16) -> u16 {
    parse_port(s).unwrap_or(default)
}

// ============================================================================
// RESAMPLING: Length relationships
// ============================================================================

/// Resample 8kHz audio to 16kHz (doubles the sample count)
///
/// Flux signature expresses the length relationship:
/// - Empty input produces empty output
/// - Non-empty input produces output of length 2*n
#[flux::sig(fn(samples: &[f32][n]) -> Vec<f32>[if n == 0 { 0 } else { n * 2 }])]
pub fn resample_8k_to_16k(samples: &[f32]) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }

    let mut output = Vec::with_capacity(samples.len() * 2);

    for i in 0..samples.len() {
        output.push(samples[i]);

        if i + 1 < samples.len() {
            // Interpolate between current and next sample
            let interpolated = (samples[i] + samples[i + 1]) / 2.0;
            output.push(interpolated);
        } else {
            // Last sample: duplicate
            output.push(samples[i]);
        }
    }

    output
}

// ============================================================================
// JITTER BUFFER: Sequence number refinements
// ============================================================================

/// RTP sequence number (full u16 range, but with ordering semantics)
#[flux::alias(type SeqNum = u16)]
pub type SeqNum = u16;

/// Buffer size that respects max_size configuration
#[flux::alias(type BoundedSize[max: int] = usize{v: v <= max})]
pub type BoundedSize = usize;

/// Jitter buffer configuration with refined types
pub struct JitterBufferConfig {
    /// Target buffer depth (packets held before output)
    #[flux::field(u16{v: v >= 0})]
    pub target_depth: u16,

    /// Maximum buffer size (bounded)
    #[flux::field(u16{v: v > 0 && v <= 1000})]
    pub max_size: u16,

    /// Maximum sequence gap before skipping
    #[flux::field(u16{v: v > 0})]
    pub max_gap: u16,
}

// ============================================================================
// CIRCUIT BREAKER: State transitions with refinements
// ============================================================================

/// Failure count (bounded by threshold)
#[flux::alias(type FailureCount[threshold: int] = u32{v: v >= 0})]
pub type FailureCount = u32;

/// Record a failure with bounded increment
///
/// Flux ensures we don't overflow
#[flux::sig(fn(current: u32{v: v < u32::MAX}) -> u32{v: v == current + 1})]
pub fn increment_failures(current: u32) -> u32 {
    current + 1
}

/// Check if circuit should open
///
/// Flux tracks the relationship between failures and threshold
#[flux::sig(fn(failures: u32, threshold: u32{v: v > 0}) -> bool)]
pub fn should_open_circuit(failures: u32, threshold: u32) -> bool {
    failures >= threshold
}

// ============================================================================
// LEVENSHTEIN DISTANCE: Non-negative result
// ============================================================================

/// Levenshtein edit distance between two strings
///
/// Flux signature ensures result is non-negative
#[flux::sig(fn(a: &str, b: &str) -> usize{v: v >= 0})]
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();

    if a_chars.is_empty() {
        return b_chars.len();
    }
    if b_chars.is_empty() {
        return a_chars.len();
    }

    let mut prev_row: Vec<usize> = (0..=b_chars.len()).collect();
    let mut curr_row: Vec<usize> = vec![0; b_chars.len() + 1];

    for i in 1..=a_chars.len() {
        curr_row[0] = i;

        for j in 1..=b_chars.len() {
            let cost = if a_chars[i - 1] == b_chars[j - 1] { 0 } else { 1 };

            curr_row[j] = (prev_row[j] + 1)
                .min(curr_row[j - 1] + 1)
                .min(prev_row[j - 1] + cost);
        }

        std::mem::swap(&mut prev_row, &mut curr_row);
    }

    prev_row[b_chars.len()]
}

// ============================================================================
// TESTS (standard Rust tests, Flux verifies at compile time)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_short() {
        let msg = "Hello";
        let result = truncate_sms_message(msg);
        assert_eq!(result, "Hello");
        assert!(result.len() <= MAX_SMS_LENGTH);
    }

    #[test]
    fn test_truncate_long() {
        let msg = "a".repeat(200);
        let result = truncate_sms_message(&msg);
        assert!(result.len() <= MAX_SMS_LENGTH);
    }

    #[test]
    fn test_parse_port_valid() {
        assert_eq!(parse_port("8080"), Some(8080));
        assert_eq!(parse_port("443"), Some(443));
    }

    #[test]
    fn test_parse_port_invalid() {
        assert_eq!(parse_port("0"), None);
        assert_eq!(parse_port("abc"), None);
    }

    #[test]
    fn test_resample_empty() {
        let result = resample_8k_to_16k(&[]);
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_resample_length() {
        let samples = vec![1.0, 2.0, 3.0];
        let result = resample_8k_to_16k(&samples);
        assert_eq!(result.len(), 6); // 3 * 2
    }

    #[test]
    fn test_levenshtein_same() {
        assert_eq!(levenshtein("hello", "hello"), 0);
    }

    #[test]
    fn test_levenshtein_different() {
        assert_eq!(levenshtein("hello", "hallo"), 1);
        assert_eq!(levenshtein("", "abc"), 3);
    }
}
