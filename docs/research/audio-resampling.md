# Audio Resampling Research

## Current Implementation

PhoneCheck uses simple linear interpolation to resample from 8kHz (G.711 telephone audio) to 16kHz (Whisper requirement). This is fast but introduces aliasing artifacts.

## Recommended Libraries

### 1. [Rubato](https://crates.io/crates/rubato) (Recommended)

High-quality async/sync audio resampling in Rust.

**Pros:**
- Band-limited interpolation using sinc filters (anti-aliasing)
- FFT-based sync mode is fast
- Realtime capable (no allocation during processing)
- Actively maintained

**Usage:**
```rust
use rubato::{FftFixedIn, Resampler};

// 8kHz to 16kHz = ratio of 2.0
let mut resampler = FftFixedIn::<f32>::new(
    8000,   // input sample rate
    16000,  // output sample rate
    1024,   // chunk size
    2,      // sub-chunks
    1,      // channels (mono)
)?;

let waves_in = vec![input_samples];
let waves_out = resampler.process(&waves_in, None)?;
```

### 2. [dasp](https://crates.io/crates/dasp)

General DSP library with resampling support.

**Cons:**
- Reported noise issues when downsampling for speech-to-text ([Issue #135](https://github.com/RustAudio/dasp/issues/135))
- Not ideal for our use case

### 3. Linear Interpolation (Current)

Simple doubling of samples (e.g., `[a, b]` → `[a, a, b, b]` or interpolated).

**Pros:**
- Zero dependencies
- Fast

**Cons:**
- Introduces aliasing
- May reduce transcription accuracy

## Recommendation for PhoneCheck

**Use Rubato's FFT-based sync resampler** for best quality with reasonable performance.

For 8kHz → 16kHz upsampling (ratio 2:1), the FFT method is efficient and avoids aliasing that could confuse Whisper.

### Implementation Notes

1. **Input format**: G.711 decodes to i16 PCM
2. **Convert**: i16 → f32 normalized (-1.0 to 1.0)
3. **Resample**: 8kHz → 16kHz using Rubato
4. **Output**: f32 samples ready for Whisper

### Minimum Viable Change

If adding a dependency is not desired, improve the current linear interpolation by averaging adjacent samples:

```rust
// Better than simple duplication
fn upsample_linear_interp(samples: &[i16]) -> Vec<i16> {
    let mut output = Vec::with_capacity(samples.len() * 2);
    for window in samples.windows(2) {
        output.push(window[0]);
        output.push((window[0] as i32 + window[1] as i32) / 2 as i16);
    }
    if let Some(&last) = samples.last() {
        output.push(last);
        output.push(last);
    }
    output
}
```

## Sources
- [Rubato GitHub](https://github.com/HEnquist/rubato)
- [Rubato docs.rs](https://docs.rs/rubato)
- [dasp GitHub](https://github.com/RustAudio/dasp)
- [dasp noise issue](https://github.com/RustAudio/dasp/issues/135)
