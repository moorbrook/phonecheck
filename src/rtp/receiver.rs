use anyhow::{Context, Result};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::timeout;
use tracing::{debug, trace, warn};

use super::g711::{G711Codec, G711Decoder};

/// RTP packet header (simplified)
#[derive(Debug)]
struct RtpHeader {
    payload_type: u8,
    sequence: u16,
    timestamp: u32,
    ssrc: u32,
}

pub struct RtpReceiver {
    socket: UdpSocket,
    decoder: Option<G711Decoder>,
    samples: Vec<i16>,
}

impl RtpReceiver {
    pub async fn bind(port: u16) -> Result<Self> {
        let addr = format!("0.0.0.0:{}", port);
        let socket = UdpSocket::bind(&addr)
            .await
            .context(format!("Failed to bind RTP socket on {}", addr))?;

        debug!("RTP receiver bound to port {}", port);

        Ok(Self {
            socket,
            decoder: None,
            samples: Vec::new(),
        })
    }

    pub fn local_port(&self) -> Result<u16> {
        Ok(self.socket.local_addr()?.port())
    }

    /// Receive RTP packets for the specified duration
    pub async fn receive_for(&mut self, duration: Duration) -> Result<()> {
        let mut buf = [0u8; 2048];
        let deadline = tokio::time::Instant::now() + duration;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }

            match timeout(remaining, self.socket.recv_from(&mut buf)).await {
                Ok(Ok((len, _addr))) => {
                    if len >= 12 {
                        self.process_packet(&buf[..len]);
                    }
                }
                Ok(Err(e)) => {
                    warn!("RTP receive error: {}", e);
                }
                Err(_) => {
                    // Timeout - we're done
                    break;
                }
            }
        }

        debug!("Received {} audio samples", self.samples.len());
        Ok(())
    }

    fn process_packet(&mut self, data: &[u8]) {
        if data.len() < 12 {
            return;
        }

        // Verify RTP version (first 2 bits must be 2)
        let version = (data[0] >> 6) & 0x03;
        if version != 2 {
            trace!("Ignoring non-RTP packet (version={})", version);
            return;
        }

        let header = self.parse_header(data);
        trace!(
            "RTP: PT={} seq={} ts={} ssrc={}",
            header.payload_type,
            header.sequence,
            header.timestamp,
            header.ssrc
        );

        // Initialize decoder based on payload type if not already done
        if self.decoder.is_none() {
            self.decoder = G711Decoder::from_payload_type(header.payload_type);
            if self.decoder.is_none() {
                warn!(
                    "Unsupported RTP payload type: {}. Only G.711 (0=PCMU, 8=PCMA) supported.",
                    header.payload_type
                );
                return;
            }
            debug!(
                "Detected audio codec: {:?}",
                if header.payload_type == 0 {
                    G711Codec::ULaw
                } else {
                    G711Codec::ALaw
                }
            );
        }

        // Skip header (12 bytes minimum, but could have CSRC and extensions)
        let payload_start = self.calculate_payload_offset(data);
        if payload_start >= data.len() {
            return;
        }

        let payload = &data[payload_start..];

        if let Some(ref decoder) = self.decoder {
            // Decode directly into samples buffer (avoids per-packet Vec allocation)
            decoder.decode_into(payload, &mut self.samples);
        }
    }

    fn parse_header(&self, data: &[u8]) -> RtpHeader {
        RtpHeader {
            payload_type: data[1] & 0x7F,
            sequence: u16::from_be_bytes([data[2], data[3]]),
            timestamp: u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
            ssrc: u32::from_be_bytes([data[8], data[9], data[10], data[11]]),
        }
    }

    fn calculate_payload_offset(&self, data: &[u8]) -> usize {
        let cc = data[0] & 0x0F; // CSRC count
        let has_extension = (data[0] & 0x10) != 0;

        let mut offset = 12 + (cc as usize * 4);

        if has_extension && data.len() > offset + 4 {
            let ext_length = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;
            offset += 4 + (ext_length * 4);
        }

        offset
    }

    /// Get accumulated samples as f32 (for Whisper)
    /// Resamples from 8kHz (G.711) to 16kHz (Whisper expects 16kHz)
    /// Fused operation: converts i16→f32 and resamples in a single pass
    pub fn get_samples_f32(&self) -> Vec<f32> {
        if self.samples.is_empty() {
            return Vec::new();
        }

        // Fused i16→f32 conversion with 8kHz→16kHz resampling in one allocation
        let mut output = Vec::with_capacity(self.samples.len() * 2);

        for i in 0..self.samples.len() {
            let sample = self.samples[i] as f32 / 32768.0;
            output.push(sample);

            // Interpolate between this sample and the next
            if i + 1 < self.samples.len() {
                let next = self.samples[i + 1] as f32 / 32768.0;
                output.push((sample + next) * 0.5);
            } else {
                // Last sample - duplicate
                output.push(sample);
            }
        }

        output
    }

}

/// Simple linear interpolation resampling from 8kHz to 16kHz (public for testing)
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

/// Parse RTP header from raw bytes (public for testing)
/// Returns None if packet is too short or not RTP version 2
pub fn parse_rtp_header(data: &[u8]) -> Option<(u8, u16, u32, u32, usize)> {
    if data.len() < 12 {
        return None;
    }

    // Check RTP version (first 2 bits must be 2)
    let version = (data[0] >> 6) & 0x03;
    if version != 2 {
        return None;
    }

    let payload_type = data[1] & 0x7F;
    let sequence = u16::from_be_bytes([data[2], data[3]]);
    let timestamp = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let ssrc = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);

    // Calculate payload offset
    let cc = data[0] & 0x0F; // CSRC count
    let has_extension = (data[0] & 0x10) != 0;

    let mut offset = 12 + (cc as usize * 4);

    if has_extension && data.len() > offset + 4 {
        let ext_length = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;
        offset += 4 + (ext_length * 4);
    }

    Some((payload_type, sequence, timestamp, ssrc, offset))
}

#[cfg(test)]
mod tests {
    use super::*;

    // === resample_8k_to_16k tests ===

    #[test]
    fn test_resample_empty() {
        assert_eq!(resample_8k_to_16k(&[]), Vec::<f32>::new());
    }

    #[test]
    fn test_resample_single_sample() {
        let input = vec![0.5];
        let output = resample_8k_to_16k(&input);
        assert_eq!(output.len(), 2);
        assert_eq!(output[0], 0.5);
        assert_eq!(output[1], 0.5); // Duplicated
    }

    #[test]
    fn test_resample_two_samples() {
        let input = vec![0.0, 1.0];
        let output = resample_8k_to_16k(&input);
        assert_eq!(output.len(), 4);
        assert_eq!(output[0], 0.0);
        assert_eq!(output[1], 0.5); // Interpolated
        assert_eq!(output[2], 1.0);
        assert_eq!(output[3], 1.0); // Duplicated (last sample)
    }

    #[test]
    fn test_resample_preserves_original_samples() {
        let input = vec![0.1, 0.2, 0.3, 0.4];
        let output = resample_8k_to_16k(&input);
        // Original samples should be at even indices
        assert_eq!(output[0], 0.1);
        assert_eq!(output[2], 0.2);
        assert_eq!(output[4], 0.3);
        assert_eq!(output[6], 0.4);
    }

    #[test]
    fn test_resample_doubles_length() {
        for len in [1, 2, 5, 10, 100] {
            let input: Vec<f32> = (0..len).map(|i| i as f32).collect();
            let output = resample_8k_to_16k(&input);
            assert_eq!(output.len(), len * 2);
        }
    }

    #[test]
    fn test_fused_matches_separate_operations() {
        use crate::rtp::g711::G711Decoder;

        // Create sample i16 PCM data
        let pcm_samples: Vec<i16> = vec![0, 1000, -1000, 16000, -16000, 32000, -32000, 0];

        // Original two-step approach
        let f32_samples = G711Decoder::pcm_to_f32(&pcm_samples);
        let resampled_separate = resample_8k_to_16k(&f32_samples);

        // Fused approach (simulating what get_samples_f32 does)
        let mut fused_output = Vec::with_capacity(pcm_samples.len() * 2);
        for i in 0..pcm_samples.len() {
            let sample = pcm_samples[i] as f32 / 32768.0;
            fused_output.push(sample);
            if i + 1 < pcm_samples.len() {
                let next = pcm_samples[i + 1] as f32 / 32768.0;
                fused_output.push((sample + next) * 0.5);
            } else {
                fused_output.push(sample);
            }
        }

        // Compare results
        assert_eq!(resampled_separate.len(), fused_output.len());
        for (a, b) in resampled_separate.iter().zip(fused_output.iter()) {
            assert!((a - b).abs() < 1e-6, "Mismatch: {} vs {}", a, b);
        }
    }

    // === parse_rtp_header tests ===

    #[test]
    fn test_parse_rtp_header_valid() {
        // Build a valid RTP packet
        // Version 2, no padding, no extension, no CSRC, PT=0 (PCMU)
        let packet = [
            0x80, // V=2, P=0, X=0, CC=0
            0x00, // M=0, PT=0 (PCMU)
            0x00, 0x01, // Sequence = 1
            0x00, 0x00, 0x00, 0x10, // Timestamp = 16
            0x12, 0x34, 0x56, 0x78, // SSRC
            0xAA, 0xBB, // Payload
        ];

        let result = parse_rtp_header(&packet);
        assert!(result.is_some());
        let (pt, seq, ts, ssrc, offset) = result.unwrap();
        assert_eq!(pt, 0);
        assert_eq!(seq, 1);
        assert_eq!(ts, 16);
        assert_eq!(ssrc, 0x12345678);
        assert_eq!(offset, 12);
    }

    #[test]
    fn test_parse_rtp_header_pcma() {
        let packet = [0x80, 0x08, 0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01];
        let (pt, _, _, _, _) = parse_rtp_header(&packet).unwrap();
        assert_eq!(pt, 8); // PCMA
    }

    #[test]
    fn test_parse_rtp_header_too_short() {
        let packet = [0x80, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00]; // Only 7 bytes
        assert!(parse_rtp_header(&packet).is_none());
    }

    #[test]
    fn test_parse_rtp_header_wrong_version() {
        // Version 0 (not RTP)
        let packet = [0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01];
        assert!(parse_rtp_header(&packet).is_none());

        // Version 1
        let packet = [0x40, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01];
        assert!(parse_rtp_header(&packet).is_none());

        // Version 3
        let packet = [0xC0, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01];
        assert!(parse_rtp_header(&packet).is_none());
    }

    #[test]
    fn test_parse_rtp_header_with_csrc() {
        // Version 2, CC=2 (2 CSRCs)
        let packet = [
            0x82, // V=2, P=0, X=0, CC=2
            0x00, // PT=0
            0x00, 0x01, // Seq
            0x00, 0x00, 0x00, 0x10, // Timestamp
            0x00, 0x00, 0x00, 0x01, // SSRC
            0x00, 0x00, 0x00, 0x02, // CSRC 1
            0x00, 0x00, 0x00, 0x03, // CSRC 2
            0xAA, // Payload
        ];

        let result = parse_rtp_header(&packet);
        assert!(result.is_some());
        let (_, _, _, _, offset) = result.unwrap();
        assert_eq!(offset, 12 + 8); // 12 header + 2*4 CSRCs
    }

    #[test]
    fn test_parse_rtp_header_with_extension() {
        // Version 2, X=1 (has extension)
        let packet = [
            0x90, // V=2, P=0, X=1, CC=0
            0x00, // PT=0
            0x00, 0x01, // Seq
            0x00, 0x00, 0x00, 0x10, // Timestamp
            0x00, 0x00, 0x00, 0x01, // SSRC
            0xBE, 0xDE, // Extension header ID
            0x00, 0x01, // Extension length = 1 (4 bytes)
            0x00, 0x00, 0x00, 0x00, // Extension data
            0xAA, // Payload
        ];

        let result = parse_rtp_header(&packet);
        assert!(result.is_some());
        let (_, _, _, _, offset) = result.unwrap();
        assert_eq!(offset, 12 + 4 + 4); // 12 header + 4 ext header + 4 ext data
    }

    #[test]
    fn test_parse_rtp_sequence_wraparound() {
        // Max sequence number
        let packet = [
            0x80, 0x00,
            0xFF, 0xFF, // Seq = 65535
            0x00, 0x00, 0x00, 0x10,
            0x00, 0x00, 0x00, 0x01,
        ];
        let (_, seq, _, _, _) = parse_rtp_header(&packet).unwrap();
        assert_eq!(seq, 65535);
    }

    #[test]
    fn test_parse_rtp_marker_bit() {
        // Marker bit set (bit 7 of byte 1)
        let packet = [
            0x80, 0x80, // M=1, PT=0
            0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01,
        ];
        let (pt, _, _, _, _) = parse_rtp_header(&packet).unwrap();
        // PT should still be 0 (marker bit is separate)
        assert_eq!(pt, 0);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Resampled output is exactly 2x input length
        #[test]
        fn resample_doubles_length(samples in proptest::collection::vec(-1.0f32..1.0f32, 0..100)) {
            let output = resample_8k_to_16k(&samples);
            if samples.is_empty() {
                prop_assert_eq!(output.len(), 0);
            } else {
                prop_assert_eq!(output.len(), samples.len() * 2);
            }
        }

        /// Resampled output values are bounded by input range
        #[test]
        fn resample_preserves_range(samples in proptest::collection::vec(-1.0f32..1.0f32, 1..50)) {
            let output = resample_8k_to_16k(&samples);
            let min_in = samples.iter().cloned().fold(f32::INFINITY, f32::min);
            let max_in = samples.iter().cloned().fold(f32::NEG_INFINITY, f32::max);

            for sample in &output {
                prop_assert!(*sample >= min_in && *sample <= max_in);
            }
        }

        /// parse_rtp_header never panics on any input
        #[test]
        fn parse_header_never_panics(data in proptest::collection::vec(any::<u8>(), 0..100)) {
            let _ = parse_rtp_header(&data);
        }

        /// Valid RTP v2 packets are parsed
        #[test]
        fn valid_rtp_parsed(
            pt in 0u8..128u8,
            seq in any::<u16>(),
            ts in any::<u32>(),
            ssrc in any::<u32>()
        ) {
            let mut packet = vec![0x80, pt]; // V=2, PT
            packet.extend_from_slice(&seq.to_be_bytes());
            packet.extend_from_slice(&ts.to_be_bytes());
            packet.extend_from_slice(&ssrc.to_be_bytes());

            let result = parse_rtp_header(&packet);
            prop_assert!(result.is_some());
            let (parsed_pt, parsed_seq, parsed_ts, parsed_ssrc, offset) = result.unwrap();
            prop_assert_eq!(parsed_pt, pt & 0x7F);
            prop_assert_eq!(parsed_seq, seq);
            prop_assert_eq!(parsed_ts, ts);
            prop_assert_eq!(parsed_ssrc, ssrc);
            prop_assert_eq!(offset, 12);
        }

        /// Fused i16→f32+resample matches separate operations
        #[test]
        fn fused_matches_separate(samples in proptest::collection::vec(-32768i16..32767i16, 1..50)) {
            use crate::rtp::g711::G711Decoder;

            // Original two-step approach
            let f32_samples = G711Decoder::pcm_to_f32(&samples);
            let resampled_separate = resample_8k_to_16k(&f32_samples);

            // Fused approach
            let mut fused_output = Vec::with_capacity(samples.len() * 2);
            for i in 0..samples.len() {
                let sample = samples[i] as f32 / 32768.0;
                fused_output.push(sample);
                if i + 1 < samples.len() {
                    let next = samples[i + 1] as f32 / 32768.0;
                    fused_output.push((sample + next) * 0.5);
                } else {
                    fused_output.push(sample);
                }
            }

            prop_assert_eq!(resampled_separate.len(), fused_output.len());
            for (a, b) in resampled_separate.iter().zip(fused_output.iter()) {
                prop_assert!((a - b).abs() < 1e-6);
            }
        }
    }
}

/// Kani formal verification proofs
#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    fn parse_header_never_panics() {
        let data: [u8; 16] = kani::any();
        let _ = parse_rtp_header(&data);
    }

    #[kani::proof]
    fn resample_length_correct() {
        // Test with small input to keep verification tractable
        let samples: [f32; 3] = kani::any();
        let output = resample_8k_to_16k(&samples);
        kani::assert(output.len() == 6, "output must be 2x input length");
    }

    #[kani::proof]
    fn rtp_version_check() {
        let data: [u8; 12] = kani::any();
        let version = (data[0] >> 6) & 0x03;

        let result = parse_rtp_header(&data);

        if version != 2 {
            kani::assert(result.is_none(), "non-v2 packets must be rejected");
        }
    }
}
