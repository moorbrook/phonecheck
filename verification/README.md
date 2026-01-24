# Formal Verification for PhoneCheck

This directory contains formal verification specifications using three different tools:
Verus, Flux, and Aeneas/Lean. Each approach has different tradeoffs.

## Overview

| Tool | Approach | Proof Location | Strengths |
|------|----------|----------------|-----------|
| **Verus** | SMT-based | Inline Rust macros | Full functional correctness, automation |
| **Flux** | Refinement types | Function signatures | Lightweight, compile-time checks |
| **Aeneas** | Translation to Lean | Separate `.lean` files | Deep mathematical proofs, theorem prover |

## Directory Structure

```
verification/
├── verus/
│   ├── Cargo.toml
│   └── src/lib.rs      # Verus specs with requires/ensures
├── flux/
│   └── lib.rs          # Flux refinement type annotations
└── aeneas/
    ├── Cargo.toml
    ├── src/lib.rs      # Aeneas-compatible Rust code
    └── Proofs.lean     # Lean 4 theorems
```

## Verus

[Verus](https://github.com/verus-lang/verus) uses SMT solvers to verify Rust code.
Specifications are written in Rust using the `verus!` macro.

### Properties Verified

- `resample_8k_to_16k`: Output length is exactly 2x input length
- `seq_is_before`: Trichotomy (exactly one of <, >, == holds)
- `seq_is_before`: Antisymmetry (if a < b then not b < a)
- `truncate_sms_message`: Output bounded by MAX_SMS_LENGTH

### Usage

```bash
# Install Verus
git clone https://github.com/verus-lang/verus
cd verus && ./tools/get-z3.sh && cargo build --release

# Verify
cd verification/verus
verus src/lib.rs
```

## Flux

[Flux](https://github.com/flux-rs/flux) adds refinement types to Rust.
Types are annotated with logical predicates checked at compile time.

### Properties Verified

- `truncate_sms_message`: Return type `String{v: len(v) <= 160}`
- `parse_port`: Return type `Option<u16{v: v > 0}>`
- `resample_8k_to_16k`: Length relationship `Vec<f32>[n * 2]`
- `levenshtein`: Non-negative result `usize{v: v >= 0}`

### Usage

```bash
# Install Flux
cargo install flux-rs

# Verify
cd verification/flux
flux-rs check lib.rs
```

## Aeneas / Lean 4

[Aeneas](https://github.com/AeneasVerif/aeneas) translates Rust to Lean 4.
Proofs are written in Lean against the generated functional definitions.

### Properties Verified

- `PacketList.len`: Always non-negative
- `PacketList.insert`: Increases length by exactly 1
- `seq_is_before`: Antisymmetry and trichotomy
- `JitterBuffer`: No duplicate packets invariant
- Wraparound boundary: 65535 is before 0

### Usage

```bash
# Install Aeneas
opam install aeneas

# Translate Rust to Lean
cd verification/aeneas
aeneas -backend lean4 src/lib.rs -dest Generated.lean

# Install Lean 4
curl https://raw.githubusercontent.com/leanprover/elan/master/elan-init.sh -sSf | sh

# Verify proofs
lake build
```

## Comparison with Existing Verification

The main codebase already uses:

- **Kani**: Bounded model checking for panic-freedom
- **Stateright**: State machine exploration for protocols

These new tools complement Kani and Stateright:

| Property | Kani | Stateright | Verus | Flux | Aeneas |
|----------|------|------------|-------|------|--------|
| Panic-freedom | ✓ | | ✓ | | |
| Protocol correctness | | ✓ | | | ✓ |
| Functional correctness | | | ✓ | | ✓ |
| Type refinements | | | | ✓ | |
| Mathematical proofs | | | | | ✓ |

## Properties Covered

### Resampling (`resample_8k_to_16k`)
- **Kani**: Never panics
- **Verus**: Output length = 2 * input length
- **Flux**: Dependent length type annotation

### Sequence Ordering (`is_before`)
- **Kani**: Trichotomy holds for all u16 pairs
- **Verus**: Formal proof of trichotomy and antisymmetry
- **Aeneas/Lean**: Mathematical proof with bitvector reasoning

### SMS Truncation
- **Kani**: Output length ≤ MAX_SMS_LENGTH
- **Verus**: Precondition/postcondition contract
- **Flux**: Refinement type `String{v: len(v) <= 160}`

### Jitter Buffer
- **Kani**: Insert/pop never panic, max_size enforced
- **Stateright**: State machine properties (ordering, no duplicates)
- **Aeneas/Lean**: Full correctness proof in Lean

### Circuit Breaker
- **Stateright**: State transition correctness
- **Flux**: Failure count bounds, threshold relationships
