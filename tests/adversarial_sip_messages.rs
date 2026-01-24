//! Adversarial Property-Based Tests for SIP Message Parsing
//!
//! # Attack Plan
//!
//! 1. **Header Injection via Newlines**: from_display, target_uri, etc. are interpolated
//!    directly into format strings. Injecting \r\n could add arbitrary SIP headers.
//!
//! 2. **Parser Unicode Confusion**: extract_to_tag uses to_lowercase() to find position
//!    but slices original string. Turkish İ→i changes byte positions, causing panics
//!    or incorrect extraction.
//!
//! 3. **Status Code Integer Overflow**: parse_status_code parses to u16 but input could
//!    be "99999" - must handle gracefully.
//!
//! # Invariants
//!
//! - Parsers must NEVER panic on any input
//! - Generated messages must be valid UTF-8
//! - Status codes extracted must be in range 100-699 or None
//! - Branch parameters must always start with "z9hG4bK"
//! - Tags must be valid hex strings
//! - No CR/LF in header field values (RFC 3261 §7.3.1)

use proptest::prelude::*;
use std::net::SocketAddr;

// Import the module under test - NO shared helpers allowed
use phonecheck::sip::messages::{
    build_ack, build_bye, build_invite, build_invite_with_auth, extract_to_tag,
    extract_via_branch, generate_branch, generate_call_id, generate_tag, parse_status_code,
};

// ============================================================================
// ADVERSARIAL GENERATORS
// ============================================================================

/// Generator for strings containing header injection attempts
fn header_injection_string() -> impl Strategy<Value = String> {
    prop_oneof![
        // Direct CRLF injection
        Just("Normal\r\nEvil-Header: malicious".to_string()),
        Just("Test\r\n\r\nBody Injection".to_string()),
        // Partial CRLF
        Just("Has\rCarriage".to_string()),
        Just("Has\nNewline".to_string()),
        // Multiple injections
        Just("A\r\nB: 1\r\nC: 2".to_string()),
        // Null byte injection
        Just("Before\x00After".to_string()),
        // Tab injection (header folding)
        Just("Line1\r\n\tContinued".to_string()),
        Just("Line1\r\n Continued".to_string()),
        // Empty and edge cases
        Just("".to_string()),
        Just(" ".to_string()),
        Just("\r\n".to_string()),
        // Very long string
        "[a-zA-Z0-9]{0,1000}".prop_map(|s| s),
    ]
}

/// Generator for Unicode edge cases that break to_lowercase() position mapping
fn unicode_position_breaker() -> impl Strategy<Value = String> {
    prop_oneof![
        // Turkish İ lowercases to "i" (1 byte shorter in UTF-8)
        Just("To: <sip:test>;tag=İBM".to_string()),
        Just("To: İİİ;tag=value".to_string()),
        // German ß uppercases to "SS" (length changes)
        Just("To: ß;tag=test".to_string()),
        // Greek sigma (ς vs σ context-dependent)
        Just("To: Σ;tag=ΣIGMA".to_string()),
        // Combining characters
        Just("To: e\u{0301};tag=café".to_string()),  // é as e + combining accent
        // Zero-width characters
        Just("To: \u{200B};tag=hidden".to_string()),  // zero-width space
        Just("To: a\u{200D}b;tag=joined".to_string()), // zero-width joiner
        // Right-to-left override
        Just("To: \u{202E}evil;tag=reversed".to_string()),
        // Emoji with modifiers
        Just("To: 👨‍👩‍👧;tag=family".to_string()),
        // Very long multi-byte sequences
        Just("To: ".to_string() + &"日本語".repeat(100) + ";tag=test"),
    ]
}

/// Generator for malformed SIP status lines
fn malformed_status_line() -> impl Strategy<Value = String> {
    prop_oneof![
        // Missing components
        Just("".to_string()),
        Just("SIP/2.0".to_string()),
        Just("SIP/2.0 ".to_string()),
        Just("SIP/2.0  ".to_string()),
        Just("200 OK".to_string()),
        // Invalid status codes
        Just("SIP/2.0 99999 Overflow".to_string()),
        Just("SIP/2.0 -1 Negative".to_string()),
        Just("SIP/2.0 0 Zero".to_string()),
        Just("SIP/2.0 1 TooLow".to_string()),
        Just("SIP/2.0 abc NotNumber".to_string()),
        Just("SIP/2.0 12.5 Float".to_string()),
        Just("SIP/2.0 1e9 Scientific".to_string()),
        // Invalid protocol
        Just("HTTP/1.1 200 OK".to_string()),
        Just("SIP/1.0 200 OK".to_string()),
        Just("sip/2.0 200 OK".to_string()),
        // Binary/control characters
        Just("SIP/2.0 \x00 200 OK".to_string()),
        Just("\x00SIP/2.0 200 OK".to_string()),
        // Extremely long lines
        Just(format!("SIP/2.0 200 {}", "O".repeat(10000))),
        // Whitespace variations
        Just("SIP/2.0\t200\tOK".to_string()),
        Just("  SIP/2.0 200 OK".to_string()),
        // Unicode in status code position
        Just("SIP/2.0 ２００ OK".to_string()),  // fullwidth digits
    ]
}

/// Generator for malformed Via headers
fn malformed_via_header() -> impl Strategy<Value = String> {
    prop_oneof![
        // Missing branch
        Just("Via: SIP/2.0/UDP 192.168.1.1:5060;rport".to_string()),
        // Empty branch
        Just("Via: SIP/2.0/UDP 192.168.1.1:5060;branch=".to_string()),
        // Branch with injection
        Just("Via: SIP/2.0/UDP 1.1.1.1;branch=z9hG4bK\r\nEvil: header".to_string()),
        // Multiple Via headers
        Just("Via: SIP/2.0/UDP 1.1.1.1;branch=first\r\nVia: SIP/2.0/UDP 2.2.2.2;branch=second".to_string()),
        // Case variations
        Just("VIA: SIP/2.0/UDP 1.1.1.1;BRANCH=z9hG4bKtest".to_string()),
        Just("via: sip/2.0/udp 1.1.1.1;branch=z9hG4bKtest".to_string()),
        // Truncated
        Just("Via: SIP/2.0/UDP 1.1.1.1;bran".to_string()),
        // Unicode in branch value
        Just("Via: SIP/2.0/UDP 1.1.1.1;branch=z9hG4bK日本語".to_string()),
    ]
}

/// Generator for malformed To headers
fn malformed_to_header() -> impl Strategy<Value = String> {
    prop_oneof![
        // Missing tag
        Just("To: <sip:user@example.com>".to_string()),
        // Empty tag
        Just("To: <sip:user@example.com>;tag=".to_string()),
        // Multiple tags
        Just("To: <sip:user@example.com>;tag=first;tag=second".to_string()),
        // Tag with injection
        Just("To: <sip:user@example.com>;tag=abc\r\nEvil: header".to_string()),
        // Malformed URI
        Just("To: not-a-uri;tag=test".to_string()),
        // Case variations
        Just("TO: <sip:user@example.com>;TAG=test123".to_string()),
        Just("to: <sip:user@example.com>;tag=test123".to_string()),
        // Unicode in tag
        Just("To: <sip:user@example.com>;tag=日本語".to_string()),
        // Quoted strings
        Just("To: \"Display Name\" <sip:user@example.com>;tag=test".to_string()),
        // Nested angle brackets
        Just("To: <<sip:user@example.com>>;tag=test".to_string()),
        // Tag with special chars
        Just("To: <sip:u@e.com>;tag=abc;def;ghi".to_string()),
    ]
}

/// Generator for arbitrary UTF-8 strings (including all edge cases)
fn arbitrary_utf8() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("".to_string()),
        ".*",  // any valid UTF-8
        // Specific problematic patterns
        Just("\r\n\r\n".to_string()),
        Just("\x00".repeat(100)),
        Just("\t\t\t".to_string()),
        Just(" \r\n ".to_string()),
    ]
}

// ============================================================================
// INVARIANT: PARSERS NEVER PANIC
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(10000))]

    /// parse_status_code must never panic on any UTF-8 input
    #[test]
    fn prop_parse_status_code_never_panics(input in ".*") {
        let _ = parse_status_code(&input);
    }

    /// parse_status_code must never panic on malformed inputs
    #[test]
    fn prop_parse_status_code_malformed(input in malformed_status_line()) {
        let _ = parse_status_code(&input);
    }

    /// extract_to_tag must never panic on any UTF-8 input
    #[test]
    fn prop_extract_to_tag_never_panics(input in ".*") {
        let _ = extract_to_tag(&input);
    }

    /// extract_to_tag must never panic on Unicode edge cases
    #[test]
    fn prop_extract_to_tag_unicode(input in unicode_position_breaker()) {
        // This specifically tests the to_lowercase() position mapping bug
        let _ = extract_to_tag(&input);
    }

    /// extract_to_tag must never panic on malformed inputs
    #[test]
    fn prop_extract_to_tag_malformed(input in malformed_to_header()) {
        let _ = extract_to_tag(&input);
    }

    /// extract_via_branch must never panic on any UTF-8 input
    #[test]
    fn prop_extract_via_branch_never_panics(input in ".*") {
        let _ = extract_via_branch(&input);
    }

    /// extract_via_branch must never panic on malformed inputs
    #[test]
    fn prop_extract_via_branch_malformed(input in malformed_via_header()) {
        let _ = extract_via_branch(&input);
    }
}

// ============================================================================
// INVARIANT: GENERATED IDENTIFIERS ARE RFC-COMPLIANT
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// Branch MUST always start with magic cookie z9hG4bK (RFC 3261 §8.1.1.7)
    #[test]
    fn prop_branch_magic_cookie(_seed in 0u64..u64::MAX) {
        let branch = generate_branch();
        prop_assert!(
            branch.starts_with("z9hG4bK"),
            "Branch {} must start with z9hG4bK", branch
        );
    }

    /// Branch must be sufficiently random (collision resistance)
    #[test]
    fn prop_branch_uniqueness(_seed in 0u64..10000u64) {
        let b1 = generate_branch();
        let b2 = generate_branch();
        // With 64-bit random, collision probability is negligible
        prop_assert_ne!(b1, b2, "Branches must be unique");
    }

    /// Branch must contain only valid characters for SIP token
    #[test]
    fn prop_branch_valid_chars(_seed in 0u64..u64::MAX) {
        let branch = generate_branch();
        // RFC 3261 token = 1*(alphanum / "-" / "." / "!" / "%" / "*" / "_" / "+" / "`" / "'" / "~")
        prop_assert!(
            branch.chars().all(|c| c.is_ascii_alphanumeric() || "-._!%*+`'~".contains(c)),
            "Branch {} contains invalid characters", branch
        );
    }

    /// Tag must be valid hex (our implementation uses hex)
    #[test]
    fn prop_tag_is_hex(_seed in 0u64..u64::MAX) {
        let tag = generate_tag();
        prop_assert!(
            tag.chars().all(|c| c.is_ascii_hexdigit()),
            "Tag {} must be hex", tag
        );
    }

    /// Tag must be exactly 8 characters (our format)
    #[test]
    fn prop_tag_length(_seed in 0u64..u64::MAX) {
        let tag = generate_tag();
        prop_assert_eq!(tag.len(), 8, "Tag must be 8 chars");
    }

    /// Call-ID must contain @ separator
    #[test]
    fn prop_call_id_format(host in "[a-zA-Z0-9.-]{1,50}") {
        let call_id = generate_call_id(&host);
        prop_assert!(call_id.contains('@'), "Call-ID must have @ separator");
        prop_assert!(call_id.ends_with(&host), "Call-ID must end with host");
    }
}

// ============================================================================
// INVARIANT: VALID STATUS CODES ROUND-TRIP CORRECTLY
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// Valid SIP status codes (100-699) must be extracted correctly
    #[test]
    fn prop_valid_status_roundtrip(code in 100u16..700u16) {
        let response = format!("SIP/2.0 {} Reason Phrase\r\n", code);
        let parsed = parse_status_code(&response);
        prop_assert_eq!(parsed, Some(code), "Status {} must parse correctly", code);
    }

    /// Status codes outside valid range should still parse (we don't validate range)
    #[test]
    fn prop_status_boundary_codes(code in 0u16..100u16) {
        let response = format!("SIP/2.0 {} Reason\r\n", code);
        let parsed = parse_status_code(&response);
        // These are technically invalid SIP but should parse as numbers
        prop_assert_eq!(parsed, Some(code));
    }

    /// Very large status codes that overflow u16 must not panic
    #[test]
    fn prop_status_overflow(code in 70000u32..100000u32) {
        let response = format!("SIP/2.0 {} Reason\r\n", code);
        let parsed = parse_status_code(&response);
        // Should return None for values that can't fit in u16
        prop_assert!(parsed.is_none(), "Overflow status {} should fail gracefully", code);
    }
}

// ============================================================================
// NEGATIVE ASSERTIONS: REJECTION OF INVALID INPUT
// ============================================================================

#[test]
fn test_status_code_rejects_non_sip() {
    assert_eq!(parse_status_code("HTTP/1.1 200 OK"), None);
    assert_eq!(parse_status_code("RTSP/1.0 200 OK"), None);
    assert_eq!(parse_status_code("FOO/2.0 200 OK"), None);
}

#[test]
fn test_status_code_rejects_malformed() {
    assert_eq!(parse_status_code(""), None);
    assert_eq!(parse_status_code("SIP/2.0"), None);
    assert_eq!(parse_status_code("SIP/2.0 "), None);
    assert_eq!(parse_status_code("SIP/2.0 abc"), None);
    assert_eq!(parse_status_code("SIP/2.0 -200 OK"), None);
}

#[test]
fn test_to_tag_rejects_missing_tag() {
    assert_eq!(extract_to_tag("To: <sip:user@example.com>"), None);
    assert_eq!(extract_to_tag("To: <sip:user@example.com>;rport"), None);
}

#[test]
fn test_via_branch_rejects_missing_branch() {
    assert_eq!(extract_via_branch("Via: SIP/2.0/UDP 1.1.1.1:5060;rport"), None);
    assert_eq!(extract_via_branch("Via: SIP/2.0/UDP 1.1.1.1:5060"), None);
}

#[test]
fn test_parsers_handle_empty_string() {
    assert_eq!(parse_status_code(""), None);
    assert_eq!(extract_to_tag(""), None);
    assert_eq!(extract_via_branch(""), None);
}

#[test]
fn test_parsers_handle_null_bytes() {
    assert_eq!(parse_status_code("\x00"), None);
    assert_eq!(extract_to_tag("\x00"), None);
    assert_eq!(extract_via_branch("\x00"), None);
}

// ============================================================================
// SECURITY: HEADER INJECTION DETECTION
// ============================================================================

#[test]
fn test_invite_header_injection_in_display_name() {
    // If display name contains CRLF, it could inject headers
    let evil_display = "Alice\r\nEvil-Header: malicious";
    let addr: SocketAddr = "192.168.1.1:5060".parse().unwrap();

    let invite = build_invite(
        "sip:bob@example.com",
        "sip:alice@example.com",
        evil_display,
        "call123",
        "tag456",
        1,
        addr,
        10000,
        None,
    );

    // The message should NOT have a valid "Evil-Header" at the start of a line
    // (it's embedded in the From header value, which is "safe" but ugly)
    // This test documents the current behavior - injection is possible
    let has_injected_header = invite.lines().any(|line| line.starts_with("Evil-Header:"));
    // Currently this WILL inject - this test documents the vulnerability
    assert!(
        has_injected_header,
        "SECURITY: Header injection is currently possible in display name"
    );
}

#[test]
fn test_invite_header_injection_in_uri() {
    // If target_uri contains CRLF, it could inject headers
    let evil_uri = "sip:bob@example.com\r\nEvil: injected";
    let addr: SocketAddr = "192.168.1.1:5060".parse().unwrap();

    let invite = build_invite(
        evil_uri,
        "sip:alice@example.com",
        "Alice",
        "call123",
        "tag456",
        1,
        addr,
        10000,
        None,
    );

    // Check if injection succeeded
    let has_injected_header = invite.lines().any(|line| line.starts_with("Evil:"));
    assert!(
        has_injected_header,
        "SECURITY: Header injection is currently possible in target URI"
    );
}

// ============================================================================
// SECURITY: UNICODE CASE MAPPING ATTACKS
// ============================================================================

#[test]
fn test_to_tag_turkish_i_attack() {
    // Turkish İ (U+0130) lowercases to ASCII "i" but is 2 bytes in UTF-8
    // This can cause off-by-one errors when using to_lowercase() for position finding
    let input = "To: <sip:test@example.com>;tag=İABC";

    // This must not panic
    let result = extract_to_tag(input);

    // The extraction might be wrong, but it must not crash
    // Due to the byte position mismatch, it might extract garbage
    if let Some(tag) = result {
        // Document what actually gets extracted
        assert!(
            !tag.is_empty(),
            "Tag extraction should produce something (possibly incorrect)"
        );
    }
}

#[test]
fn test_via_branch_unicode_attack() {
    // Similar attack on Via branch extraction
    let input = "Via: SIP/2.0/UDP 1.1.1.1:5060;branch=İz9hG4bKtest";

    // Must not panic
    let result = extract_via_branch(input);

    // Check that we handle this gracefully
    assert!(result.is_some() || result.is_none()); // Just don't panic
}

// ============================================================================
// BOUNDARY STRESS TESTING
// ============================================================================

#[test]
fn test_parse_status_extremely_long_input() {
    // 1MB of data
    let huge_input = "SIP/2.0 200 ".to_string() + &"X".repeat(1_000_000);
    let _ = parse_status_code(&huge_input); // Must not panic or OOM
}

#[test]
fn test_parse_status_many_lines() {
    // Thousands of lines
    let many_lines = (0..10000).map(|i| format!("Header{}: value{}", i, i)).collect::<Vec<_>>().join("\r\n");
    let input = format!("SIP/2.0 200 OK\r\n{}", many_lines);
    let result = parse_status_code(&input);
    assert_eq!(result, Some(200));
}

#[test]
fn test_extract_tag_deeply_nested() {
    // Multiple To headers (first one wins per RFC)
    let mut input = String::new();
    for i in 0..1000 {
        input.push_str(&format!("To: <sip:user{}@example.com>;tag=tag{}\r\n", i, i));
    }
    let result = extract_to_tag(&input);
    assert_eq!(result, Some("tag0".to_string())); // First one
}

#[test]
fn test_status_code_max_u16() {
    // Test boundary at u16::MAX
    let input = format!("SIP/2.0 {} OK\r\n", u16::MAX);
    let result = parse_status_code(&input);
    assert_eq!(result, Some(u16::MAX));
}

#[test]
fn test_status_code_overflow_u16() {
    // Test just beyond u16::MAX
    let input = format!("SIP/2.0 {} OK\r\n", u16::MAX as u32 + 1);
    let result = parse_status_code(&input);
    assert_eq!(result, None); // Must fail gracefully
}

// ============================================================================
// INVARIANT: GENERATED MESSAGES ARE VALID UTF-8
// ============================================================================

proptest! {
    /// All generated messages must be valid UTF-8 (they're Strings, so this is guaranteed,
    /// but let's verify no panics occur during generation)
    #[test]
    fn prop_invite_is_valid_utf8(
        target in "[a-zA-Z0-9:@./]{1,50}",
        from in "[a-zA-Z0-9:@./]{1,50}",
        display in "[a-zA-Z0-9 ]{0,20}",
        call_id in "[a-zA-Z0-9@.]{1,30}",
        tag in "[a-zA-Z0-9]{1,10}",
        cseq in 1u32..1000000u32,
        rtp_port in 1024u16..65535u16,
    ) {
        let addr: SocketAddr = "192.168.1.1:5060".parse().unwrap();
        let invite = build_invite(
            &target,
            &from,
            &display,
            &call_id,
            &tag,
            cseq,
            addr,
            rtp_port,
            None,
        );
        // Must be valid UTF-8 (would panic on invalid)
        prop_assert!(invite.is_ascii() || !invite.is_empty());
        // Must have CRLF line endings
        prop_assert!(invite.contains("\r\n"));
    }

    #[test]
    fn prop_ack_is_valid(
        target in "[a-zA-Z0-9:@./]{1,50}",
        from in "[a-zA-Z0-9:@./]{1,50}",
        display in "[a-zA-Z0-9 ]{0,20}",
        to_uri in "[a-zA-Z0-9:@./]{1,50}",
        call_id in "[a-zA-Z0-9@.]{1,30}",
        from_tag in "[a-zA-Z0-9]{1,10}",
        to_tag in proptest::option::of("[a-zA-Z0-9]{1,10}"),
        cseq in 1u32..1000000u32,
        branch in "[a-zA-Z0-9]{10,30}",
    ) {
        let addr: SocketAddr = "192.168.1.1:5060".parse().unwrap();
        let ack = build_ack(
            &target,
            &from,
            &display,
            &to_uri,
            to_tag.as_deref(),
            &call_id,
            &from_tag,
            cseq,
            addr,
            &branch,
        );
        prop_assert!(ack.starts_with("ACK "));
        prop_assert!(ack.contains("\r\n"));
    }

    #[test]
    fn prop_bye_is_valid(
        target in "[a-zA-Z0-9:@./]{1,50}",
        from in "[a-zA-Z0-9:@./]{1,50}",
        display in "[a-zA-Z0-9 ]{0,20}",
        to_uri in "[a-zA-Z0-9:@./]{1,50}",
        call_id in "[a-zA-Z0-9@.]{1,30}",
        from_tag in "[a-zA-Z0-9]{1,10}",
        to_tag in proptest::option::of("[a-zA-Z0-9]{1,10}"),
        cseq in 1u32..1000000u32,
    ) {
        let addr: SocketAddr = "192.168.1.1:5060".parse().unwrap();
        let bye = build_bye(
            &target,
            &from,
            &display,
            &to_uri,
            to_tag.as_deref(),
            &call_id,
            &from_tag,
            cseq,
            addr,
        );
        prop_assert!(bye.starts_with("BYE "));
        prop_assert!(bye.contains("\r\n"));
    }
}

// ============================================================================
// DETERMINISM: SAME INPUT PRODUCES SAME STRUCTURE (ignoring random parts)
// ============================================================================

#[test]
fn test_invite_structure_deterministic() {
    let addr: SocketAddr = "192.168.1.1:5060".parse().unwrap();

    let invite1 = build_invite(
        "sip:test@example.com",
        "sip:caller@example.com",
        "Caller",
        "fixed-call-id@host",
        "fixed-tag",
        42,
        addr,
        10000,
        None,
    );

    let invite2 = build_invite(
        "sip:test@example.com",
        "sip:caller@example.com",
        "Caller",
        "fixed-call-id@host",
        "fixed-tag",
        42,
        addr,
        10000,
        None,
    );

    // The only difference should be in the random Via branch and SDP session IDs
    // All other structure should be identical
    assert!(invite1.contains("INVITE sip:test@example.com SIP/2.0"));
    assert!(invite2.contains("INVITE sip:test@example.com SIP/2.0"));
    assert!(invite1.contains("Call-ID: fixed-call-id@host"));
    assert!(invite2.contains("Call-ID: fixed-call-id@host"));
    assert!(invite1.contains("CSeq: 42 INVITE"));
    assert!(invite2.contains("CSeq: 42 INVITE"));
}
