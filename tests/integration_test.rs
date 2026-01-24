/// Integration tests for SIP/RTP call flow
/// Uses a mock SIP server to test the full call lifecycle

use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// RFC 3261 compliance: Validate basic SIP request structure
/// Returns (method, errors) - empty errors means valid
fn validate_sip_request(msg: &str) -> (Option<&str>, Vec<String>) {
    let mut errors = Vec::new();

    // Parse request line
    let first_line = msg.lines().next().unwrap_or("");
    let parts: Vec<&str> = first_line.split_whitespace().collect();

    if parts.len() < 3 {
        errors.push("Invalid request line: must have method, URI, and version".to_string());
        return (None, errors);
    }

    let method = parts[0];
    let _uri = parts[1];
    let version = parts[2];

    // RFC 3261: Version must be SIP/2.0
    if version != "SIP/2.0" {
        errors.push(format!("Invalid SIP version: expected 'SIP/2.0', got '{}'", version));
    }

    // RFC 3261 Section 8.1.1: Required headers for all requests
    let msg_lower = msg.to_lowercase();

    // Via header required
    if !msg_lower.contains("\nvia:") && !msg_lower.starts_with("via:") {
        errors.push("Missing required Via header".to_string());
    }

    // From header required
    if !msg_lower.contains("\nfrom:") {
        errors.push("Missing required From header".to_string());
    }

    // To header required
    if !msg_lower.contains("\nto:") {
        errors.push("Missing required To header".to_string());
    }

    // Call-ID header required
    if !msg_lower.contains("\ncall-id:") {
        errors.push("Missing required Call-ID header".to_string());
    }

    // CSeq header required
    if !msg_lower.contains("\ncseq:") {
        errors.push("Missing required CSeq header".to_string());
    }

    // CSeq must contain method name (e.g., "CSeq: 1 INVITE")
    for line in msg.lines() {
        if line.to_lowercase().starts_with("cseq:") {
            let cseq_parts: Vec<&str> = line.split_whitespace().collect();
            if cseq_parts.len() < 3 {
                errors.push("CSeq header must have sequence number and method".to_string());
            } else if cseq_parts[2] != method {
                errors.push(format!(
                    "CSeq method '{}' doesn't match request method '{}'",
                    cseq_parts[2], method
                ));
            }
        }
    }

    (Some(method), errors)
}

/// Mock SIP server that accepts INVITE, sends RTP audio, and handles BYE
struct MockSipServer {
    socket: UdpSocket,
    local_addr: SocketAddr,
    rtp_socket: UdpSocket,
    running: Arc<AtomicBool>,
    invite_received: Arc<AtomicBool>,
    bye_received: Arc<AtomicBool>,
    rtp_packets_sent: Arc<AtomicU32>,
}

impl MockSipServer {
    fn new() -> std::io::Result<Self> {
        // Bind SIP socket
        let socket = UdpSocket::bind("127.0.0.1:0")?;
        let local_addr = socket.local_addr()?;
        socket.set_read_timeout(Some(Duration::from_millis(100)))?;

        // Bind RTP socket
        let rtp_socket = UdpSocket::bind("127.0.0.1:0")?;

        Ok(Self {
            socket,
            local_addr,
            rtp_socket,
            running: Arc::new(AtomicBool::new(true)),
            invite_received: Arc::new(AtomicBool::new(false)),
            bye_received: Arc::new(AtomicBool::new(false)),
            rtp_packets_sent: Arc::new(AtomicU32::new(0)),
        })
    }

    fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    fn rtp_port(&self) -> u16 {
        self.rtp_socket.local_addr().unwrap().port()
    }

    fn invite_received(&self) -> bool {
        self.invite_received.load(Ordering::SeqCst)
    }

    fn bye_received(&self) -> bool {
        self.bye_received.load(Ordering::SeqCst)
    }

    fn rtp_packets_sent(&self) -> u32 {
        self.rtp_packets_sent.load(Ordering::SeqCst)
    }

    fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// Run the mock server loop in a separate thread
    fn run(self) -> thread::JoinHandle<Self> {
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut client_addr: Option<SocketAddr> = None;
            let mut client_rtp_addr: Option<SocketAddr> = None;
            let mut call_id = String::new();
            let mut from_tag = String::new();
            let mut cseq = 0u32;

            while self.running.load(Ordering::SeqCst) {
                // Try to receive SIP message
                match self.socket.recv_from(&mut buf) {
                    Ok((len, addr)) => {
                        let msg = String::from_utf8_lossy(&buf[..len]);

                        if msg.starts_with("INVITE ") {
                            self.invite_received.store(true, Ordering::SeqCst);
                            client_addr = Some(addr);

                            // Parse Call-ID
                            for line in msg.lines() {
                                if line.to_lowercase().starts_with("call-id:") {
                                    call_id = line.split(':').nth(1).unwrap_or("").trim().to_string();
                                }
                                if line.to_lowercase().starts_with("from:") && line.contains("tag=") {
                                    if let Some(tag_start) = line.find("tag=") {
                                        let tag_end = line[tag_start + 4..]
                                            .find(|c: char| c == ';' || c == '>' || c == '\r')
                                            .unwrap_or(line.len() - tag_start - 4);
                                        from_tag = line[tag_start + 4..tag_start + 4 + tag_end].to_string();
                                    }
                                }
                                if line.to_lowercase().starts_with("cseq:") {
                                    if let Some(num_str) = line.split_whitespace().nth(1) {
                                        cseq = num_str.parse().unwrap_or(1);
                                    }
                                }
                                // Parse RTP port from SDP
                                if line.starts_with("m=audio ") {
                                    if let Some(port_str) = line.split_whitespace().nth(1) {
                                        if let Ok(port) = port_str.parse::<u16>() {
                                            // Client is on same host as SIP
                                            client_rtp_addr = Some(SocketAddr::new(addr.ip(), port));
                                        }
                                    }
                                }
                            }

                            // Send 100 Trying
                            let trying = format!(
                                "SIP/2.0 100 Trying\r\n\
                                 Via: SIP/2.0/UDP {}:{};\r\n\
                                 Call-ID: {}\r\n\
                                 From: <sip:test@test>;tag={}\r\n\
                                 To: <sip:target@test>\r\n\
                                 CSeq: {} INVITE\r\n\
                                 Content-Length: 0\r\n\r\n",
                                addr.ip(),
                                addr.port(),
                                call_id,
                                from_tag,
                                cseq
                            );
                            let _ = self.socket.send_to(trying.as_bytes(), addr);

                            // Send 180 Ringing
                            let ringing = format!(
                                "SIP/2.0 180 Ringing\r\n\
                                 Via: SIP/2.0/UDP {}:{};\r\n\
                                 Call-ID: {}\r\n\
                                 From: <sip:test@test>;tag={}\r\n\
                                 To: <sip:target@test>;tag=mock123\r\n\
                                 CSeq: {} INVITE\r\n\
                                 Content-Length: 0\r\n\r\n",
                                addr.ip(),
                                addr.port(),
                                call_id,
                                from_tag,
                                cseq
                            );
                            let _ = self.socket.send_to(ringing.as_bytes(), addr);

                            // Small delay before 200 OK
                            thread::sleep(Duration::from_millis(50));

                            // Send 200 OK with SDP
                            let sdp = format!(
                                "v=0\r\n\
                                 o=mockserver 1 1 IN IP4 127.0.0.1\r\n\
                                 s=Mock Call\r\n\
                                 c=IN IP4 127.0.0.1\r\n\
                                 t=0 0\r\n\
                                 m=audio {} RTP/AVP 0\r\n\
                                 a=rtpmap:0 PCMU/8000\r\n",
                                self.rtp_port()
                            );

                            let ok = format!(
                                "SIP/2.0 200 OK\r\n\
                                 Via: SIP/2.0/UDP {}:{};branch=z9hG4bKtest\r\n\
                                 Call-ID: {}\r\n\
                                 From: <sip:test@test>;tag={}\r\n\
                                 To: <sip:target@test>;tag=mock123\r\n\
                                 CSeq: {} INVITE\r\n\
                                 Contact: <sip:mock@127.0.0.1:{}>\r\n\
                                 Content-Type: application/sdp\r\n\
                                 Content-Length: {}\r\n\r\n{}",
                                addr.ip(),
                                addr.port(),
                                call_id,
                                from_tag,
                                cseq,
                                self.local_addr.port(),
                                sdp.len(),
                                sdp
                            );
                            let _ = self.socket.send_to(ok.as_bytes(), addr);

                            // Start sending RTP packets
                            if let Some(rtp_addr) = client_rtp_addr {
                                self.send_rtp_audio(rtp_addr);
                            }
                        } else if msg.starts_with("ACK ") {
                            // ACK received, continue sending RTP
                        } else if msg.starts_with("BYE ") {
                            self.bye_received.store(true, Ordering::SeqCst);

                            // Parse BYE's CSeq (RFC 3261: response CSeq must match request)
                            let bye_cseq = msg
                                .lines()
                                .find(|l| l.to_lowercase().starts_with("cseq:"))
                                .and_then(|l| l.split_whitespace().nth(1))
                                .and_then(|s| s.parse::<u32>().ok())
                                .unwrap_or(2);

                            // Send 200 OK for BYE
                            let ok = format!(
                                "SIP/2.0 200 OK\r\n\
                                 Via: SIP/2.0/UDP {}:{};\r\n\
                                 Call-ID: {}\r\n\
                                 From: <sip:test@test>;tag={}\r\n\
                                 To: <sip:target@test>;tag=mock123\r\n\
                                 CSeq: {} BYE\r\n\
                                 Content-Length: 0\r\n\r\n",
                                addr.ip(),
                                addr.port(),
                                call_id,
                                from_tag,
                                bye_cseq
                            );
                            let _ = self.socket.send_to(ok.as_bytes(), addr);

                            // Stop running after BYE
                            self.running.store(false, Ordering::SeqCst);
                        }
                    }
                    Err(_) => {
                        // Timeout - continue loop
                    }
                }
            }

            self
        })
    }

    /// Send RTP packets with a simple tone (G.711 PCMU encoded)
    fn send_rtp_audio(&self, client_addr: SocketAddr) {
        // G.711 u-law silence (0xFF = silence/zero level)
        let silence_sample: u8 = 0xFF;

        let mut sequence: u16 = 0;
        let mut timestamp: u32 = 0;
        let ssrc: u32 = 0x12345678;

        // Send 50 packets (1 second of audio at 20ms per packet)
        for _ in 0..50 {
            if !self.running.load(Ordering::SeqCst) {
                break;
            }

            // Build RTP packet
            // RTP header: V=2, P=0, X=0, CC=0, M=0, PT=0 (PCMU)
            let mut packet = vec![
                0x80,                           // V=2, P=0, X=0, CC=0
                0x00,                           // M=0, PT=0 (PCMU)
                (sequence >> 8) as u8,          // Sequence high byte
                (sequence & 0xFF) as u8,        // Sequence low byte
                (timestamp >> 24) as u8,        // Timestamp
                (timestamp >> 16) as u8,
                (timestamp >> 8) as u8,
                timestamp as u8,
                (ssrc >> 24) as u8,             // SSRC
                (ssrc >> 16) as u8,
                (ssrc >> 8) as u8,
                ssrc as u8,
            ];

            // Add 160 samples (20ms at 8kHz)
            packet.extend(std::iter::repeat(silence_sample).take(160));

            let _ = self.rtp_socket.send_to(&packet, client_addr);
            self.rtp_packets_sent.fetch_add(1, Ordering::SeqCst);

            sequence = sequence.wrapping_add(1);
            timestamp = timestamp.wrapping_add(160);

            thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn test_mock_sip_server_lifecycle() {
    // Create and start mock server
    let server = MockSipServer::new().expect("Failed to create mock server");
    let server_addr = server.local_addr();

    assert!(!server.invite_received());
    assert!(!server.bye_received());
    assert_eq!(server.rtp_packets_sent(), 0);

    let handle = server.run();

    // Simulate a simple INVITE
    let client = UdpSocket::bind("127.0.0.1:0").unwrap();
    client.set_read_timeout(Some(Duration::from_secs(2))).unwrap();

    // Build SDP body first to calculate Content-Length
    let sdp_body = "v=0\r\n\
                    o=test 1 1 IN IP4 127.0.0.1\r\n\
                    s=Test\r\n\
                    c=IN IP4 127.0.0.1\r\n\
                    m=audio 20000 RTP/AVP 0\r\n";

    let invite = format!(
        "INVITE sip:target@test SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:5060;branch=z9hG4bKtest\r\n\
         From: <sip:test@test>;tag=client123\r\n\
         To: <sip:target@test>\r\n\
         Call-ID: testcall@127.0.0.1\r\n\
         CSeq: 1 INVITE\r\n\
         Content-Type: application/sdp\r\n\
         Content-Length: {}\r\n\r\n\
         {}",
        sdp_body.len(),
        sdp_body
    );

    client.send_to(invite.as_bytes(), server_addr).unwrap();

    // Wait for responses
    let mut buf = [0u8; 4096];
    let mut got_trying = false;
    let mut got_ringing = false;
    let mut got_ok = false;

    for _ in 0..10 {
        if let Ok((len, _)) = client.recv_from(&mut buf) {
            let response = String::from_utf8_lossy(&buf[..len]);
            if response.contains("100 Trying") {
                got_trying = true;
            }
            if response.contains("180 Ringing") {
                got_ringing = true;
            }
            if response.contains("200 OK") {
                got_ok = true;
                break;
            }
        }
    }

    assert!(got_trying, "Should have received 100 Trying");
    assert!(got_ringing, "Should have received 180 Ringing");
    assert!(got_ok, "Should have received 200 OK");

    // Send ACK
    let ack = "ACK sip:target@test SIP/2.0\r\n\
               Via: SIP/2.0/UDP 127.0.0.1:5060;branch=z9hG4bKtest\r\n\
               Call-ID: testcall@127.0.0.1\r\n\
               CSeq: 1 ACK\r\n\
               Content-Length: 0\r\n\r\n";
    client.send_to(ack.as_bytes(), server_addr).unwrap();

    // Wait a bit for RTP
    thread::sleep(Duration::from_millis(500));

    // Send BYE
    let bye = "BYE sip:target@test SIP/2.0\r\n\
               Via: SIP/2.0/UDP 127.0.0.1:5060;branch=z9hG4bKbye\r\n\
               Call-ID: testcall@127.0.0.1\r\n\
               CSeq: 2 BYE\r\n\
               Content-Length: 0\r\n\r\n";
    client.send_to(bye.as_bytes(), server_addr).unwrap();

    // Wait for BYE response
    for _ in 0..5 {
        if let Ok((len, _)) = client.recv_from(&mut buf) {
            let response = String::from_utf8_lossy(&buf[..len]);
            if response.contains("200 OK") && response.contains("BYE") {
                break;
            }
        }
    }

    // Wait for server to finish
    let server = handle.join().expect("Server thread panicked");

    // Verify call lifecycle
    assert!(server.invite_received(), "INVITE should have been received");
    assert!(server.bye_received(), "BYE should have been received");
    assert!(server.rtp_packets_sent() > 0, "Should have sent RTP packets");
}

#[test]
fn test_rtp_packet_format() {
    // Verify RTP packet structure
    let silence_sample: u8 = 0xFF;
    let sequence: u16 = 1234;
    let timestamp: u32 = 5678;
    let ssrc: u32 = 0xABCDEF01;

    let mut packet = vec![
        0x80,
        0x00,
        (sequence >> 8) as u8,
        (sequence & 0xFF) as u8,
        (timestamp >> 24) as u8,
        (timestamp >> 16) as u8,
        (timestamp >> 8) as u8,
        timestamp as u8,
        (ssrc >> 24) as u8,
        (ssrc >> 16) as u8,
        (ssrc >> 8) as u8,
        ssrc as u8,
    ];
    packet.extend(std::iter::repeat(silence_sample).take(160));

    // Verify header
    assert_eq!(packet[0] & 0xC0, 0x80); // Version 2
    assert_eq!(packet[1] & 0x7F, 0x00); // Payload type 0 (PCMU)
    assert_eq!(u16::from_be_bytes([packet[2], packet[3]]), sequence);
    assert_eq!(
        u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]),
        timestamp
    );
    assert_eq!(
        u32::from_be_bytes([packet[8], packet[9], packet[10], packet[11]]),
        ssrc
    );

    // Verify payload
    assert_eq!(packet.len(), 12 + 160);
    assert!(packet[12..].iter().all(|&b| b == silence_sample));
}

/// Mock SIP server that requires authentication (sends 401 challenge)
struct MockAuthSipServer {
    socket: UdpSocket,
    local_addr: SocketAddr,
    running: Arc<AtomicBool>,
    invite_count: Arc<AtomicU32>,
    auth_received: Arc<AtomicBool>,
}

impl MockAuthSipServer {
    fn new() -> std::io::Result<Self> {
        let socket = UdpSocket::bind("127.0.0.1:0")?;
        let local_addr = socket.local_addr()?;
        socket.set_read_timeout(Some(Duration::from_millis(100)))?;

        Ok(Self {
            socket,
            local_addr,
            running: Arc::new(AtomicBool::new(true)),
            invite_count: Arc::new(AtomicU32::new(0)),
            auth_received: Arc::new(AtomicBool::new(false)),
        })
    }

    fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    fn invite_count(&self) -> u32 {
        self.invite_count.load(Ordering::SeqCst)
    }

    fn auth_received(&self) -> bool {
        self.auth_received.load(Ordering::SeqCst)
    }

    fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// Run the auth-challenging server loop
    fn run(self) -> thread::JoinHandle<Self> {
        thread::spawn(move || {
            let mut buf = [0u8; 4096];

            while self.running.load(Ordering::SeqCst) {
                match self.socket.recv_from(&mut buf) {
                    Ok((len, addr)) => {
                        let msg = String::from_utf8_lossy(&buf[..len]);

                        if msg.starts_with("INVITE ") {
                            let count = self.invite_count.fetch_add(1, Ordering::SeqCst);

                            // Check if Authorization header is present
                            let has_auth = msg.lines().any(|l| {
                                l.to_lowercase().starts_with("authorization:")
                            });

                            if has_auth {
                                self.auth_received.store(true, Ordering::SeqCst);
                                // Send 200 OK (auth accepted)
                                let ok = "SIP/2.0 200 OK\r\n\
                                         Via: SIP/2.0/UDP 127.0.0.1:5060\r\n\
                                         Call-ID: test\r\n\
                                         CSeq: 1 INVITE\r\n\
                                         Content-Length: 0\r\n\r\n";
                                let _ = self.socket.send_to(ok.as_bytes(), addr);
                                self.running.store(false, Ordering::SeqCst);
                            } else if count == 0 {
                                // First INVITE without auth - send 401 challenge
                                let challenge = "SIP/2.0 401 Unauthorized\r\n\
                                                Via: SIP/2.0/UDP 127.0.0.1:5060\r\n\
                                                Call-ID: test\r\n\
                                                CSeq: 1 INVITE\r\n\
                                                WWW-Authenticate: Digest realm=\"test\", nonce=\"abc123\", algorithm=MD5\r\n\
                                                Content-Length: 0\r\n\r\n";
                                let _ = self.socket.send_to(challenge.as_bytes(), addr);
                            }
                        }
                    }
                    Err(_) => {
                        // Timeout - continue loop
                    }
                }
            }

            self
        })
    }
}

#[test]
fn test_mock_server_auth_challenge() {
    // Test that mock auth server sends 401 and recognizes Authorization header
    let server = MockAuthSipServer::new().expect("Failed to create mock auth server");
    let server_addr = server.local_addr();

    let handle = server.run();

    // Create client socket
    let client = UdpSocket::bind("127.0.0.1:0").unwrap();
    client.set_read_timeout(Some(Duration::from_secs(2))).unwrap();

    // Send INVITE without auth
    let invite1 = "INVITE sip:target@test SIP/2.0\r\n\
                   Via: SIP/2.0/UDP 127.0.0.1:5060\r\n\
                   Call-ID: test\r\n\
                   CSeq: 1 INVITE\r\n\
                   Content-Length: 0\r\n\r\n";
    client.send_to(invite1.as_bytes(), server_addr).unwrap();

    // Wait for 401 response
    let mut buf = [0u8; 4096];
    let mut got_401 = false;
    for _ in 0..5 {
        if let Ok((len, _)) = client.recv_from(&mut buf) {
            let response = String::from_utf8_lossy(&buf[..len]);
            if response.contains("401 Unauthorized") {
                got_401 = true;
                assert!(response.contains("WWW-Authenticate"));
                assert!(response.contains("nonce="));
                break;
            }
        }
    }
    assert!(got_401, "Should have received 401 challenge");

    // Send INVITE with auth
    let invite2 = "INVITE sip:target@test SIP/2.0\r\n\
                   Via: SIP/2.0/UDP 127.0.0.1:5060\r\n\
                   Call-ID: test\r\n\
                   CSeq: 2 INVITE\r\n\
                   Authorization: Digest username=\"user\", realm=\"test\", nonce=\"abc123\", response=\"dummy\"\r\n\
                   Content-Length: 0\r\n\r\n";
    client.send_to(invite2.as_bytes(), server_addr).unwrap();

    // Wait for 200 OK
    let mut got_ok = false;
    for _ in 0..5 {
        if let Ok((len, _)) = client.recv_from(&mut buf) {
            let response = String::from_utf8_lossy(&buf[..len]);
            if response.contains("200 OK") {
                got_ok = true;
                break;
            }
        }
    }
    assert!(got_ok, "Should have received 200 OK after auth");

    // Verify server state
    let server = handle.join().expect("Server thread panicked");
    assert_eq!(server.invite_count(), 2, "Should have received 2 INVITEs");
    assert!(server.auth_received(), "Should have received Authorization header");
}

// === SIP Validation Tests ===

#[test]
fn test_validate_sip_request_valid() {
    let valid_invite = "INVITE sip:target@test SIP/2.0\r\n\
                        Via: SIP/2.0/UDP 127.0.0.1:5060\r\n\
                        From: <sip:user@test>;tag=abc\r\n\
                        To: <sip:target@test>\r\n\
                        Call-ID: test123\r\n\
                        CSeq: 1 INVITE\r\n\
                        Content-Length: 0\r\n\r\n";

    let (method, errors) = validate_sip_request(valid_invite);
    assert_eq!(method, Some("INVITE"));
    assert!(errors.is_empty(), "Valid request should have no errors: {:?}", errors);
}

#[test]
fn test_validate_sip_request_missing_via() {
    let missing_via = "INVITE sip:target@test SIP/2.0\r\n\
                       From: <sip:user@test>\r\n\
                       To: <sip:target@test>\r\n\
                       Call-ID: test123\r\n\
                       CSeq: 1 INVITE\r\n\r\n";

    let (_, errors) = validate_sip_request(missing_via);
    assert!(errors.iter().any(|e| e.contains("Via")), "Should report missing Via");
}

#[test]
fn test_validate_sip_request_missing_call_id() {
    let missing_call_id = "INVITE sip:target@test SIP/2.0\r\n\
                           Via: SIP/2.0/UDP 127.0.0.1\r\n\
                           From: <sip:user@test>\r\n\
                           To: <sip:target@test>\r\n\
                           CSeq: 1 INVITE\r\n\r\n";

    let (_, errors) = validate_sip_request(missing_call_id);
    assert!(errors.iter().any(|e| e.contains("Call-ID")), "Should report missing Call-ID");
}

#[test]
fn test_validate_sip_request_cseq_method_mismatch() {
    let mismatch = "INVITE sip:target@test SIP/2.0\r\n\
                    Via: SIP/2.0/UDP 127.0.0.1\r\n\
                    From: <sip:user@test>\r\n\
                    To: <sip:target@test>\r\n\
                    Call-ID: test123\r\n\
                    CSeq: 1 BYE\r\n\r\n";

    let (method, errors) = validate_sip_request(mismatch);
    assert_eq!(method, Some("INVITE"));
    assert!(errors.iter().any(|e| e.contains("doesn't match")),
            "Should report CSeq method mismatch: {:?}", errors);
}

#[test]
fn test_validate_sip_request_wrong_version() {
    let wrong_version = "INVITE sip:target@test SIP/1.0\r\n\
                         Via: SIP/2.0/UDP 127.0.0.1\r\n\
                         From: <sip:user@test>\r\n\
                         To: <sip:target@test>\r\n\
                         Call-ID: test123\r\n\
                         CSeq: 1 INVITE\r\n\r\n";

    let (_, errors) = validate_sip_request(wrong_version);
    assert!(errors.iter().any(|e| e.contains("SIP version")), "Should report wrong version");
}
