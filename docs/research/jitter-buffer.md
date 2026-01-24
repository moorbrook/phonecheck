# Jitter Buffer Research

## PJSIP Implementation (Reference)

Source: [PJSIP Jitter Buffer Documentation](https://docs.pjsip.org/en/latest/specific-guides/audio/jitter_buffer.html)

### Overview
PJMEDIA implements an adaptive jitter buffer that handles both network jitter and sound device timing variations.

### Key Concepts

#### Adaptive vs Fixed Mode
- **Adaptive (default)**: Buffer adjusts prefetch based on observed jitter
- **Fixed**: Set via `pjmedia_jbuf_set_fixed()` for predictable latency

#### Optimal Latency Calculation
- Optimal latency = minimum buffering to handle current jitter
- Latency should be approximately equal to burst level
- Example: burst level 3 with 20ms frames → 60ms latency

#### Burst Level
- Measures consecutive add/remove operations
- Accounts for both network and sound device jitter
- Latency should never be shorter than burst level

### Discard Algorithms

#### Progressive Discard (Default)
- Drops frames at varying rates based on overflow magnitude
- Configurable parameters:
  - `PJMEDIA_JBUF_PRO_DISC_MIN_BURST`
  - `PJMEDIA_JBUF_PRO_DISC_MAX_BURST`
  - `PJMEDIA_JBUF_PRO_DISC_T1`, `PJMEDIA_JBUF_PRO_DISC_T2`

#### Conservative/Static Discard
- Optimal latency = 2x burst level
- Fixed discard rate (default: every 200ms)
- More stable but higher latency

### Prefetch Buffering
- Initial buffering before returning frames
- Configurable min/max prefetch values
- Activates each time buffer empties

### Edge Case Handling
- Duplicate/old frames: automatically handled
- Sequence number jumps: triggers restart
- DTX (silence suppression): handled without false restarts

## Recommended Implementation for PhoneCheck

### Simple Approach (Start Here)
Given our use case (short calls, reliable networks):

1. **Fixed buffer**: 50-100ms (4-8 packets at 20ms/packet)
2. **Sequence-based reordering**: Hold packets until sequence gaps filled or timeout
3. **Late packet policy**: Discard if >100ms late

### Data Structure
```rust
struct JitterBuffer {
    buffer: BTreeMap<u16, RtpPacket>,  // Keyed by sequence number
    next_seq: u16,                      // Next expected sequence
    max_delay_ms: u32,                  // Maximum buffering delay
}
```

### Algorithm
1. On packet arrival: insert into BTreeMap by sequence
2. On read request:
   - If next_seq packet available: return it, increment next_seq
   - If gap exists and oldest packet is old enough: skip gap, return oldest
   - If buffer empty: return None (underflow)

### Metrics to Track
- Packets received
- Packets reordered (arrived out of sequence)
- Packets discarded (too late)
- Buffer underflows

## Sources
- [PJSIP Jitter Buffer Docs](https://docs.pjsip.org/en/latest/specific-guides/audio/jitter_buffer.html)
- [PJSIP API Reference](https://docs.pjsip.org/en/latest/api/generated/pjmedia/group/group__PJMED__JBUF.html)
