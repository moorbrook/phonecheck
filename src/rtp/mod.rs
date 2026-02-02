pub mod g711;
pub mod jitter;
pub mod receiver;

pub use receiver::RtpReceiver;

use anyhow::{Context, Result};
use std::path::Path;

/// Save f32 audio samples (16kHz) to a WAV file
pub fn save_wav<P: AsRef<Path>>(samples: &[f32], path: P) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16000,
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
