/// SIP message building utilities
/// Reference: RFC 3261 - SIP: Session Initiation Protocol

use rand::Rng;
use std::net::SocketAddr;

/// Generate a random Call-ID
pub fn generate_call_id(local_host: &str) -> String {
    let random: u64 = rand::thread_rng().gen();
    format!("{:016x}@{}", random, local_host)
}

/// Generate a random tag for From/To headers
pub fn generate_tag() -> String {
    let random: u32 = rand::thread_rng().gen();
    format!("{:08x}", random)
}

/// Generate a random branch parameter for Via header
/// Must start with "z9hG4bK" per RFC 3261
pub fn generate_branch() -> String {
    let random: u64 = rand::thread_rng().gen();
    format!("z9hG4bK{:016x}", random)
}

/// Build SIP INVITE request
///
/// If `external_addr` is provided (from STUN), use it for Contact header and SDP.
/// Otherwise, use `local_addr`.
pub fn build_invite(
    target_uri: &str,
    from_uri: &str,
    from_display: &str,
    call_id: &str,
    from_tag: &str,
    cseq: u32,
    local_addr: SocketAddr,
    rtp_port: u16,
    external_rtp_addr: Option<SocketAddr>,
) -> String {
    build_invite_internal(
        target_uri,
        from_uri,
        from_display,
        call_id,
        from_tag,
        cseq,
        local_addr,
        rtp_port,
        external_rtp_addr,
        None,
    )
}

/// Build SIP INVITE request with Authorization header for digest authentication
pub fn build_invite_with_auth(
    target_uri: &str,
    from_uri: &str,
    from_display: &str,
    call_id: &str,
    from_tag: &str,
    cseq: u32,
    local_addr: SocketAddr,
    rtp_port: u16,
    external_rtp_addr: Option<SocketAddr>,
    authorization: &str,
) -> String {
    build_invite_internal(
        target_uri,
        from_uri,
        from_display,
        call_id,
        from_tag,
        cseq,
        local_addr,
        rtp_port,
        external_rtp_addr,
        Some(authorization),
    )
}

/// Internal INVITE builder with optional authorization
fn build_invite_internal(
    target_uri: &str,
    from_uri: &str,
    from_display: &str,
    call_id: &str,
    from_tag: &str,
    cseq: u32,
    local_addr: SocketAddr,
    rtp_port: u16,
    external_rtp_addr: Option<SocketAddr>,
    authorization: Option<&str>,
) -> String {
    let branch = generate_branch();
    let local_ip = local_addr.ip();
    let local_port = local_addr.port();

    // Use external address for SDP if available (NAT traversal)
    let (sdp_ip, sdp_rtp_port) = match external_rtp_addr {
        Some(addr) => (addr.ip().to_string(), addr.port()),
        None => (local_ip.to_string(), rtp_port),
    };

    // SDP body for audio session
    let sdp = build_sdp(&sdp_ip, sdp_rtp_port);
    let content_length = sdp.len();

    // Build Authorization header if present
    let auth_header = match authorization {
        Some(auth) => format!("Authorization: {}\r\n", auth),
        None => String::new(),
    };

    format!(
        "INVITE {} SIP/2.0\r\n\
         Via: SIP/2.0/UDP {}:{};branch={};rport\r\n\
         Max-Forwards: 70\r\n\
         From: \"{}\" <{}>;tag={}\r\n\
         To: <{}>\r\n\
         Call-ID: {}\r\n\
         CSeq: {} INVITE\r\n\
         Contact: <sip:phonecheck@{}:{}>\r\n\
         {}Content-Type: application/sdp\r\n\
         Allow: INVITE, ACK, CANCEL, BYE\r\n\
         User-Agent: phonecheck/0.1.0\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {}",
        target_uri,
        local_ip,
        local_port,
        branch,
        from_display,
        from_uri,
        from_tag,
        target_uri,
        call_id,
        cseq,
        local_ip,
        local_port,
        auth_header,
        content_length,
        sdp
    )
}

/// Build SDP body for audio session
/// We offer G.711 u-law (PCMU) and A-law (PCMA)
fn build_sdp(local_ip: &str, rtp_port: u16) -> String {
    let session_id: u64 = rand::thread_rng().gen();
    let session_version: u64 = rand::thread_rng().gen();

    format!(
        "v=0\r\n\
         o=phonecheck {} {} IN IP4 {}\r\n\
         s=Phone Check Session\r\n\
         c=IN IP4 {}\r\n\
         t=0 0\r\n\
         m=audio {} RTP/AVP 0 8\r\n\
         a=rtpmap:0 PCMU/8000\r\n\
         a=rtpmap:8 PCMA/8000\r\n\
         a=ptime:20\r\n\
         a=recvonly\r\n",
        session_id, session_version, local_ip, local_ip, rtp_port
    )
}

/// Build ACK request (sent after receiving final response)
pub fn build_ack(
    target_uri: &str,
    from_uri: &str,
    from_display: &str,
    to_uri: &str,
    to_tag: Option<&str>,
    call_id: &str,
    from_tag: &str,
    cseq: u32,
    local_addr: SocketAddr,
    via_branch: &str,
) -> String {
    let local_ip = local_addr.ip();
    let local_port = local_addr.port();

    let to_header = match to_tag {
        Some(tag) => format!("<{}>;tag={}", to_uri, tag),
        None => format!("<{}>", to_uri),
    };

    format!(
        "ACK {} SIP/2.0\r\n\
         Via: SIP/2.0/UDP {}:{};branch={};rport\r\n\
         Max-Forwards: 70\r\n\
         From: \"{}\" <{}>;tag={}\r\n\
         To: {}\r\n\
         Call-ID: {}\r\n\
         CSeq: {} ACK\r\n\
         Content-Length: 0\r\n\
         \r\n",
        target_uri,
        local_ip,
        local_port,
        via_branch,
        from_display,
        from_uri,
        from_tag,
        to_header,
        call_id,
        cseq
    )
}

/// Build BYE request to end call
pub fn build_bye(
    target_uri: &str,
    from_uri: &str,
    from_display: &str,
    to_uri: &str,
    to_tag: Option<&str>,
    call_id: &str,
    from_tag: &str,
    cseq: u32,
    local_addr: SocketAddr,
) -> String {
    let branch = generate_branch();
    let local_ip = local_addr.ip();
    let local_port = local_addr.port();

    let to_header = match to_tag {
        Some(tag) => format!("<{}>;tag={}", to_uri, tag),
        None => format!("<{}>", to_uri),
    };

    format!(
        "BYE {} SIP/2.0\r\n\
         Via: SIP/2.0/UDP {}:{};branch={};rport\r\n\
         Max-Forwards: 70\r\n\
         From: \"{}\" <{}>;tag={}\r\n\
         To: {}\r\n\
         Call-ID: {}\r\n\
         CSeq: {} BYE\r\n\
         Content-Length: 0\r\n\
         \r\n",
        target_uri,
        local_ip,
        local_port,
        branch,
        from_display,
        from_uri,
        from_tag,
        to_header,
        call_id,
        cseq
    )
}

/// Parse SIP response status code from first line
pub fn parse_status_code(response: &str) -> Option<u16> {
    // First line format: "SIP/2.0 200 OK\r\n..."
    let first_line = response.lines().next()?;
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() >= 2 && parts[0].starts_with("SIP/") {
        parts[1].parse().ok()
    } else {
        None
    }
}

/// Extract To tag from response
pub fn extract_to_tag(response: &str) -> Option<String> {
    for line in response.lines() {
        if line.to_lowercase().starts_with("to:") {
            if let Some(tag_pos) = line.to_lowercase().find("tag=") {
                let tag_start = tag_pos + 4;
                let tag_value = &line[tag_start..];
                // Tag ends at ; or end of line
                let tag_end = tag_value
                    .find(|c: char| c == ';' || c == '>' || c == '\r' || c == '\n')
                    .unwrap_or(tag_value.len());
                return Some(tag_value[..tag_end].to_string());
            }
        }
    }
    None
}

/// Extract Via branch from response (for ACK)
pub fn extract_via_branch(response: &str) -> Option<String> {
    for line in response.lines() {
        if line.to_lowercase().starts_with("via:") {
            if let Some(branch_pos) = line.to_lowercase().find("branch=") {
                let branch_start = branch_pos + 7;
                let branch_value = &line[branch_start..];
                let branch_end = branch_value
                    .find(|c: char| c == ';' || c == ',' || c == '\r' || c == '\n')
                    .unwrap_or(branch_value.len());
                return Some(branch_value[..branch_end].to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_call_id() {
        let call_id = generate_call_id("192.168.1.1");
        assert!(call_id.contains('@'));
        assert!(call_id.ends_with("192.168.1.1"));
    }

    #[test]
    fn test_generate_call_id_unique() {
        let id1 = generate_call_id("host");
        let id2 = generate_call_id("host");
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_generate_branch() {
        let branch = generate_branch();
        assert!(branch.starts_with("z9hG4bK"));
    }

    #[test]
    fn test_generate_branch_rfc3261_compliant() {
        // RFC 3261 requires branch to start with "z9hG4bK"
        for _ in 0..10 {
            let branch = generate_branch();
            assert!(branch.starts_with("z9hG4bK"), "branch must start with magic cookie");
            assert!(branch.len() > 7, "branch must have random component");
        }
    }

    #[test]
    fn test_generate_tag() {
        let tag = generate_tag();
        assert_eq!(tag.len(), 8); // 8 hex digits
        assert!(tag.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_parse_status_code() {
        assert_eq!(parse_status_code("SIP/2.0 200 OK\r\n"), Some(200));
        assert_eq!(parse_status_code("SIP/2.0 100 Trying\r\n"), Some(100));
        assert_eq!(parse_status_code("SIP/2.0 404 Not Found\r\n"), Some(404));
        assert_eq!(parse_status_code("garbage"), None);
    }

    #[test]
    fn test_parse_status_code_all_classes() {
        // 1xx Provisional
        assert_eq!(parse_status_code("SIP/2.0 100 Trying\r\n"), Some(100));
        assert_eq!(parse_status_code("SIP/2.0 180 Ringing\r\n"), Some(180));
        assert_eq!(parse_status_code("SIP/2.0 183 Session Progress\r\n"), Some(183));

        // 2xx Success
        assert_eq!(parse_status_code("SIP/2.0 200 OK\r\n"), Some(200));

        // 3xx Redirection
        assert_eq!(parse_status_code("SIP/2.0 302 Moved Temporarily\r\n"), Some(302));

        // 4xx Client Error
        assert_eq!(parse_status_code("SIP/2.0 400 Bad Request\r\n"), Some(400));
        assert_eq!(parse_status_code("SIP/2.0 401 Unauthorized\r\n"), Some(401));
        assert_eq!(parse_status_code("SIP/2.0 403 Forbidden\r\n"), Some(403));
        assert_eq!(parse_status_code("SIP/2.0 404 Not Found\r\n"), Some(404));
        assert_eq!(parse_status_code("SIP/2.0 486 Busy Here\r\n"), Some(486));

        // 5xx Server Error
        assert_eq!(parse_status_code("SIP/2.0 500 Server Internal Error\r\n"), Some(500));
        assert_eq!(parse_status_code("SIP/2.0 503 Service Unavailable\r\n"), Some(503));

        // 6xx Global Failure
        assert_eq!(parse_status_code("SIP/2.0 603 Decline\r\n"), Some(603));
    }

    #[test]
    fn test_parse_status_code_edge_cases() {
        assert_eq!(parse_status_code(""), None);
        assert_eq!(parse_status_code("SIP/2.0\r\n"), None);
        assert_eq!(parse_status_code("SIP/2.0 abc OK\r\n"), None);
        assert_eq!(parse_status_code("HTTP/1.1 200 OK\r\n"), None);
    }

    #[test]
    fn test_extract_to_tag() {
        let response = "SIP/2.0 200 OK\r\n\
                        To: <sip:user@example.com>;tag=abc123\r\n\
                        From: <sip:caller@example.com>;tag=xyz789\r\n";
        assert_eq!(extract_to_tag(response), Some("abc123".to_string()));
    }

    #[test]
    fn test_extract_to_tag_missing() {
        let response = "SIP/2.0 200 OK\r\n\
                        To: <sip:user@example.com>\r\n";
        assert_eq!(extract_to_tag(response), None);
    }

    #[test]
    fn test_extract_to_tag_case_insensitive() {
        let response = "SIP/2.0 200 OK\r\n\
                        TO: <sip:user@example.com>;TAG=abc123\r\n";
        assert_eq!(extract_to_tag(response), Some("abc123".to_string()));
    }

    #[test]
    fn test_extract_via_branch() {
        let response = "SIP/2.0 200 OK\r\n\
                        Via: SIP/2.0/UDP 192.168.1.1:5060;branch=z9hG4bK12345;rport\r\n";
        assert_eq!(
            extract_via_branch(response),
            Some("z9hG4bK12345".to_string())
        );
    }

    #[test]
    fn test_extract_via_branch_at_end() {
        let response = "SIP/2.0 200 OK\r\n\
                        Via: SIP/2.0/UDP 192.168.1.1:5060;branch=z9hG4bK12345\r\n";
        assert_eq!(
            extract_via_branch(response),
            Some("z9hG4bK12345".to_string())
        );
    }

    #[test]
    fn test_build_invite_contains_required_headers() {
        let invite = build_invite(
            "sip:1234@example.com",
            "sip:caller@example.com",
            "Caller",
            "callid123@host",
            "fromtag",
            1,
            "192.168.1.1:5060".parse().unwrap(),
            10000,
            None,
        );

        assert!(invite.starts_with("INVITE sip:1234@example.com SIP/2.0\r\n"));
        assert!(invite.contains("Via:"));
        assert!(invite.contains("From:"));
        assert!(invite.contains("To:"));
        assert!(invite.contains("Call-ID:"));
        assert!(invite.contains("CSeq:"));
        assert!(invite.contains("Content-Type: application/sdp"));
        assert!(invite.contains("m=audio"));
        assert!(invite.contains("a=rtpmap:0 PCMU/8000"));
    }

    #[test]
    fn test_build_invite_with_external_addr() {
        let invite = build_invite(
            "sip:1234@example.com",
            "sip:caller@example.com",
            "Caller",
            "callid123@host",
            "fromtag",
            1,
            "192.168.1.1:5060".parse().unwrap(),
            10000,
            Some("203.0.113.50:10000".parse().unwrap()),
        );

        // SDP should contain the external IP, not the local IP
        assert!(invite.contains("c=IN IP4 203.0.113.50"));
        assert!(invite.contains("m=audio 10000"));
        // Local IP should still be in Via header
        assert!(invite.contains("Via: SIP/2.0/UDP 192.168.1.1:5060"));
    }

    #[test]
    fn test_build_invite_with_auth_contains_authorization() {
        let auth_header = r#"Digest username="user", realm="test", nonce="abc123", uri="sip:1234@example.com", response="xyz789""#;
        let invite = build_invite_with_auth(
            "sip:1234@example.com",
            "sip:caller@example.com",
            "Caller",
            "callid123@host",
            "fromtag",
            2, // CSeq 2 for retry after 401
            "192.168.1.1:5060".parse().unwrap(),
            10000,
            None,
            auth_header,
        );

        assert!(invite.starts_with("INVITE sip:1234@example.com SIP/2.0\r\n"));
        assert!(invite.contains("Authorization: Digest"));
        assert!(invite.contains("CSeq: 2 INVITE"));
        assert!(invite.contains("username=\"user\""));
        assert!(invite.contains("realm=\"test\""));
        // Should still have all other required headers
        assert!(invite.contains("Via:"));
        assert!(invite.contains("From:"));
        assert!(invite.contains("To:"));
        assert!(invite.contains("Call-ID:"));
        assert!(invite.contains("Content-Type: application/sdp"));
    }

    #[test]
    fn test_build_ack_contains_required_headers() {
        let ack = build_ack(
            "sip:1234@example.com",
            "sip:caller@example.com",
            "Caller",
            "sip:1234@example.com",
            Some("totag123"),
            "callid123@host",
            "fromtag",
            1,
            "192.168.1.1:5060".parse().unwrap(),
            "z9hG4bKbranch",
        );

        assert!(ack.starts_with("ACK sip:1234@example.com SIP/2.0\r\n"));
        assert!(ack.contains("Via:"));
        assert!(ack.contains("From:"));
        assert!(ack.contains("To:"));
        assert!(ack.contains("tag=totag123"));
        assert!(ack.contains("Call-ID:"));
        assert!(ack.contains("CSeq: 1 ACK"));
        assert!(ack.contains("Content-Length: 0"));
    }

    #[test]
    fn test_build_bye_contains_required_headers() {
        let bye = build_bye(
            "sip:1234@example.com",
            "sip:caller@example.com",
            "Caller",
            "sip:1234@example.com",
            Some("totag123"),
            "callid123@host",
            "fromtag",
            2,
            "192.168.1.1:5060".parse().unwrap(),
        );

        assert!(bye.starts_with("BYE sip:1234@example.com SIP/2.0\r\n"));
        assert!(bye.contains("Via:"));
        assert!(bye.contains("From:"));
        assert!(bye.contains("To:"));
        assert!(bye.contains("CSeq: 2 BYE"));
        assert!(bye.contains("Content-Length: 0"));
    }

    #[test]
    fn test_ack_without_to_tag() {
        let ack = build_ack(
            "sip:1234@example.com",
            "sip:caller@example.com",
            "Caller",
            "sip:1234@example.com",
            None,
            "callid123@host",
            "fromtag",
            1,
            "192.168.1.1:5060".parse().unwrap(),
            "z9hG4bKbranch",
        );

        // To header should not have tag
        assert!(ack.contains("To: <sip:1234@example.com>\r\n"));
        assert!(!ack.contains("To: <sip:1234@example.com>;tag="));
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// parse_status_code never panics
        #[test]
        fn parse_status_code_never_panics(input in ".*") {
            let _ = parse_status_code(&input);
        }

        /// extract_to_tag never panics
        #[test]
        fn extract_to_tag_never_panics(input in ".*") {
            let _ = extract_to_tag(&input);
        }

        /// extract_via_branch never panics
        #[test]
        fn extract_via_branch_never_panics(input in ".*") {
            let _ = extract_via_branch(&input);
        }

        /// Valid SIP status lines are parsed correctly
        #[test]
        fn valid_status_codes_parsed(code in 100u16..700u16) {
            let response = format!("SIP/2.0 {} Reason\r\n", code);
            prop_assert_eq!(parse_status_code(&response), Some(code));
        }

        /// Generated branches always start with magic cookie
        #[test]
        fn branches_have_magic_cookie(_seed in 0u32..1000u32) {
            let branch = generate_branch();
            prop_assert!(branch.starts_with("z9hG4bK"));
        }

        /// Generated tags are valid hex
        #[test]
        fn tags_are_hex(_seed in 0u32..1000u32) {
            let tag = generate_tag();
            prop_assert!(tag.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }
}

/// Kani formal verification proofs
#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    fn parse_status_never_panics() {
        let data: [u8; 32] = kani::any();
        if let Ok(s) = std::str::from_utf8(&data) {
            let _ = parse_status_code(s);
        }
    }

    #[kani::proof]
    fn extract_to_tag_never_panics() {
        let data: [u8; 64] = kani::any();
        if let Ok(s) = std::str::from_utf8(&data) {
            let _ = extract_to_tag(s);
        }
    }
}
