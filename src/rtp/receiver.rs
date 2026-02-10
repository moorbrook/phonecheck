use anyhow::{Context, Result};
use rand::Rng;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, trace, warn};

use super::g711::{G711Codec, G711Decoder};
use super::jitter::{BufferedPacket, JitterBuffer, JitterBufferConfig};
use super::resample::resample_to_16k;

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
    jitter_buffer: JitterBuffer,
}

impl RtpReceiver {
    /// Bind to a specific port (or 0 for auto-assign)
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
            jitter_buffer: JitterBuffer::new(JitterBufferConfig::default()),
        })
    }

    /// Create from an already-bound socket (avoids port race conditions)
    pub fn from_socket(socket: UdpSocket) -> Self {
        Self {
            socket,
            decoder: None,
            samples: Vec::new(),
            jitter_buffer: JitterBuffer::new(JitterBufferConfig::default()),
        }
    }

    pub fn local_port(&self) -> Result<u16> {
        Ok(self.socket.local_addr()?.port())
    }

    /// Discover public address/port for this socket using STUN
    pub async fn discover_public_address(&self, stun_server: &str) -> Result<std::net::SocketAddr> {
        crate::stun::discover_public_address_tokio(&self.socket, stun_server).await
    }

    /// Discover our CGNAT-mapped external address by sending a SIP OPTIONS
    /// from this socket to the SIP server. Under CGNAT, this reveals the
    /// external IP:port the server sees when we send from this socket.
    /// This is critical because CGNAT assigns different external ports per
    /// local socket, and STUN to a different server gives a useless mapping.
    pub async fn discover_cgnat_mapping(&self, sip_server: std::net::SocketAddr) -> Result<std::net::SocketAddr> {
        use std::time::Duration;
        let local_port = self.socket.local_addr()?.port();
        let branch = format!("z9hG4bK{:016x}", rand::thread_rng().gen::<u64>());
        let tag = format!("{:08x}", rand::thread_rng().gen::<u32>());
        let call_id = format!("{:016x}@cgnat-probe", rand::thread_rng().gen::<u64>());

        let options = format!(
            "OPTIONS sip:ping@{} SIP/2.0\r\n\
             Via: SIP/2.0/UDP 0.0.0.0:{};branch={};rport\r\n\
             From: <sip:probe@cgnat>;tag={}\r\n\
             To: <sip:ping@{}>\r\n\
             Call-ID: {}\r\n\
             CSeq: 1 OPTIONS\r\n\
             Max-Forwards: 70\r\n\
             Content-Length: 0\r\n\
             \r\n",
            sip_server, local_port, branch, tag, sip_server.ip(), call_id
        );

        self.socket.send_to(options.as_bytes(), sip_server).await?;

        let mut buf = [0u8; 4096];
        let (len, _) = tokio::time::timeout(Duration::from_secs(5), self.socket.recv_from(&mut buf))
            .await
            .map_err(|_| anyhow::anyhow!("CGNAT probe timeout"))?
            .map_err(|e| anyhow::anyhow!("CGNAT probe recv error: {}", e))?;

        let response = std::str::from_utf8(&buf[..len])
            .map_err(|_| anyhow::anyhow!("CGNAT probe: non-UTF8 response"))?;

        // Parse Via header for received= and rport=
        let mut received_ip: Option<std::net::IpAddr> = None;
        let mut rport: Option<u16> = None;
        for line in response.lines() {
            if line.to_lowercase().starts_with("via:") {
                for part in line.split(';') {
                    let part = part.trim();
                    if let Some(val) = part.strip_prefix("received=") {
                        received_ip = val.parse().ok();
                    } else if let Some(val) = part.strip_prefix("rport=") {
                        rport = val.trim().parse().ok();
                    }
                }
                break; // only first Via
            }
        }

        match (received_ip, rport) {
            (Some(ip), Some(port)) => Ok(std::net::SocketAddr::new(ip, port)),
            (Some(ip), None) => Err(anyhow::anyhow!("CGNAT probe: got received={} but no rport", ip)),
            _ => Err(anyhow::anyhow!("CGNAT probe: no received/rport in Via header")),
        }
    }

    /// Send empty RTP packets to punch through NAT
    pub async fn punch_nat(&self, remote_addr: std::net::SocketAddr) -> Result<()> {
        info!("Sending NAT hole-punch packets to {}", remote_addr);

        let mut packet = [0u8; 12];
        packet[0] = 0x80; // V=2, P=0, X=0, CC=0
        packet[1] = 0x00; // M=0, PT=0

        for i in 0..5 {
            packet[2] = 0;
            packet[3] = i;

            let ts = (i as u32) * 160; // 20ms @ 8kHz
            packet[4..8].copy_from_slice(&ts.to_be_bytes());
            packet[8..12].copy_from_slice(&[0x00, 0x00, 0x00, 0x01]);

            self.socket.send_to(&packet, remote_addr).await?;
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        info!("NAT hole-punch complete");
        Ok(())
    }

    /// Receive RTP packets for the specified duration with cancellation support.
    /// If `keepalive_target` is provided, continuously sends keepalive RTP packets
    /// to that address every 20ms to maintain NAT/CGNAT mappings.
    pub async fn receive_for_cancellable(
        &mut self,
        duration: Duration,
        cancel_token: CancellationToken,
    ) -> Result<bool> {
        self.receive_for_impl(duration, cancel_token, None).await
    }

    /// Like receive_for_cancellable but with continuous NAT keepalive packets
    pub async fn receive_for_with_keepalive(
        &mut self,
        duration: Duration,
        cancel_token: CancellationToken,
        keepalive_target: std::net::SocketAddr,
    ) -> Result<bool> {
        self.receive_for_impl(duration, cancel_token, Some(keepalive_target)).await
    }

    async fn receive_for_impl(
        &mut self,
        duration: Duration,
        cancel_token: CancellationToken,
        keepalive_target: Option<std::net::SocketAddr>,
    ) -> Result<bool> {
        let mut buf = [0u8; 2048];
        let deadline = tokio::time::Instant::now() + duration;
        let mut cancelled = false;
        let mut packet_count: u32 = 0;
        let mut first_packet_logged = false;
        let mut keepalive_seq: u16 = 100;
        let mut last_keepalive = tokio::time::Instant::now();
        let keepalive_interval = Duration::from_millis(20);

        // Build a reusable keepalive packet (minimal RTP header)
        let mut keepalive_pkt = [0u8; 12];
        keepalive_pkt[0] = 0x80; // V=2
        keepalive_pkt[1] = 0x00; // PT=0 (PCMU)
        keepalive_pkt[8..12].copy_from_slice(&[0x00, 0x00, 0x00, 0x01]); // SSRC

        loop {
            if cancel_token.is_cancelled() {
                debug!("RTP receive cancelled by shutdown signal");
                cancelled = true;
                break;
            }

            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }

            // Send keepalive if interval elapsed
            if let Some(target) = keepalive_target {
                if last_keepalive.elapsed() >= keepalive_interval {
                    keepalive_pkt[2..4].copy_from_slice(&keepalive_seq.to_be_bytes());
                    let ts = (keepalive_seq as u32) * 160;
                    keepalive_pkt[4..8].copy_from_slice(&ts.to_be_bytes());
                    let _ = self.socket.send_to(&keepalive_pkt, target).await;
                    keepalive_seq = keepalive_seq.wrapping_add(1);
                    last_keepalive = tokio::time::Instant::now();
                }
            }

            tokio::select! {
                result = timeout(remaining.min(Duration::from_millis(20)), self.socket.recv_from(&mut buf)) => {
                    match result {
                        Ok(Ok((len, addr))) => {
                            packet_count += 1;
                            if !first_packet_logged {
                                info!("First RTP packet received: {} bytes from {}", len, addr);
                                first_packet_logged = true;
                            }
                            if len >= 12 {
                                self.process_packet(&buf[..len]);
                            }
                        }
                        Ok(Err(e)) => {
                            warn!("RTP receive error: {}", e);
                        }
                        Err(_) => {}
                    }
                }
                _ = cancel_token.cancelled() => {
                    debug!("RTP receive cancelled by shutdown signal");
                    cancelled = true;
                    break;
                }
            }
        }

        info!("RTP receive done: {} packets received, {} i16 samples decoded", packet_count, self.samples.len());
        self.flush_jitter_buffer();
        Ok(!cancelled)
    }

    fn process_packet(&mut self, data: &[u8]) {
        if data.len() < 12 {
            return;
        }

        let version = (data[0] >> 6) & 0x03;
        if version != 2 {
            return;
        }

        let header = self.parse_header(data);
        if self.decoder.is_none() {
            self.decoder = G711Decoder::from_payload_type(header.payload_type);
            if self.decoder.is_none() {
                warn!("Unsupported RTP payload type: {}", header.payload_type);
                return;
            }
        }

        let payload_start = self.calculate_payload_offset(data);
        if payload_start >= data.len() {
            return;
        }

        self.jitter_buffer.insert(BufferedPacket {
            sequence: header.sequence,
            timestamp: header.timestamp,
            payload: data[payload_start..].to_vec(),
        });

        self.process_buffered_packets();
    }

    fn process_buffered_packets(&mut self) {
        while let Some(packet) = self.jitter_buffer.pop() {
            if let Some(ref decoder) = self.decoder {
                decoder.decode_into(&packet.payload, &mut self.samples);
            }
        }
    }

    fn flush_jitter_buffer(&mut self) {
        for packet in self.jitter_buffer.drain() {
            if let Some(ref decoder) = self.decoder {
                decoder.decode_into(&packet.payload, &mut self.samples);
            }
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
        let cc = data[0] & 0x0F;
        let has_extension = (data[0] & 0x10) != 0;
        let mut offset = 12 + (cc as usize * 4);
        if has_extension && data.len() > offset + 4 {
            let ext_length = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;
            offset += 4 + (ext_length * 4);
        }
        offset
    }

    /// Get accumulated samples as f32 (resampled to 16kHz)
    pub fn get_samples_f32(&self) -> Vec<f32> {
        if self.samples.is_empty() {
            return Vec::new();
        }

        let f32_samples: Vec<f32> = self.samples.iter().map(|&s| s as f32 / 32768.0).collect();
        resample_to_16k(&f32_samples)
    }
}

/// Parse RTP header from raw bytes (public for testing)
pub fn parse_rtp_header(data: &[u8]) -> Option<(u8, u16, u32, u32, usize)> {
    if data.len() < 12 { return None; }
    let version = (data[0] >> 6) & 0x03;
    if version != 2 { return None; }

    let payload_type = data[1] & 0x7F;
    let sequence = u16::from_be_bytes([data[2], data[3]]);
    let timestamp = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let ssrc = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);

    let cc = data[0] & 0x0F;
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

    #[tokio::test]
    async fn test_receive_for_cancellable_immediate_cancel() {
        let mut receiver = RtpReceiver::bind(0).await.unwrap();
        let cancel_token = CancellationToken::new();
        cancel_token.cancel();
        let result = receiver.receive_for_cancellable(Duration::from_secs(10), cancel_token).await;
        assert!(result.is_ok());
        assert!(!result.unwrap());
    }

    #[test]
    fn test_parse_rtp_header_valid() {
        let packet = [
            0x80, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0x12, 0x34, 0x56, 0x78, 0xAA, 0xBB,
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
}
