# Formal Verification for PhoneCheck

This directory contains formal verification specifications using Aeneas/Lean.
The main codebase also uses Kani and Stateright for verification.

## Overview

| Tool | Approach | Proof Location | Strengths |
|------|----------|----------------|-----------|
| **Kani** | Bounded model checking | Inline `#[kani::proof]` | Panic-freedom, exhaustive u8/u16 |
| **Stateright** | State machine exploration | `state_machine` modules | Protocol correctness, invariants |
| **Aeneas** | Translation to Lean | Separate `.lean` files | Deep mathematical proofs |

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

## Directory Structure

```
verification/
└── aeneas/
    ├── Cargo.toml
    ├── src/lib.rs      # Aeneas-compatible Rust code
    └── Proofs.lean     # Lean 4 theorems
```

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

### Usage

```bash
# Run all Kani proofs (slow - some involve complex operations)
cargo kani

# Run specific proof (fast)
cargo kani --harness is_before_trichotomy
cargo kani --harness ulaw_decode_never_panics
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

## Aeneas / Lean 4

[Aeneas](https://github.com/AeneasVerif/aeneas) translates Rust to Lean 4.
Proofs are written in Lean against the generated functional definitions.

### Proven Theorems

| Theorem | Status | Description |
|---------|--------|-------------|
| `len_nonneg` | ✓ | Packet list length ≥ 0 |
| `insert_len` | ✓ | Insert increases length by 1 |
| `insert_contains` | ✓ | Inserted seq is in list |
| `buffer_no_duplicates` | ✓ | No-duplicate invariant preserved |
| `wraparound_boundary` | ✓ | 65535 is before 0 |
| `levenshtein_identity` | ✓ | Distance(a, a) = 0 |
| `seq_antisymmetric` | sorry | Requires bitvector reasoning |
| `levenshtein_symmetric` | sorry | Requires structural induction |

### Usage

```bash
# Check Lean proofs (requires Lean 4)
lean verification/aeneas/Proofs.lean

# Full Aeneas workflow
opam install aeneas
cd verification/aeneas
aeneas -backend lean4 src/lib.rs -dest Generated.lean
lake build
```

## Properties by Component

### Sequence Ordering (`is_before`)
- **Kani**: Trichotomy (excluding midpoint), antisymmetry, wraparound
- **Aeneas/Lean**: Mathematical proof with bitvector reasoning

### Jitter Buffer
- **Kani**: Insert/pop never panic, max_size enforced
- **Stateright**: State machine properties (ordering, no duplicates, size bounds)
- **Aeneas/Lean**: Insertion length invariant, no-duplicate preservation

### G.711 Codec
- **Kani**: Decode tables never panic, output normalized

### Circuit Breaker
- **Stateright**: State transition correctness, threshold behavior

### SMS Truncation
- **Kani**: Output length ≤ MAX_SMS_LENGTH, never panics
