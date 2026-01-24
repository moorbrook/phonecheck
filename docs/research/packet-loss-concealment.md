# RTP Packet Loss Concealment (PLC) Research

## Overview

[Packet loss concealment](https://en.wikipedia.org/wiki/Packet_loss_concealment) masks the effects of lost packets in VoIP communications. Since ARQ (retransmission) isn't feasible for real-time audio, the receiver must cope with loss.

## PLC Techniques

### 1. Zero Insertion (Simplest)
Replace lost frames with silence.
- **Pros**: Simple, no computation
- **Cons**: Audible gaps, worst quality

### 2. Waveform Substitution (G.711 Appendix I)
Repeat the last received frame.
- **Pros**: Simple, handles short gaps well
- **Cons**: Artifacts on longer gaps, pitch drift

### 3. Interpolation
Use statistical methods to generate synthetic speech.
- Linear interpolation between known samples
- MDCT/DSTFT domain interpolation
- Better quality but more complex

### 4. Linear Prediction (LPC)
Predict lost samples from surrounding context.
- Forward prediction from packet before loss
- Backward prediction from packet after loss
- Blend predictions with weighted average

### 5. Pitch Waveform Replication (PWR)
For voiced speech, detect pitch period and repeat waveform.
- Better for tonal content
- Combine with LPC for best results

## G.711 Appendix I Algorithm

The ITU-T G.711 specification includes a standard PLC algorithm:

1. **For short gaps (≤10ms)**: Repeat last sample
2. **For longer gaps**:
   - Copy pitch period from previous audio
   - Apply gentle fade-out (attenuation)
   - When next packet arrives, fade-in and overlap-add

```rust
// Simplified PLC for G.711
fn conceal_packet_loss(history: &[i16], gap_samples: usize) -> Vec<i16> {
    if gap_samples == 0 {
        return vec![];
    }

    // Simple approach: repeat last samples with fade
    let mut output = Vec::with_capacity(gap_samples);
    let fade_start = gap_samples.saturating_sub(80); // ~10ms fade at 8kHz

    for i in 0..gap_samples {
        let sample_idx = history.len().saturating_sub(1).saturating_sub(i % 160);
        let mut sample = history.get(sample_idx).copied().unwrap_or(0);

        // Apply fade-out for longer gaps
        if i >= fade_start {
            let fade = 1.0 - ((i - fade_start) as f32 / (gap_samples - fade_start) as f32);
            sample = (sample as f32 * fade) as i16;
        }

        output.push(sample);
    }

    output
}
```

## Recommended Implementation for PhoneCheck

Given our use case (speech recognition, not real-time playback), a simple approach suffices:

### Option 1: Simple Repetition (Minimum Viable)
```rust
fn fill_gap(last_samples: &[i16], gap_ms: usize) -> Vec<i16> {
    let gap_samples = (gap_ms * 8) as usize; // 8 samples per ms at 8kHz
    let mut output = Vec::with_capacity(gap_samples);

    // Repeat with fade
    for i in 0..gap_samples {
        let idx = i % last_samples.len().max(1);
        let fade = if i < 80 { 1.0 } else { 0.95_f32.powi((i - 80) as i32 / 8) };
        output.push((last_samples[idx] as f32 * fade) as i16);
    }
    output
}
```

### Option 2: Linear Interpolation (Better for Speech Recognition)
```rust
fn interpolate_gap(before: &[i16], after: &[i16], gap_samples: usize) -> Vec<i16> {
    let mut output = Vec::with_capacity(gap_samples);
    let last_before = *before.last().unwrap_or(&0);
    let first_after = *after.first().unwrap_or(&0);

    for i in 0..gap_samples {
        let t = (i + 1) as f32 / (gap_samples + 1) as f32;
        let sample = ((1.0 - t) * last_before as f32 + t * first_after as f32) as i16;
        output.push(sample);
    }
    output
}
```

## Metrics

| Packet Loss | Speech Quality Impact |
|-------------|----------------------|
| <1% | Negligible |
| 1-3% | Minor artifacts, tolerable |
| 3-5% | Noticeable degradation |
| >5% | Significant issues with G.711 |

## Integration with Jitter Buffer

PLC works in conjunction with the jitter buffer:
1. Jitter buffer requests next packet
2. If packet missing, jitter buffer asks PLC to generate fill audio
3. PLC uses history to generate concealment frame
4. When late packet arrives, either discard or blend

## Sources
- [Packet Loss Concealment - Wikipedia](https://en.wikipedia.org/wiki/Packet_loss_concealment)
- [ITU-T G.711 Appendix I](https://www.itu.int/rec/T-REC-G.711)
- [PLC using polynomial interpolation](https://www.sciencedirect.com/science/article/abs/pii/S0920548922000769)
- [PLC for VoIP using PWR and LPC](https://www.researchgate.net/publication/282381329)
