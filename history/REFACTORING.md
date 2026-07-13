# PhoneCheck Refactoring Opportunities

This document identifies refactoring opportunities in the PhoneCheck codebase with the goal of improving maintainability, reducing duplication, and increasing type safety.

## Completed Refactorings

### ✅ 1. Redundant CallResult Construction

**Status**: Completed

**Changes**:
- Added builder methods to `CallResult` in `src/sip/client.rs`:
  - `success(audio_samples, audio_received)` - for successful calls
  - `failed(error)` - for failed calls without status code
  - `failed_with_status(status, error)` - for failed calls with SIP status code
- Removed 5+ duplicated `CallResult` constructions

**Impact**: Reduced code duplication, easier maintenance.

---

### ✅ 2. Duplicate Cancellation Token Handling

**Status**: Completed

**Changes**:
- Updated `SipClient::make_test_call()` to use `CancellationToken::new()` directly
- `RtpReceiver::receive_for()` already had correct pattern

**Impact**: Consistent pattern across codebase.

---

### ✅ 3. Duplicate URI Construction in SIP Client

**Status**: Completed

**Changes**:
- Added cached fields to `SipClient` struct: `from_uri`, `target_uri`, `display_name`
- URIs are computed once in `new()` and reused across all calls
- Replaced 8+ URI string allocations with field references

**Impact**: Reduced string allocations in hot path, improved performance.

---

### ✅ 4. Duplicate Duration Calculations

**Status**: Completed

**Changes**:
- Added helpers to `src/rtp/mod.rs`:
  - `duration_to_samples(duration_ms)` - convert ms to samples
  - `samples_to_duration_ms(samples)` - convert samples to ms
  - `WHISPER_SAMPLE_RATE` constant (16000 Hz)
- Replaced inline calculations with helper functions
- Added tests for conversion functions

**Impact**: Consistent conversion logic, easier to change sample rate.

---

### ✅ 5. Config Cloning

**Status**: Completed

**Changes**:
- Changed `SipClient` to take `Arc<Config>` instead of owning `Config`
- `SipClient::new` now takes `Arc<Config>`
- `Config` is wrapped in `Arc` early in `main()` before any cloning
- All config access goes through `Arc` references

**Impact**: Clear ownership semantics, no unnecessary config clones.

---

### ✅ 6. main.rs is Too Large

**Status**: Completed

**Changes**:
- Created `src/cli.rs` - argument parsing (166 lines)
- Created `src/orchestrator.rs` - check coordination (146 lines)
- Reduced `main.rs` from ~300 lines to 135 lines
- Added comprehensive tests for CLI parsing

**Impact**: Better separation of concerns, easier to navigate and test.

---

### ✅ 7. Stringly-Typed Configuration

**Status**: Completed

**Changes**:
- Added `ConfigKey` enum in `src/config.rs` with all configuration keys
- Each key has methods:
  - `env_var()` - returns environment variable name
  - `is_required()` - checks if key has no default
  - `default_value()` - returns default value if any
- `Config::from_getter()` uses `ConfigKey` instead of string literals
- Compile-time safety for configuration keys
- Added tests for ConfigKey methods

**Impact**: No typos in config keys, better documentation.

---

## Outstanding Refactorings

These items from the original REFACTORING.md document were intentionally not implemented:

### 🚫 String Allocations in Hot Paths
**Rationale**: Display name ("PhoneCheck") is constant, but it's only allocated once per SipClient (in constructor). The URIs are now cached, so the hot path allocation issue is resolved. Further optimization would be micro-optimization with minimal benefit.

### 🚫 Magic Numbers Extraction
**Rationale**: As documented in REFACTORING.md, some magic numbers carry important context and shouldn't be extracted:
- `16000` (sample rate) - clear from type and context
- `20ms` (RTP packetization) - standard value, self-documenting
- These values are now encapsulated in named constants where it makes sense (`WHISPER_SAMPLE_RATE`)

---

## Module Structure Guidelines

The following conventions have been established:

### Single-File Modules
Use a single `.rs` file for modules that are:
- Less than ~200 lines
- Self-contained (no complex sub-modules)
- Examples: `config.rs`, `health.rs`, `notify.rs`, `embedding.rs`

### Multi-File Modules  
Use a directory with `mod.rs` for modules that are:
- Larger than ~200 lines
- Have logically separate concerns
- Benefit from splitting into multiple files
- Examples: `sip/`, `rtp/` (multi-file)

### New Module Creation
When adding a new module:
1. Keep under ~200 lines if possible
2. If growing larger, consider splitting into sub-modules
3. Add comprehensive module-level documentation
4. Place tests at bottom of file for small modules, or in separate `tests/` directory for integration tests

---

## Summary Statistics

### Code Reduction
- `main.rs`: ~300 → 135 lines (55% reduction)
- `sip/client.rs`: ~40 lines of duplicated CallResult code removed
- 8+ URI string allocations per call eliminated

### Test Coverage
- All existing tests pass: 227 tests
- New CLI tests: 8 tests added
- New duration helper tests: 3 tests added
- New ConfigKey tests: 3 tests added

### Compilation
- All changes compile without warnings
- Release build successful
- No breaking changes to public APIs

---

## Future Improvements (Not Prioritized)

These items remain for future consideration:
1. **Optional State Consistency** - Currently no issues; would add complexity for minimal gain
2. **Property-Based Tests Expansion** - Add more property tests where beneficial
3. **Documentation Improvements** - Consistent doc comment style across modules
4. **`#[must_use]` Attributes** - Add to important `Result`-returning functions
