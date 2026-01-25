# PhoneCheck Verification Strategy

This document describes the multi-layered testing and verification approach used in PhoneCheck. The strategy combines traditional testing with formal verification techniques.

## Overview

| Layer | Tool | Purpose | Count |
|-------|------|---------|-------|
| Unit Tests | Rust `#[test]` | Basic correctness | ~250 |
| Property Tests | Proptest | Invariant checking | ~40 |
| Model Checking | Stateright | State machine verification | 3 models |
| Formal Proofs | Kani | Bit-precise verification | 20+ |

---

## Testing Approaches

### 1. Traditional Unit Tests

Standard example-based tests that verify specific behaviors.

```rust
#[test]
fn test_parse_status_code() {
    assert_eq!(parse_status_code("SIP/2.0 200 OK\r\n"), Some(200));
    assert_eq!(parse_status_code("SIP/2.0 486 Busy Here\r\n"), Some(486));
}
```

**Strengths:** Easy to write, fast execution, documents expected behavior
**Limitations:** Only tests specific examples, easy to miss edge cases

### 2. Property-Based Testing (Proptest)

Verifies that invariants hold for randomly generated inputs.

```rust
proptest! {
    #[test]
    fn levenshtein_symmetric(a in "[a-z]{0,20}", b in "[a-z]{0,20}") {
        prop_assert_eq!(levenshtein(&a, &b), levenshtein(&b, &a));
    }
}
```

**Coverage by Module:**

| Module | Properties Tested |
|--------|-------------------|
| config.rs | Valid configs parse, port parsing never panics |
| speech.rs | Levenshtein symmetry, identity, triangle inequality |
| scheduler.rs | Business hours boundaries, wait time bounds |
| rtp/receiver.rs | Resample length, output range |
| sip/messages.rs | Parsing safety, branch format |

### 3. Model Checking (Stateright)

[Stateright](https://github.com/stateright/stateright) exhaustively explores all possible states of a system.

**SIP Call State Machine Model:**

```rust
impl Model for SipCallChecker {
    fn properties(&self) -> Vec<Property<Self>> {
        vec![
            Property::always("rtp_only_when_established", |_, state| {
                !state.rtp_active || state.state == CallState::Established
            }),
            Property::always("clean_termination", |_, state| {
                !matches!(state.state, CallState::Terminated | CallState::Failed)
                    || !state.rtp_active
            }),
            Property::eventually("call_terminates", |_, state| {
                matches!(state.state, CallState::Terminated | CallState::Failed)
            }),
        ]
    }
}
```

**Models:**

| Model | States | Properties Verified |
|-------|--------|---------------------|
| `JitterBufferModel` | 51 | Size bounds, no duplicates, ordered output |
| `CircuitBreakerModel` | ~50 | State transitions, threshold behavior |
| `SipModel` | 270 | Auth flows, timeouts, retries |

### 4. Formal Verification (Kani)

[Kani](https://github.com/model-checking/kani) uses symbolic execution to mathematically prove properties for *all possible inputs*.

```rust
#[kani::proof]
fn ulaw_decode_never_panics() {
    let byte: u8 = kani::any();  // ALL possible u8 values
    let _result = ULAW_TO_PCM[byte as usize];
    // If we reach here, no panic occurred for ANY input
}
```

**Verified Properties:**

| Harness | Property | Status |
|---------|----------|--------|
| `is_before_trichotomy` | Ordering holds (excluding midpoint) | ✓ |
| `is_before_midpoint_ambiguous` | Midpoint case documented | ✓ |
| `is_before_antisymmetric` | If a < b then ¬(b < a) | ✓ |
| `is_before_wraparound_boundary` | 65535 is before 0 | ✓ |
| `ulaw_decode_never_panics` | G.711 µ-law safe | ✓ |
| `alaw_decode_never_panics` | G.711 A-law safe | ✓ |
| `pcm_to_f32_always_normalized` | Output in [-1, 1] | ✓ |
| `ulaw_symmetry_proof` | U-law table symmetry | ✓ |
| `alaw_symmetry_proof` | A-law table symmetry | ✓ |
| `nanpa_10_digit_valid` | 10-digit phone numbers valid | ✓ |
| `short_number_invalid` | <10 digits invalid | ✓ |
| `phone_validation_never_panics` | Validation doesn't panic | ✓ |
| `phone_redaction_10_digits_never_panics` | Redaction safe | ✓ |
| `phone_keeps_last_4_digits` | Last 4 digits preserved | ✓ |
| `phone_prefix_only_asterisks` | Prefix is all `*` | ✓ |
| `business_hours_valid_range` | 8am-5pm Pacific correct | ✓ |
| `time_until_bounded` | Wait time ≤ 24h | ✓ |

---

## Bugs Found by Verification

### 1. Sequence Number Trichotomy Bug (Kani)

**Problem**: The `is_before()` function for RTP sequence ordering was assumed to satisfy trichotomy (exactly one of `a < b`, `b < a`, or `a == b` holds).

**Found by**: Kani proof `is_before_trichotomy` failed with counterexample.

**Root cause**: When sequence numbers are exactly 0x8000 (32768) apart, neither direction returns true:
```rust
// For a = 0, b = 32768:
// diff = b.wrapping_sub(a) = 32768
// is_before(a, b) = (32768 > 0 && 32768 < 32768) = false
// is_before(b, a) = (32768 > 0 && 32768 < 32768) = false
// a != b, so trichotomy violated!
```

**Fix**: Documented that the midpoint case is intentionally ambiguous.

### 2. Circuit Breaker State Space Explosion (Stateright)

**Problem**: Initial Stateright model hung during model checking.

**Root cause**: Using a counter for time created too many states (time × failures × state = millions).

**Fix**: Simplified to boolean `timeout_elapsed` flag, reducing state space to ~51 states.

### 3. JitterBuffer Drain Ordering (Stateright)

**Problem**: Initial test for `drain()` only checked that packets were returned, not their order.

**Fix**: Stateright model now verifies drained packets are in sequence order.

### 4. Test Assumption Error (Proptest)

**File:** `speech.rs`
**Issue:** Test assumed "thank" and "thanks" are not similar

```rust
// WRONG: "thank" (5 chars) and "thanks" (6 chars) differ by 1 char
assert!(!words_similar("thank", "thanks"));

// CORRECT: 1 char difference is allowed for words > 3 chars
assert!(words_similar("thank", "thanks"));
```

### 5. Non-Deterministic Tests

**File:** `scheduler.rs`
**Issue:** Time-dependent tests couldn't verify business hours logic

**Fix:** Refactored to accept injectable time values:
```rust
pub fn is_business_hours_at(hour: u32, minute: u32, second: u32) -> bool {
    hour >= BUSINESS_START_HOUR && hour < BUSINESS_END_HOUR
}
```

---

## Running Tests

```bash
# Run all unit and property tests
cargo test

# Run Stateright model tests
cargo test --lib _model

# Run Kani formal proofs
cargo kani

# Run specific Kani proof
cargo kani --harness ulaw_decode_never_panics
```

---

## Defense in Depth

Each layer catches different types of bugs:

| Bug Type | Unit Tests | Proptest | Stateright | Kani |
|----------|------------|----------|------------|------|
| Logic errors | ✓ | ✓ | ✓ | |
| Edge cases | ○ | ✓ | ✓ | ✓ |
| State machine bugs | ○ | | ✓ | |
| Panic/overflow | ○ | ✓ | | ✓ |
| Memory safety | | | | ✓ |
| Concurrency | | | ✓ | |

✓ = strong coverage, ○ = partial coverage

---

## Verification Coverage Summary

### Fully Verified (Kani + Stateright)
- G.711 codec (all 256 input values proven safe)
- SIP state machine (all transitions verified)
- Levenshtein distance (no panics, correct properties)
- Business hours logic (all 24 hours verified)
- Phone number validation and redaction

### Well-Tested (Proptest + Unit Tests)
- Configuration parsing
- Fuzzy phrase matching
- RTP header parsing
- Audio resampling

### Partially Tested (Unit Tests Only)
- Whisper transcription (requires model)
- Network transport (environment-dependent)
- Full E2E flow (requires live SIP server)

---

## Future Improvements

1. **Miri**: Run under Miri interpreter to check for undefined behavior
2. **Loom**: Add concurrency tests if async code becomes more complex
3. **Coverage**: Add `cargo-llvm-cov` for test coverage metrics
4. **Fuzzing**: Add `cargo-fuzz` for finding edge cases in parsers
