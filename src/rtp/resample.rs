use anyhow::{Context, Result};
use rubato::{FftFixedIn, Resampler};
use tracing::warn;

/// High-quality FFT-based resampling from 8kHz to 16kHz using Rubato
pub fn resample_8k_to_16k_fft(samples: &[f32]) -> Result<Vec<f32>> {
    if samples.is_empty() {
        return Ok(Vec::new());
    }

    // Create resampler: 8000 Hz -> 16000 Hz (ratio = 2.0)
    // chunk_size should be a reasonable size for processing
    let chunk_size = 1024;
    let mut resampler = FftFixedIn::<f32>::new(8000, 16000, chunk_size, 2, 1)
        .context("Failed to create resampler")?;

    let mut output = Vec::with_capacity(samples.len() * 2);

    // Process in chunks
    let mut pos = 0;
    while pos < samples.len() {
        let end = (pos + chunk_size).min(samples.len());
        let chunk = &samples[pos..end];

        // Rubato expects Vec<Vec<f32>> for multi-channel, we have mono
        let input_frames = vec![chunk.to_vec()];

        // For the last chunk, we may need to pad
        if chunk.len() < chunk_size {
            // Pad with zeros for the final chunk
            let mut padded = chunk.to_vec();
            padded.resize(chunk_size, 0.0);
            let padded_input = vec![padded];

            let resampled = resampler
                .process(&padded_input, None)
                .context("Failed to resample audio")?;

            if !resampled.is_empty() && !resampled[0].is_empty() {
                // Only take the proportion of samples we actually need
                let expected_output = (chunk.len() as f64 * 2.0).ceil() as usize;
                let take = expected_output.min(resampled[0].len());
                output.extend_from_slice(&resampled[0][..take]);
            }
        } else {
            let resampled = resampler
                .process(&input_frames, None)
                .context("Failed to resample audio")?;

            if !resampled.is_empty() {
                output.extend_from_slice(&resampled[0]);
            }
        }

        pos = end;
    }

    Ok(output)
}

/// Simple linear interpolation resampling from 8kHz to 16kHz
pub fn resample_8k_to_16k(samples: &[f32]) -> Vec<f32> {
    if samples.is_empty() {
        return Vec::new();
    }

    let mut output = Vec::with_capacity(samples.len() * 2);

    for i in 0..samples.len() {
        output.push(samples[i]);

        // Interpolate between this sample and the next
        if i + 1 < samples.len() {
            let interpolated = (samples[i] + samples[i + 1]) / 2.0;
            output.push(interpolated);
        } else {
            // Last sample - just duplicate
            output.push(samples[i]);
        }
    }

    output
}

/// Unified 8k to 16k resampler with fallback
pub fn resample_to_16k(samples: &[f32]) -> Vec<f32> {
    match resample_8k_to_16k_fft(samples) {
        Ok(resampled) => resampled,
        Err(e) => {
            warn!("FFT resampling failed, falling back to linear: {}", e);
            resample_8k_to_16k(samples)
        }
    }
}
