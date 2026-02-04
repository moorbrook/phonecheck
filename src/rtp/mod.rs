pub mod g711;
pub mod jitter;
pub mod receiver;

pub use receiver::RtpReceiver;

use anyhow::{Context, Result};
use std::path::Path;

/// Audio sample rate for Whisper (Hz)
pub const WHISPER_SAMPLE_RATE: u32 = 16000;

/// Convert duration in milliseconds to number of samples at 16kHz
#[inline]
pub fn duration_to_samples(duration_ms: u64) -> usize {
    ((duration_ms as u64) * WHISPER_SAMPLE_RATE as u64 / 1000) as usize
}

/// Convert number of samples to duration in milliseconds at 16kHz
#[inline]
pub fn samples_to_duration_ms(samples: usize) -> u64 {
    ((samples as u64) * 1000) / WHISPER_SAMPLE_RATE as u64
}

/// Save f32 audio samples (16kHz) to a WAV file
pub fn save_wav<P: AsRef<Path>>(samples: &[f32], path: P) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: WHISPER_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = hound::WavWriter::create(path.as_ref(), spec)
        .with_context(|| format!("Failed to create WAV file: {:?}", path.as_ref()))?;

    for &sample in samples {
        // Convert f32 [-1.0, 1.0] to i16
        let s = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
        writer.write_sample(s)?;
    }

    writer.finalize()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_duration_to_samples() {
        // 1000 ms at 16 kHz = 16000 samples
        assert_eq!(duration_to_samples(1000), 16000);
        // 500 ms at 16 kHz = 8000 samples
        assert_eq!(duration_to_samples(500), 8000);
        // 0 ms = 0 samples
        assert_eq!(duration_to_samples(0), 0);
    }

    #[test]
    fn test_samples_to_duration_ms() {
        // 16000 samples at 16 kHz = 1000 ms
        assert_eq!(samples_to_duration_ms(16000), 1000);
        // 8000 samples at 16 kHz = 500 ms
        assert_eq!(samples_to_duration_ms(8000), 500);
        // 0 samples = 0 ms
        assert_eq!(samples_to_duration_ms(0), 0);
    }

    #[test]
    fn test_round_trip_conversion() {
        for ms in [100, 500, 1000, 2000, 5000] {
            let samples = duration_to_samples(ms);
            let back_ms = samples_to_duration_ms(samples);
            assert_eq!(back_ms, ms, "Round trip failed for {}ms", ms);
        }
    }
}

