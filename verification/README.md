# Formal Verification for PhoneCheck

This directory contains documentation for formal verification.
The main codebase uses Kani and Stateright for verification.

## Overview

| Tool | Approach | Proof Location | Strengths |
|------|----------|----------------|-----------|
| **Kani** | Bounded model checking | Inline `#[kani::proof]` | Panic-freedom, exhaustive u8/u16 |
| **Stateright** | State machine exploration | `state_machine` modules | Protocol correctness, invariants |

## Bugs Found by Verification

### 1. Sequence Number Trichotomy Bug (Kani)

**Problem**: The `is_before()` function for RTP sequence ordering was assumed to satisfy trichotomy (exactly one of `a < b`, `b < a`, or `a == b` holds for all pairs).

**Found by**: Kani proof `is_before_trichotomy` failed with counterexample.

**Root cause**: When sequence numbers are exactly 0x8000 (32768) apart, neither direction returns true:
```rust
// For a = 0, b = 32768:
// diff = b.wrapping_sub(a) = 32768
// is_before(a, b) = (32768 > 0 && 32768 < 32768) = false
// is_before(b, a) = (32768 > 0 && 32768 < 32768) = false
// a != b, so trichotomy violated!
```

**Fix**: Documented that the midpoint case is intentionally ambiguous (neither packet is clearly "before" the other when exactly half the sequence space apart). Added separate `is_before_midpoint_ambiguous` proof.

**Impact**: Prevents incorrect assumptions in jitter buffer ordering logic when packets are far apart in sequence space.

### 2. Circuit Breaker State Space Explosion (Stateright)

**Problem**: Initial Stateright model for CircuitBreaker hung during model checking.

**Found by**: Stateright model checker never completed.

**Root cause**: Using a counter for time created too many states (time × failures × state = millions of states).

**Fix**: Simplified to boolean `timeout_elapsed` flag, reducing state space to ~51 states while preserving safety properties.

### 3. JitterBuffer Drain Ordering (Stateright)

**Problem**: Initial property-based test for `drain()` only checked that packets were returned, not their order.

**Found by**: Adversarial code review of PBT tests.

**Fix**: Stateright model now verifies that drained packets are in sequence order and match inserted packets.

## Kani (Main Codebase)

[Kani](https://github.com/model-checking/kani) performs bounded model checking on Rust code.
Proofs are in the main `src/` directory with `#[kani::proof]` attributes.

### Verified Properties

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
| `nanpa_10_digit_valid` | 10-digit NANPA numbers valid | ✓ |
| `e164_na_11_digit_valid` | 11-digit E.164 NA valid | ✓ |
| `e164_intl_valid` | E.164 international valid | ✓ |
| `short_number_invalid` | <10 digits invalid | ✓ |
| `phone_validation_never_panics` | Validation doesn't panic | ✓ |
| `phone_redaction_10_digits_never_panics` | Redaction safe | ✓ |
| `phone_length_preserved_5` | Redacted length = input length | ✓ |
| `phone_short_fully_masked` | ≤4 digits fully masked | ✓ |
| `phone_keeps_last_4_digits` | Last 4 digits preserved | ✓ |
| `phone_prefix_only_asterisks` | Prefix is all `*` | ✓ |
| `phone_pii_not_leaked_in_prefix` | No digit leakage in prefix | ✓ |

### Usage

```bash
# Run all Kani proofs (slow - some involve complex operations)
cargo kani

# Run specific proof (fast)
cargo kani --harness is_before_trichotomy
cargo kani --harness ulaw_decode_never_panics
cargo kani --harness phone_keeps_last_4_digits
```

## Stateright (Main Codebase)

[Stateright](https://github.com/stateright/stateright) explores state machines exhaustively.

### Models

| Model | States | Properties Verified |
|-------|--------|---------------------|
| `JitterBufferModel` | 51 | Size bounds, no duplicates, ordered output |
| `CircuitBreakerModel` | ~50 | State transitions, threshold behavior |
| `SipModel` | 270 | Auth flows, timeouts, retries |

### Usage

```bash
# Run all Stateright model tests
cargo test --lib _model

# Run specific model
cargo test --lib jitter_buffer_model
cargo test --lib circuit_breaker_model
cargo test --lib sip_model
```

## Properties by Component

### Sequence Ordering (`is_before`)
- **Kani**: Trichotomy (excluding midpoint), antisymmetry, wraparound

### Jitter Buffer
- **Kani**: Insert/pop never panic, max_size enforced
- **Stateright**: State machine properties (ordering, no duplicates, size bounds)

### G.711 Codec
- **Kani**: Decode tables never panic, output normalized, symmetry

### Phone Validation
- **Kani**: NANPA/E.164 format acceptance, short number rejection, never panics

### Phone Redaction
- **Kani**: Length preserved, last 4 digits visible, prefix masked, PII protection

### Circuit Breaker
- **Stateright**: State transition correctness, threshold behavior

### SMS Truncation
- **Kani**: Output length ≤ MAX_SMS_LENGTH, never panics
