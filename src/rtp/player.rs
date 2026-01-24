/// RTP Packet Player
/// Replays RTP packets from a pcap file for deterministic testing
///
/// This module uses pcap-file (dev-dependency) to read pcap files
/// without requiring libpcap at runtime.

use anyhow::{Context, Result};
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tracing::{debug, info, trace};

/// Parsed RTP packet from pcap
#[derive(Debug, Clone)]
pub struct RtpPacket {
    /// Timestamp relative to first packet (microseconds)
    pub timestamp_us: u64,
    /// Raw UDP payload (RTP data)
    pub data: Vec<u8>,
    /// Source port from IP header
    pub src_port: u16,
    /// Destination port from IP header
    pub dst_port: u16,
}

/// Load RTP packets from a pcap file
/// Extracts UDP payloads, stripping Ethernet/IP/UDP headers
pub fn load_pcap<P: AsRef<Path>>(path: P) -> Result<Vec<RtpPacket>> {
    use std::fs::File;
    use std::io::BufReader;

    let file = File::open(path.as_ref())
        .context(format!("Failed to open pcap file: {:?}", path.as_ref()))?;
    let reader = BufReader::new(file);

    // Try to parse as pcap or pcapng
    load_pcap_from_reader(reader)
}

/// Load pcap file using pcap-file crate (only available in tests)
#[cfg(test)]
fn load_pcap_from_reader<R: std::io::Read>(reader: R) -> Result<Vec<RtpPacket>> {
    use pcap_file::pcap::PcapReader;

    let mut pcap_reader = PcapReader::new(reader)
        .context("Failed to parse pcap file")?;

    let mut packets = Vec::new();
    let mut first_ts: Option<u64> = None;

    while let Some(pkt) = pcap_reader.next_packet() {
        let pkt = pkt.context("Failed to read packet")?;

        // Calculate relative timestamp
        let ts_us = pkt.timestamp.as_micros() as u64;
        let relative_ts = match first_ts {
            None => {
                first_ts = Some(ts_us);
                0
            }
            Some(first) => ts_us.saturating_sub(first),
        };

        // Parse Ethernet + IP + UDP headers to extract RTP payload
        if let Some(rtp_packet) = parse_udp_payload(&pkt.data, relative_ts) {
            packets.push(rtp_packet);
        }
    }

    info!("Loaded {} RTP packets from pcap", packets.len());
    Ok(packets)
}

/// Stub for non-test builds - pcap loading requires pcap-file (dev-dependency)
#[cfg(not(test))]
fn load_pcap_from_reader<R: std::io::Read>(_reader: R) -> Result<Vec<RtpPacket>> {
    anyhow::bail!("pcap loading only available in test builds (pcap-file is a dev-dependency)")
}

/// Parse Ethernet/IP/UDP headers and extract UDP payload
fn parse_udp_payload(data: &[u8], timestamp_us: u64) -> Option<RtpPacket> {
    // Minimum: 14 (eth) + 20 (ip) + 8 (udp) + 12 (rtp header) = 54 bytes
    if data.len() < 54 {
        return None;
    }

    // Skip Ethernet header (14 bytes)
    let ip_data = &data[14..];

    // Check IP version (must be IPv4)
    let ip_version = (ip_data[0] >> 4) & 0x0F;
    if ip_version != 4 {
        trace!("Skipping non-IPv4 packet (version={})", ip_version);
        return None;
    }

    // Get IP header length (in 32-bit words)
    let ip_header_len = ((ip_data[0] & 0x0F) as usize) * 4;
    if ip_data.len() < ip_header_len + 8 {
        return None;
    }

    // Check protocol is UDP (17)
    let protocol = ip_data[9];
    if protocol != 17 {
        trace!("Skipping non-UDP packet (protocol={})", protocol);
        return None;
    }

    // Parse UDP header
    let udp_data = &ip_data[ip_header_len..];
    let src_port = u16::from_be_bytes([udp_data[0], udp_data[1]]);
    let dst_port = u16::from_be_bytes([udp_data[2], udp_data[3]]);
    let udp_len = u16::from_be_bytes([udp_data[4], udp_data[5]]) as usize;

    // Extract UDP payload (skip 8-byte UDP header)
    if udp_data.len() < 8 || udp_len < 8 {
        return None;
    }

    let payload_len = udp_len - 8;
    if udp_data.len() < 8 + payload_len {
        return None;
    }

    let payload = udp_data[8..8 + payload_len].to_vec();

    // Verify it looks like RTP (version 2)
    if payload.len() >= 12 {
        let rtp_version = (payload[0] >> 6) & 0x03;
        if rtp_version != 2 {
            trace!("Skipping non-RTP packet (rtp_version={})", rtp_version);
            return None;
        }
    }

    Some(RtpPacket {
        timestamp_us,
        data: payload,
        src_port,
        dst_port,
    })
}

/// Replay RTP packets to a target address with original timing
pub async fn replay_to_socket(
    packets: &[RtpPacket],
    socket: &UdpSocket,
    target_addr: std::net::SocketAddr,
    speed: f64, // 1.0 = realtime, 2.0 = 2x speed, 0.0 = instant
) -> Result<usize> {
    if packets.is_empty() {
        return Ok(0);
    }

    info!(
        "Replaying {} packets to {} (speed: {}x)",
        packets.len(),
        target_addr,
        speed
    );

    let start = Instant::now();
    let mut sent = 0;

    for (i, packet) in packets.iter().enumerate() {
        // Wait for correct timing (if speed > 0)
        if speed > 0.0 && i > 0 {
            let target_elapsed = Duration::from_micros(
                (packet.timestamp_us as f64 / speed) as u64
            );
            let actual_elapsed = start.elapsed();

            if target_elapsed > actual_elapsed {
                tokio::time::sleep(target_elapsed - actual_elapsed).await;
            }
        }

        // Send packet
        socket
            .send_to(&packet.data, target_addr)
            .await
            .context("Failed to send RTP packet")?;

        sent += 1;
        if sent % 100 == 0 {
            debug!("Sent {}/{} packets", sent, packets.len());
        }
    }

    info!("Replay complete: {} packets sent", sent);
    Ok(sent)
}

/// Replay packets instantly (no timing) - useful for unit tests
pub async fn replay_instant(
    packets: &[RtpPacket],
    socket: &UdpSocket,
    target_addr: std::net::SocketAddr,
) -> Result<usize> {
    replay_to_socket(packets, socket, target_addr, 0.0).await
}

/// Get summary statistics about loaded packets
pub fn packet_stats(packets: &[RtpPacket]) -> PacketStats {
    if packets.is_empty() {
        return PacketStats::default();
    }

    let total_bytes: usize = packets.iter().map(|p| p.data.len()).sum();
    let duration_us = packets.last().map(|p| p.timestamp_us).unwrap_or(0);

    // Extract RTP payload types
    let mut payload_types = std::collections::HashSet::new();
    for p in packets {
        if p.data.len() >= 2 {
            let pt = p.data[1] & 0x7F;
            payload_types.insert(pt);
        }
    }

    PacketStats {
        packet_count: packets.len(),
        total_bytes,
        duration_ms: duration_us / 1000,
        payload_types: payload_types.into_iter().collect(),
    }
}

#[derive(Debug, Default)]
pub struct PacketStats {
    pub packet_count: usize,
    pub total_bytes: usize,
    pub duration_ms: u64,
    pub payload_types: Vec<u8>,
}

impl std::fmt::Display for PacketStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} packets, {} bytes, {}ms, PT={:?}",
            self.packet_count,
            self.total_bytes,
            self.duration_ms,
            self.payload_types
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === parse_udp_payload tests ===

    #[test]
    fn test_parse_udp_payload_too_short() {
        let short_data = vec![0u8; 30];
        assert!(parse_udp_payload(&short_data, 0).is_none());
    }

    #[test]
    fn test_parse_udp_payload_exact_minimum() {
        // 54 bytes is the minimum (14 eth + 20 ip + 8 udp + 12 rtp)
        let data = vec![0u8; 53];
        assert!(parse_udp_payload(&data, 0).is_none());
    }

    #[test]
    fn test_parse_udp_payload_non_ipv4() {
        // Ethernet header (14 bytes) + IPv6 packet
        let mut data = vec![0u8; 60];
        data[14] = 0x60; // IPv6 version
        assert!(parse_udp_payload(&data, 0).is_none());
    }

    #[test]
    fn test_parse_udp_payload_non_udp() {
        // Ethernet (14) + IPv4 with TCP (protocol 6)
        let mut data = vec![0u8; 60];
        data[14] = 0x45; // IPv4, 20-byte header
        data[14 + 9] = 6; // TCP protocol
        assert!(parse_udp_payload(&data, 0).is_none());
    }

    #[test]
    fn test_parse_udp_payload_valid_rtp() {
        // Build a valid Ethernet + IPv4 + UDP + RTP packet
        let mut data = vec![0u8; 60];

        // Ethernet header (14 bytes) - just zeros is fine
        // IPv4 header starts at offset 14
        data[14] = 0x45; // Version 4, IHL 5 (20 bytes)
        data[14 + 9] = 17; // UDP protocol

        // UDP header starts at offset 34 (14 + 20)
        data[34] = 0x27; // src port high byte (10000)
        data[35] = 0x10; // src port low byte
        data[36] = 0x27; // dst port high byte
        data[37] = 0x11; // dst port low byte
        data[38] = 0x00; // length high byte
        data[39] = 0x14; // length low byte (20 = 8 header + 12 payload)

        // RTP header starts at offset 42 (34 + 8)
        data[42] = 0x80; // RTP version 2
        data[43] = 0x00; // PT = 0 (PCMU)

        let result = parse_udp_payload(&data, 12345);
        assert!(result.is_some());

        let packet = result.unwrap();
        assert_eq!(packet.timestamp_us, 12345);
        assert_eq!(packet.src_port, 10000);
        assert_eq!(packet.dst_port, 10001);
        assert_eq!(packet.data.len(), 12); // RTP header only
    }

    #[test]
    fn test_parse_udp_payload_rtp_version_check() {
        let mut data = vec![0u8; 60];
        data[14] = 0x45;
        data[14 + 9] = 17;
        data[38] = 0x00;
        data[39] = 0x14;

        // RTP version 0 (not valid)
        data[42] = 0x00;
        assert!(parse_udp_payload(&data, 0).is_none());

        // RTP version 1 (not valid)
        data[42] = 0x40;
        assert!(parse_udp_payload(&data, 0).is_none());

        // RTP version 2 (valid)
        data[42] = 0x80;
        assert!(parse_udp_payload(&data, 0).is_some());

        // RTP version 3 (not valid)
        data[42] = 0xC0;
        assert!(parse_udp_payload(&data, 0).is_none());
    }

    #[test]
    fn test_parse_udp_payload_with_ip_options() {
        // IPv4 with options (IHL > 5)
        let mut data = vec![0u8; 80];
        data[14] = 0x46; // Version 4, IHL 6 (24 bytes)
        data[14 + 9] = 17;

        // UDP header at offset 38 (14 + 24)
        data[38] = 0x27;
        data[39] = 0x10;
        data[40] = 0x27;
        data[41] = 0x11;
        data[42] = 0x00;
        data[43] = 0x14;

        // RTP at offset 46 (38 + 8)
        data[46] = 0x80;
        data[47] = 0x00;

        let result = parse_udp_payload(&data, 0);
        assert!(result.is_some());
        assert_eq!(result.unwrap().data.len(), 12);
    }

    #[test]
    fn test_parse_udp_payload_extracts_payload_types() {
        let mut data = vec![0u8; 60];
        data[14] = 0x45;
        data[14 + 9] = 17;
        data[38] = 0x00;
        data[39] = 0x14;
        data[42] = 0x80;

        // PT = 0 (PCMU)
        data[43] = 0x00;
        let packet = parse_udp_payload(&data, 0).unwrap();
        assert_eq!(packet.data[1] & 0x7F, 0);

        // PT = 8 (PCMA)
        data[43] = 0x08;
        let packet = parse_udp_payload(&data, 0).unwrap();
        assert_eq!(packet.data[1] & 0x7F, 8);

        // PT with marker bit set
        data[43] = 0x80; // M=1, PT=0
        let packet = parse_udp_payload(&data, 0).unwrap();
        assert_eq!(packet.data[1] & 0x7F, 0);
    }

    // === packet_stats tests ===

    #[test]
    fn test_packet_stats_empty() {
        let stats = packet_stats(&[]);
        assert_eq!(stats.packet_count, 0);
        assert_eq!(stats.total_bytes, 0);
        assert_eq!(stats.duration_ms, 0);
        assert!(stats.payload_types.is_empty());
    }

    #[test]
    fn test_packet_stats_single_packet() {
        let packets = vec![RtpPacket {
            timestamp_us: 0,
            data: vec![0x80, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            src_port: 10000,
            dst_port: 20000,
        }];

        let stats = packet_stats(&packets);
        assert_eq!(stats.packet_count, 1);
        assert_eq!(stats.total_bytes, 12);
        assert_eq!(stats.duration_ms, 0);
        assert_eq!(stats.payload_types, vec![0]);
    }

    #[test]
    fn test_packet_stats_multiple_packets() {
        let packets = vec![
            RtpPacket {
                timestamp_us: 0,
                data: vec![0x80, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], // PT=0
                src_port: 10000,
                dst_port: 20000,
            },
            RtpPacket {
                timestamp_us: 20000,
                data: vec![0x80, 0x08, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], // PT=8
                src_port: 10000,
                dst_port: 20000,
            },
        ];

        let stats = packet_stats(&packets);
        assert_eq!(stats.packet_count, 2);
        assert_eq!(stats.total_bytes, 24);
        assert_eq!(stats.duration_ms, 20);
        assert!(stats.payload_types.contains(&0));
        assert!(stats.payload_types.contains(&8));
    }

    #[test]
    fn test_packet_stats_display() {
        let packets = vec![RtpPacket {
            timestamp_us: 10000000, // 10 seconds
            data: {
                let mut d = vec![0u8; 200];
                d[0] = 0x80; // RTP version 2
                d[1] = 0x00; // PT = 0
                d
            },
            src_port: 10000,
            dst_port: 20000,
        }];

        let stats = packet_stats(&packets);
        let display = format!("{}", stats);
        assert!(display.contains("1 packets"));
        assert!(display.contains("200 bytes"));
        assert!(display.contains("10000ms"));
    }

    // === RtpPacket tests ===

    #[test]
    fn test_rtp_packet_clone() {
        let packet = RtpPacket {
            timestamp_us: 12345,
            data: vec![0x80, 0x00, 1, 2, 3, 4],
            src_port: 5000,
            dst_port: 6000,
        };

        let cloned = packet.clone();
        assert_eq!(cloned.timestamp_us, packet.timestamp_us);
        assert_eq!(cloned.data, packet.data);
        assert_eq!(cloned.src_port, packet.src_port);
        assert_eq!(cloned.dst_port, packet.dst_port);
    }

    // === Helper to build valid ethernet/ip/udp/rtp packet ===

    fn build_test_packet(pt: u8, seq: u16, payload: &[u8]) -> Vec<u8> {
        let udp_len = 8 + 12 + payload.len(); // UDP header + RTP header + payload
        let mut data = vec![0u8; 14 + 20 + udp_len];

        // Ethernet (14 bytes) - zeros
        // IPv4 header
        data[14] = 0x45;
        data[14 + 9] = 17; // UDP

        // UDP header
        let udp_start = 34;
        data[udp_start] = 0x27;
        data[udp_start + 1] = 0x10; // src port 10000
        data[udp_start + 2] = 0x4E;
        data[udp_start + 3] = 0x20; // dst port 20000
        data[udp_start + 4] = ((udp_len >> 8) & 0xFF) as u8;
        data[udp_start + 5] = (udp_len & 0xFF) as u8;

        // RTP header
        let rtp_start = 42;
        data[rtp_start] = 0x80; // V=2
        data[rtp_start + 1] = pt;
        data[rtp_start + 2] = ((seq >> 8) & 0xFF) as u8;
        data[rtp_start + 3] = (seq & 0xFF) as u8;

        // RTP payload
        data[54..54 + payload.len()].copy_from_slice(payload);

        data
    }

    #[test]
    fn test_build_test_packet_helper() {
        let packet = build_test_packet(0, 100, &[0xFF; 160]);
        let parsed = parse_udp_payload(&packet, 0).unwrap();

        assert_eq!(parsed.src_port, 10000);
        assert_eq!(parsed.dst_port, 20000);
        assert_eq!(parsed.data[1] & 0x7F, 0); // PT
        assert_eq!(parsed.data.len(), 12 + 160); // RTP header + payload
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// parse_udp_payload never panics on any input
        #[test]
        fn parse_never_panics(data in proptest::collection::vec(any::<u8>(), 0..200)) {
            let _ = parse_udp_payload(&data, 0);
        }

        /// Timestamps are preserved correctly
        #[test]
        fn timestamp_preserved(ts in 0u64..u64::MAX) {
            let mut data = vec![0u8; 60];
            data[14] = 0x45;
            data[14 + 9] = 17;
            data[38] = 0x00;
            data[39] = 0x14;
            data[42] = 0x80;
            data[43] = 0x00;

            if let Some(packet) = parse_udp_payload(&data, ts) {
                prop_assert_eq!(packet.timestamp_us, ts);
            }
        }

        /// Port parsing is correct
        #[test]
        fn ports_parsed_correctly(src_port in 1024u16..65535u16, dst_port in 1024u16..65535u16) {
            let mut data = vec![0u8; 60];
            data[14] = 0x45;
            data[14 + 9] = 17;
            data[34] = (src_port >> 8) as u8;
            data[35] = (src_port & 0xFF) as u8;
            data[36] = (dst_port >> 8) as u8;
            data[37] = (dst_port & 0xFF) as u8;
            data[38] = 0x00;
            data[39] = 0x14;
            data[42] = 0x80;
            data[43] = 0x00;

            let packet = parse_udp_payload(&data, 0).unwrap();
            prop_assert_eq!(packet.src_port, src_port);
            prop_assert_eq!(packet.dst_port, dst_port);
        }

        /// Payload type is extracted correctly (lower 7 bits)
        #[test]
        fn payload_type_extracted(pt in 0u8..128u8) {
            let mut data = vec![0u8; 60];
            data[14] = 0x45;
            data[14 + 9] = 17;
            data[38] = 0x00;
            data[39] = 0x14;
            data[42] = 0x80;
            data[43] = pt; // PT without marker

            let packet = parse_udp_payload(&data, 0).unwrap();
            let extracted_pt = packet.data[1] & 0x7F;
            prop_assert_eq!(extracted_pt, pt);
        }

        /// packet_stats never panics
        #[test]
        fn stats_never_panics(
            count in 0usize..10,
            data_len in 12usize..100
        ) {
            let packets: Vec<RtpPacket> = (0..count)
                .map(|i| RtpPacket {
                    timestamp_us: i as u64 * 20000,
                    data: {
                        let mut d = vec![0u8; data_len];
                        if d.len() >= 2 {
                            d[0] = 0x80;
                            d[1] = 0x00;
                        }
                        d
                    },
                    src_port: 10000,
                    dst_port: 20000,
                })
                .collect();

            let _ = packet_stats(&packets);
        }
    }
}
