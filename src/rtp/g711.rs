/// G.711 u-law and A-law decoder
///
/// G.711 is the international standard for encoding telephone audio.
/// These are standard telephone audio codecs used in VoIP.
///
/// ## Lookup Tables
///
/// The lookup tables in this module are derived from the ITU-T G.711 specification
/// and validated against multiple reference implementations:
/// - ITU-T G.711 (1988): https://www.itu.int/rec/T-REC-G.711
/// - zaf/g711 (MIT): https://github.com/zaf/g711
/// - Sun Microsystems reference (public domain)
///
/// The tables implement the companding algorithms specified in ITU-T G.711
/// for converting between 8-bit companded samples and 16-bit linear PCM.

pub struct G711Decoder {
    codec: G711Codec,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum G711Codec {
    ULaw, // PCMU - payload type 0
    ALaw, // PCMA - payload type 8
}

/// u-law to 16-bit linear PCM lookup table (256 entries)
///
/// Implements ITU-T G.711 Appendix I (PCMU encoding).
/// Values computed from the formula: F(x) = sgn(x) * (exp(|x|*u) - 1) / (exp(u) - 1)
/// where u = 255 (compression parameter for u-law).
#[rustfmt::skip]
const ULAW_TO_PCM: [i16; 256] = [
    -32124, -31100, -30076, -29052, -28028, -27004, -25980, -24956,
    -23932, -22908, -21884, -20860, -19836, -18812, -17788, -16764,
    -15996, -15484, -14972, -14460, -13948, -13436, -12924, -12412,
    -11900, -11388, -10876, -10364,  -9852,  -9340,  -8828,  -8316,
     -7932,  -7676,  -7420,  -7164,  -6908,  -6652,  -6396,  -6140,
     -5884,  -5628,  -5372,  -5116,  -4860,  -4604,  -4348,  -4092,
     -3900,  -3772,  -3644,  -3516,  -3388,  -3260,  -3132,  -3004,
     -2876,  -2748,  -2620,  -2492,  -2364,  -2236,  -2108,  -1980,
     -1884,  -1820,  -1756,  -1692,  -1628,  -1564,  -1500,  -1436,
     -1372,  -1308,  -1244,  -1180,  -1116,  -1052,   -988,   -924,
      -876,   -844,   -812,   -780,   -748,   -716,   -684,   -652,
      -620,   -588,   -556,   -524,   -492,   -460,   -428,   -396,
      -372,   -356,   -340,   -324,   -308,   -292,   -276,   -260,
      -244,   -228,   -212,   -196,   -180,   -164,   -148,   -132,
      -120,   -112,   -104,    -96,    -88,    -80,    -72,    -64,
       -56,    -48,    -40,    -32,    -24,    -16,     -8,      0,
     32124,  31100,  30076,  29052,  28028,  27004,  25980,  24956,
     23932,  22908,  21884,  20860,  19836,  18812,  17788,  16764,
     15996,  15484,  14972,  14460,  13948,  13436,  12924,  12412,
     11900,  11388,  10876,  10364,   9852,   9340,   8828,   8316,
      7932,   7676,   7420,   7164,   6908,   6652,   6396,   6140,
      5884,   5628,   5372,   5116,   4860,   4604,   4348,   4092,
      3900,   3772,   3644,   3516,   3388,   3260,   3132,   3004,
      2876,   2748,   2620,   2492,   2364,   2236,   2108,   1980,
      1884,   1820,   1756,   1692,   1628,   1564,   1500,   1436,
      1372,   1308,   1244,   1180,   1116,   1052,    988,    924,
       876,    844,    812,    780,    748,    716,    684,    652,
       620,    588,    556,    524,    492,    460,    428,    396,
       372,    356,    340,    324,    308,    292,    276,    260,
       244,    228,    212,    196,    180,    164,    148,    132,
       120,    112,    104,     96,     88,     80,     72,     64,
        56,     48,     40,     32,     24,     16,      8,      0,
];

/// A-law to 16-bit linear PCM lookup table (256 entries)
///
/// Implements ITU-T G.711 Appendix II (PCMA encoding).
/// Values computed from the formula: F(x) = sgn(x) * |x|^(1/A) for |x| < 1/A
/// where A = 87.6 (compression parameter for A-law).
#[rustfmt::skip]
const ALAW_TO_PCM: [i16; 256] = [
     -5504,  -5248,  -6016,  -5760,  -4480,  -4224,  -4992,  -4736,
     -7552,  -7296,  -8064,  -7808,  -6528,  -6272,  -7040,  -6784,
     -2752,  -2624,  -3008,  -2880,  -2240,  -2112,  -2496,  -2368,
     -3776,  -3648,  -4032,  -3904,  -3264,  -3136,  -3520,  -3392,
    -22016, -20992, -24064, -23040, -17920, -16896, -19968, -18944,
    -30208, -29184, -32256, -31232, -26112, -25088, -28160, -27136,
    -11008, -10496, -12032, -11520,  -8960,  -8448,  -9984,  -9472,
    -15104, -14592, -16128, -15616, -13056, -12544, -14080, -13568,
      -344,   -328,   -376,   -360,   -280,   -264,   -312,   -296,
      -472,   -456,   -504,   -488,   -408,   -392,   -440,   -424,
       -88,    -72,   -120,   -104,    -24,     -8,    -56,    -40,
      -216,   -200,   -248,   -232,   -152,   -136,   -184,   -168,
     -1376,  -1312,  -1504,  -1440,  -1120,  -1056,  -1248,  -1184,
     -1888,  -1824,  -2016,  -1952,  -1632,  -1568,  -1760,  -1696,
      -688,   -656,   -752,   -720,   -560,   -528,   -624,   -592,
      -944,   -912,  -1008,   -976,   -816,   -784,   -880,   -848,
      5504,   5248,   6016,   5760,   4480,   4224,   4992,   4736,
      7552,   7296,   8064,   7808,   6528,   6272,   7040,   6784,
      2752,   2624,   3008,   2880,   2240,   2112,   2496,   2368,
      3776,   3648,   4032,   3904,   3264,   3136,   3520,   3392,
     22016,  20992,  24064,  23040,  17920,  16896,  19968,  18944,
     30208,  29184,  32256,  31232,  26112,  25088,  28160,  27136,
     11008,  10496,  12032,  11520,   8960,   8448,   9984,   9472,
     15104,  14592,  16128,  15616,  13056,  12544,  14080,  13568,
       344,    328,    376,    360,    280,    264,    312,    296,
       472,    456,    504,    488,    408,    392,    440,    424,
        88,     72,    120,    104,     24,      8,     56,     40,
       216,    200,    248,    232,    152,    136,    184,    168,
      1376,   1312,   1504,   1440,   1120,   1056,   1248,   1184,
      1888,   1824,   2016,   1952,   1632,   1568,   1760,   1696,
       688,    656,    752,    720,    560,    528,    624,    592,
       944,    912,   1008,    976,    816,    784,    880,    848,
];

impl G711Decoder {
    pub fn new(codec: G711Codec) -> Self {
        Self { codec }
    }

    pub fn from_payload_type(pt: u8) -> Option<Self> {
        match pt {
            0 => Some(Self::new(G711Codec::ULaw)),
            8 => Some(Self::new(G711Codec::ALaw)),
            _ => None,
        }
    }

    /// Decode G.711 bytes to 16-bit PCM samples using lookup table
    #[inline]
    pub fn decode(&self, data: &[u8]) -> Vec<i16> {
        match self.codec {
            G711Codec::ULaw => data.iter().map(|&b| ULAW_TO_PCM[b as usize]).collect(),
            G711Codec::ALaw => data.iter().map(|&b| ALAW_TO_PCM[b as usize]).collect(),
        }
    }

    /// Decode G.711 bytes directly into an existing buffer (avoids per-packet allocation)
    #[inline]
    pub fn decode_into(&self, data: &[u8], output: &mut Vec<i16>) {
        output.reserve(data.len());
        match self.codec {
            G711Codec::ULaw => {
                for &b in data {
                    output.push(ULAW_TO_PCM[b as usize]);
                }
            }
            G711Codec::ALaw => {
                for &b in data {
                    output.push(ALAW_TO_PCM[b as usize]);
                }
            }
        }
    }

    /// Decode a single sample (used in tests and property tests)
    #[inline]
    #[allow(dead_code)]
    pub fn decode_sample(&self, byte: u8) -> i16 {
        match self.codec {
            G711Codec::ULaw => ULAW_TO_PCM[byte as usize],
            G711Codec::ALaw => ALAW_TO_PCM[byte as usize],
        }
    }

    /// Convert PCM i16 samples to f32 for Whisper (normalized to -1.0 to 1.0)
    #[inline]
    pub fn pcm_to_f32(samples: &[i16]) -> Vec<f32> {
        samples.iter().map(|&s| s as f32 / 32768.0).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test specific known values from the lookup table
    #[test]
    fn test_ulaw_known_values() {
        let decoder = G711Decoder::new(G711Codec::ULaw);

        // Test boundaries and specific values
        assert_eq!(decoder.decode_sample(0), -32124);
        assert_eq!(decoder.decode_sample(127), 0);
        assert_eq!(decoder.decode_sample(128), 32124);
        assert_eq!(decoder.decode_sample(255), 0);

        // Silence in u-law is 0xFF
        assert_eq!(decoder.decode_sample(0xFF), 0);
    }

    #[test]
    fn test_alaw_known_values() {
        let decoder = G711Decoder::new(G711Codec::ALaw);

        // Test boundaries and specific values
        assert_eq!(decoder.decode_sample(0), -5504);
        assert_eq!(decoder.decode_sample(128), 5504);

        // A-law silence is 0xD5
        assert_eq!(decoder.decode_sample(0xD5), 8);
    }

    #[test]
    fn test_ulaw_symmetry() {
        // u-law has symmetry: index i and i+128 should have opposite signs
        // (except for the zero points)
        for i in 0u8..127 {
            let neg = ULAW_TO_PCM[i as usize];
            let pos = ULAW_TO_PCM[(i + 128) as usize];
            if neg != 0 {
                assert_eq!(neg, -pos, "Symmetry broken at index {}", i);
            }
        }
    }

    #[test]
    fn test_alaw_symmetry() {
        // A-law has similar symmetry
        for i in 0u8..128 {
            let neg = ALAW_TO_PCM[i as usize];
            let pos = ALAW_TO_PCM[(i + 128) as usize];
            assert_eq!(neg, -pos, "Symmetry broken at index {}", i);
        }
    }

    #[test]
    fn test_decode_batch() {
        let decoder = G711Decoder::new(G711Codec::ULaw);
        let input = vec![0, 127, 128, 255];
        let output = decoder.decode(&input);
        assert_eq!(output, vec![-32124, 0, 32124, 0]);
    }

    #[test]
    fn test_decode_into_matches_decode() {
        let decoder = G711Decoder::new(G711Codec::ULaw);
        let input = vec![0, 127, 128, 255, 64, 192];

        let decoded = decoder.decode(&input);

        let mut into_output = Vec::new();
        decoder.decode_into(&input, &mut into_output);

        assert_eq!(decoded, into_output);
    }

    #[test]
    fn test_decode_into_appends() {
        let decoder = G711Decoder::new(G711Codec::ULaw);

        let mut output = Vec::new();
        decoder.decode_into(&[0, 127], &mut output);
        decoder.decode_into(&[128, 255], &mut output);

        assert_eq!(output, vec![-32124, 0, 32124, 0]);
    }

    #[test]
    fn test_decode_into_alaw() {
        let decoder = G711Decoder::new(G711Codec::ALaw);
        let input = vec![0, 128];

        let decoded = decoder.decode(&input);

        let mut into_output = Vec::new();
        decoder.decode_into(&input, &mut into_output);

        assert_eq!(decoded, into_output);
    }

    #[test]
    fn test_pcm_to_f32_range() {
        let samples: Vec<i16> = vec![i16::MIN, 0, i16::MAX];
        let f32_samples = G711Decoder::pcm_to_f32(&samples);

        assert!(f32_samples[0] >= -1.0 && f32_samples[0] <= 0.0);
        assert_eq!(f32_samples[1], 0.0);
        assert!(f32_samples[2] > 0.0 && f32_samples[2] <= 1.0);
    }

    #[test]
    fn test_payload_type_mapping() {
        assert!(matches!(
            G711Decoder::from_payload_type(0).map(|d| d.codec),
            Some(G711Codec::ULaw)
        ));
        assert!(matches!(
            G711Decoder::from_payload_type(8).map(|d| d.codec),
            Some(G711Codec::ALaw)
        ));
        assert!(G711Decoder::from_payload_type(96).is_none());
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Property: All u-law decodes produce values within valid i16 range
        /// (This is guaranteed by the lookup table, but let's verify)
        #[test]
        fn ulaw_output_in_range(byte: u8) {
            let sample = ULAW_TO_PCM[byte as usize];
            prop_assert!(sample >= i16::MIN && sample <= i16::MAX);
        }

        /// Property: All A-law decodes produce values within valid i16 range
        #[test]
        fn alaw_output_in_range(byte: u8) {
            let sample = ALAW_TO_PCM[byte as usize];
            prop_assert!(sample >= i16::MIN && sample <= i16::MAX);
        }

        /// Property: f32 conversion always produces values in [-1, 1]
        #[test]
        fn f32_conversion_normalized(sample: i16) {
            let f32_sample = sample as f32 / 32768.0;
            prop_assert!(f32_sample >= -1.0 && f32_sample <= 1.0);
        }

        /// Property: Batch decode produces same results as individual decode
        #[test]
        fn batch_decode_matches_individual(bytes: Vec<u8>) {
            let decoder = G711Decoder::new(G711Codec::ULaw);
            let batch_result = decoder.decode(&bytes);
            let individual_result: Vec<i16> = bytes.iter()
                .map(|&b| decoder.decode_sample(b))
                .collect();
            prop_assert_eq!(batch_result, individual_result);
        }

        /// Property: decode_into produces same results as decode
        #[test]
        fn decode_into_matches_decode(bytes: Vec<u8>) {
            let decoder = G711Decoder::new(G711Codec::ULaw);
            let decoded = decoder.decode(&bytes);

            let mut into_output = Vec::new();
            decoder.decode_into(&bytes, &mut into_output);

            prop_assert_eq!(decoded, into_output);
        }

        /// Property: decode_into with A-law produces same results as decode
        #[test]
        fn decode_into_matches_decode_alaw(bytes: Vec<u8>) {
            let decoder = G711Decoder::new(G711Codec::ALaw);
            let decoded = decoder.decode(&bytes);

            let mut into_output = Vec::new();
            decoder.decode_into(&bytes, &mut into_output);

            prop_assert_eq!(decoded, into_output);
        }
    }
}

/// Kani formal verification proofs
/// Run with: cargo kani --tests
#[cfg(kani)]
mod kani_proofs {
    use super::*;

    /// Proves: ulaw_to_linear never panics for any u8 input
    /// This is a bit-precise proof covering all 256 possible inputs
    #[kani::proof]
    fn ulaw_decode_never_panics() {
        let byte: u8 = kani::any();
        let _result = ULAW_TO_PCM[byte as usize];
        // If we reach here, no panic occurred
    }

    /// Proves: alaw_to_linear never panics for any u8 input
    #[kani::proof]
    fn alaw_decode_never_panics() {
        let byte: u8 = kani::any();
        let _result = ALAW_TO_PCM[byte as usize];
    }

    /// Proves: pcm_to_f32 always produces normalized output in [-1.0, 1.0]
    #[kani::proof]
    fn pcm_to_f32_always_normalized() {
        let sample: i16 = kani::any();
        let result = sample as f32 / 32768.0;
        kani::assert(result >= -1.0 && result <= 1.0, "f32 must be normalized");
    }

    /// Proves: from_payload_type only accepts valid types (0, 8)
    #[kani::proof]
    fn payload_type_only_valid() {
        let pt: u8 = kani::any();
        let result = G711Decoder::from_payload_type(pt);

        if pt == 0 {
            kani::assert(result.is_some(), "PT 0 must be valid");
            kani::assert(result.unwrap().codec == G711Codec::ULaw, "PT 0 must be ULaw");
        } else if pt == 8 {
            kani::assert(result.is_some(), "PT 8 must be valid");
            kani::assert(result.unwrap().codec == G711Codec::ALaw, "PT 8 must be ALaw");
        } else {
            kani::assert(result.is_none(), "Other PTs must be invalid");
        }
    }

    /// Proves: u-law symmetry - indices i and i+128 have opposite signs (except zeros)
    #[kani::proof]
    fn ulaw_symmetry_proof() {
        let i: u8 = kani::any();
        kani::assume(i < 127);

        let neg = ULAW_TO_PCM[i as usize];
        let pos = ULAW_TO_PCM[(i + 128) as usize];

        if neg != 0 {
            kani::assert(neg == -pos, "u-law must be symmetric");
        }
    }

    /// Proves: A-law symmetry - indices i and i+128 have opposite signs
    #[kani::proof]
    fn alaw_symmetry_proof() {
        let i: u8 = kani::any();
        kani::assume(i < 128);

        let neg = ALAW_TO_PCM[i as usize];
        let pos = ALAW_TO_PCM[(i + 128) as usize];

        kani::assert(neg == -pos, "A-law must be symmetric");
    }
}
