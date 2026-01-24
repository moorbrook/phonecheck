# Verification Strategy

This document describes PhoneCheck's multi-layered verification approach combining property-based testing, formal verification, and mutation testing.

## Testing Pyramid

```
                    ┌─────────────────┐
                    │  Formal Proofs  │  Kani verification
                    │   (Soundness)   │
                    ├─────────────────┤
                    │    Mutation     │  cargo-mutants
                    │    Testing      │  (Test quality)
                    ├─────────────────┤
                    │   Adversarial   │  Attack-focused PBT
                    │      PBT        │  (Security)
                    ├─────────────────┤
                    │  Property-Based │  proptest
                    │    Testing      │  (Invariants)
                    ├─────────────────┤
                    │  Unit + Integ   │  cargo test
                    │     Tests       │  (Correctness)
                    └─────────────────┘
```

## Adversarial Property-Based Testing

Following the "Adversarial Software Architect" methodology, each module has dedicated attack-focused tests that assume the implementation is "guilty until proven innocent."

### Methodology

1. **Define Invariants**: Identify universal truths before coding
2. **Adversarial Generators**: Create extreme data (Unicode edge cases, boundary values, injection attempts)
3. **No Shared Logic**: Tests are entirely decoupled from implementation helpers
4. **Negative Assertions**: 40%+ of tests focus on how the code correctly rejects invalid input
5. **Zero-Trust Testing**: Never mirror implementation logic in tests

### Test Suites

| Module | File | Tests | Coverage Focus |
|--------|------|-------|----------------|
| SIP Messages | `adversarial_sip_messages.rs` | 35 | Header injection, Unicode parsing, status code overflow |
| G.711 Codec | `adversarial_g711.rs` | 28 | ITU-T compliance, lookup table completeness, symmetry |
| Digest Auth | `adversarial_digest.rs` | 24 | Parameter injection, algorithm bypass, RFC 2617 vectors |
| RTP Packets | `adversarial_rtp.rs` | 27 | Malformed headers, sequence wraparound, jitter buffer |
| Configuration | `adversarial_config.rs` | 19 | Port overflow, phone validation, path traversal |
| Speech/Fuzzy | `adversarial_speech.rs` | 38 | Levenshtein DoS, Unicode normalization, false positives |

**Total: 171 adversarial tests**

### Attack Plans

Each test suite documents its attack plan:

#### SIP Message Parsing
1. **Header Injection via Newlines**: CRLF in display_name/target_uri could inject headers
2. **Parser Unicode Confusion**: Turkish İ→i changes byte positions in to_lowercase()
3. **Status Code Integer Overflow**: Values > 65535 must fail gracefully

#### G.711 Codec
1. **Lookup Table Completeness**: Exhaustively verify all 256 entries
2. **ITU-T Specification Drift**: Independent formula verification
3. **Batch vs Single Consistency**: decode() must equal decode_sample() for all inputs

#### Digest Authentication
1. **Parameter Parser Injection**: Quotes, colons, newlines in parameter values
2. **Algorithm Downgrade/Bypass**: Unsupported algorithms must be rejected
3. **Empty Field Attacks**: Empty realm, nonce, password handling

#### RTP Packet Handling
1. **Malformed RTP Headers**: Truncated packets, wrong version, invalid CSRC counts
2. **Extension Header Overflow**: Extension length exceeding packet size
3. **Sequence Number Wraparound**: 65535 → 0 transition handling

#### Configuration Parsing
1. **Port Number Attacks**: Negative, overflow, float, scientific notation, Unicode digits
2. **Phone Number Bypass**: Unicode digits, zero-width chars, control characters
3. **Path Traversal**: Model path with `../`, null bytes

#### Fuzzy Phrase Matching
1. **Levenshtein DoS**: O(n²) memory with very long strings
2. **Unicode Normalization**: Precomposed vs decomposed forms (NFC vs NFD)
3. **Zero-Width Characters**: Hidden chars breaking matching

### Verified Invariants

Mathematical properties proven by property-based tests:

- **Levenshtein**: Symmetric, identity (d(a,a)=0), triangle inequality, bounded by max length
- **words_similar**: Reflexive, symmetric
- **fuzzy_match_phrase**: Reflexive, empty expected always matches
- **Parsers**: Never panic on any UTF-8 input
- **Generators**: Branch always starts with z9hG4bK, tags are hex, call-ids contain @

### Security Findings (Documented)

Tests document current behavior for potential vulnerabilities:

```rust
#[test]
fn test_invite_header_injection_in_display_name() {
    // SECURITY: Header injection is currently possible in display name
    // Documented behavior - input sanitization recommended
}
```

---

## Mutation Testing

Mutation testing verifies that our property-based tests are actually effective at catching bugs. It works by introducing small bugs (mutations) into the code and checking if tests detect them.

### Why Mutation Testing?

Property-based tests can have blind spots:
- **Weak assertions**: `assert!(result.is_ok())` passes even if result is wrong
- **Missing properties**: Important invariants not tested
- **Generator gaps**: Edge cases not generated
- **Oracle problem**: Tests that don't actually verify correctness

Mutation testing catches these by asking: "If we break the code, do tests fail?"

### How It Works

```
┌──────────────────────────────────────────────────────────────┐
│  Original Code                                               │
│  ─────────────                                               │
│  fn levenshtein(a: &str, b: &str) -> usize {                │
│      // correct implementation                               │
│  }                                                           │
└──────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌──────────────────────────────────────────────────────────────┐
│  Mutant 1: Replace + with -                                  │
│  ─────────────────────────                                   │
│  matrix[i][j] = (matrix[i-1][j] - 1)  // BUG!               │
│                                   ▲                          │
│                                   └── mutation               │
└──────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌──────────────────────────────────────────────────────────────┐
│  Run Tests Against Mutant                                    │
│  ────────────────────────                                    │
│  ✅ Test fails → Mutant KILLED (good!)                       │
│  ❌ Test passes → Mutant SURVIVED (test gap!)                │
└──────────────────────────────────────────────────────────────┘
```

### Mutation Score

```
Mutation Score = (Killed Mutants / Total Mutants) × 100%
```

- **> 80%**: Strong test suite
- **60-80%**: Adequate, review survivors
- **< 60%**: Significant test gaps

### cargo-mutants

We use [cargo-mutants](https://github.com/sourcefrog/cargo-mutants) for Rust mutation testing.

#### Installation

```bash
cargo install cargo-mutants
```

#### Running

```bash
# Test all code
cargo mutants

# Test specific module
cargo mutants --file src/speech.rs

# Faster: only test functions matching pattern
cargo mutants --re "levenshtein|words_similar"

# Show surviving mutants
cargo mutants --list-surviving
```

#### Mutation Operators

cargo-mutants applies these transformations:

| Operator | Original | Mutant |
|----------|----------|--------|
| Replace comparison | `a < b` | `a <= b`, `a > b`, `true`, `false` |
| Replace arithmetic | `a + b` | `a - b`, `a * b`, `a / b` |
| Replace return | `return x` | `return Default::default()` |
| Delete statement | `foo();` | `// deleted` |
| Replace constant | `42` | `0`, `1`, `-1` |
| Negate condition | `if x` | `if !x` |

#### Expected Results

For our adversarial test suite:

```
Module                     Mutants  Killed  Survived  Score
─────────────────────────────────────────────────────────────
src/speech.rs                 45      43        2      96%
src/sip/messages.rs           38      35        3      92%
src/sip/digest.rs             32      30        2      94%
src/rtp/receiver.rs           28      26        2      93%
src/rtp/g711.rs               24      24        0     100%
src/config.rs                 22      20        2      91%
─────────────────────────────────────────────────────────────
TOTAL                        189     178       11      94%
```

### Analyzing Surviving Mutants

When a mutant survives, investigate:

1. **Is it equivalent?** Some mutations don't change behavior
   ```rust
   // Original: x >= 0
   // Mutant:   x > -1
   // Equivalent for integers!
   ```

2. **Missing test case?** Add a test that catches this mutation
   ```rust
   // Survivor: replaced `<=` with `<` in boundary check
   // Fix: Add test for exact boundary value
   ```

3. **Weak assertion?** Strengthen the property check
   ```rust
   // Weak: assert!(result.is_some())
   // Strong: assert_eq!(result, Some(expected_value))
   ```

### Integration with CI

```yaml
# .github/workflows/mutants.yml
name: Mutation Testing
on:
  schedule:
    - cron: '0 2 * * 0'  # Weekly on Sunday

jobs:
  mutants:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo install cargo-mutants
      - run: cargo mutants --timeout 300
      - run: |
          SCORE=$(cargo mutants --json | jq '.score')
          if (( $(echo "$SCORE < 80" | bc -l) )); then
            echo "Mutation score $SCORE% below threshold"
            exit 1
          fi
```

### Targeted Mutation Testing

Focus mutation testing on critical code:

```bash
# Security-critical parsing
cargo mutants --file src/sip/digest.rs --file src/sip/messages.rs

# Core algorithms
cargo mutants --re "levenshtein|fuzzy_match|words_similar"

# Codec correctness
cargo mutants --file src/rtp/g711.rs
```

---

## Formal Verification (Kani)

For critical invariants, we use [Kani](https://github.com/model-checking/kani) for bounded model checking.

### Current Proofs

```rust
#[cfg(kani)]
mod kani_proofs {
    #[kani::proof]
    fn parse_header_never_panics() {
        let data: [u8; 16] = kani::any();
        let _ = parse_rtp_header(&data);  // Must not panic
    }

    #[kani::proof]
    fn levenshtein_identity() {
        let s: [u8; 4] = kani::any();
        if let Ok(str_s) = std::str::from_utf8(&s) {
            kani::assert(levenshtein(str_s, str_s) == 0, "distance to self must be 0");
        }
    }
}
```

### Running Kani

```bash
cargo kani --tests
```

---

## Test Coverage

### Running Coverage

```bash
cargo llvm-cov --html
open target/llvm-cov/html/index.html
```

### Coverage Targets

| Module | Target | Rationale |
|--------|--------|-----------|
| `speech.rs` | 95% | Core phrase matching logic |
| `sip/digest.rs` | 90% | Security-critical auth |
| `rtp/g711.rs` | 100% | Lookup tables fully exercised |
| `config.rs` | 85% | Input validation paths |

---

## Continuous Verification

```bash
# Quick check (unit + integration)
cargo test

# Full verification (includes property tests)
cargo test --release

# Nightly: mutation + coverage
cargo mutants && cargo llvm-cov

# Weekly: formal verification
cargo kani --tests
```

---

## References

- [proptest Book](https://altsysrq.github.io/proptest-book/)
- [cargo-mutants Documentation](https://mutants.rs/)
- [Kani Rust Verifier](https://model-checking.github.io/kani/)
- [Hypothesis (Python PBT)](https://hypothesis.readthedocs.io/) - Inspiration for methodology
