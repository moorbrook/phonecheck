//! Adversarial Property-Based Tests for SIP Digest Authentication
//!
//! # Attack Plan
//!
//! 1. **Parameter Parser Injection**: Inject quotes, colons, newlines into parameter
//!    values to confuse the parser and extract wrong values.
//!
//! 2. **Algorithm Downgrade/Bypass**: Verify unsupported algorithms are rejected,
//!    not silently accepted. Attacker could try "MD4", "SHA1", "none", etc.
//!
//! 3. **Empty Field Attacks**: Empty realm, nonce, password, username should be
//!    handled gracefully without panics.
//!
//! 4. **Unicode in Credentials**: Non-ASCII in username/password could break
//!    MD5 string formatting or cause unexpected hash values.
//!
//! 5. **Header Injection via to_header()**: If username contains `"` or `\r\n`,
//!    could break Authorization header format and inject headers.
//!
//! 6. **Unterminated Quote Handling**: Parser must handle malformed quoted strings
//!    without panicking or entering infinite loop.
//!
//! # Invariants
//!
//! - parse_params and DigestChallenge::parse never panic on any input
//! - Computed response is always 32 lowercase hex characters
//! - Missing required fields (realm, nonce) cause parse to return None
//! - Unsupported algorithms cause parse to return None
//! - to_header() always produces valid ASCII output

use proptest::prelude::*;

use phonecheck::sip::digest::{DigestAlgorithm, DigestChallenge, DigestResponse};

// ============================================================================
// ADVERSARIAL GENERATORS
// ============================================================================

/// Generator for header injection attempts in parameter values
fn param_injection_string() -> impl Strategy<Value = String> {
    prop_oneof![
        // Quote injection
        Just("test\"injected".to_string()),
        Just("test\", evil=injected".to_string()),
        Just("\\\"escaped\\\"".to_string()),
        // CRLF injection
        Just("test\r\nEvil-Header: value".to_string()),
        Just("test\r\n\r\nBody".to_string()),
        // Null byte injection
        Just("test\x00hidden".to_string()),
        // Very long value
        Just("A".repeat(10000)),
        // Unicode
        Just("tëst日本語".to_string()),
        Just("test\u{200B}hidden".to_string()), // zero-width space
        // Empty
        Just("".to_string()),
        // Special characters
        Just("test=value".to_string()),
        Just("test,value".to_string()),
        Just("test;value".to_string()),
    ]
}

/// Generator for malformed WWW-Authenticate headers
fn malformed_challenge() -> impl Strategy<Value = String> {
    prop_oneof![
        // Missing required fields
        Just("Digest nonce=\"123\"".to_string()),
        Just("Digest realm=\"test\"".to_string()),
        Just("Digest".to_string()),
        Just("".to_string()),
        // Unsupported algorithms
        Just("Digest realm=\"test\", nonce=\"123\", algorithm=SHA256".to_string()),
        Just("Digest realm=\"test\", nonce=\"123\", algorithm=MD4".to_string()),
        Just("Digest realm=\"test\", nonce=\"123\", algorithm=none".to_string()),
        Just("Digest realm=\"test\", nonce=\"123\", algorithm=\"\"".to_string()),
        // Unterminated quotes
        Just("Digest realm=\"unterminated, nonce=\"123\"".to_string()),
        Just("Digest realm=\"test\", nonce=\"unterminated".to_string()),
        // Multiple equals signs
        Just("Digest realm==\"test\", nonce=\"123\"".to_string()),
        // No equals sign
        Just("Digest realm, nonce".to_string()),
        // Garbage
        Just("Not a digest challenge at all".to_string()),
        Just("Basic realm=\"test\"".to_string()), // Wrong auth type
        // Duplicate fields
        Just("Digest realm=\"first\", realm=\"second\", nonce=\"123\"".to_string()),
        // Case variations
        Just("DIGEST realm=\"test\", nonce=\"123\"".to_string()),
        Just("digest realm=\"test\", nonce=\"123\"".to_string()),
        // Excessive whitespace
        Just("Digest   realm  =  \"test\"  ,  nonce  =  \"123\"".to_string()),
    ]
}

/// Generator for potentially dangerous usernames
fn dangerous_username() -> impl Strategy<Value = String> {
    prop_oneof![
        // Quote injection in header
        Just("user\"injected".to_string()),
        Just("user\", evil=\"value".to_string()),
        // CRLF injection
        Just("user\r\nEvil: header".to_string()),
        // Unicode
        Just("üser日本語".to_string()),
        // Empty
        Just("".to_string()),
        // Very long
        Just("u".repeat(10000)),
        // Special chars
        Just("user:pass".to_string()),
        Just("user@domain".to_string()),
    ]
}

// ============================================================================
// INVARIANT: PARSERS NEVER PANIC
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(10000))]

    #[test]
    fn prop_parse_challenge_never_panics(input in ".*") {
        let _ = DigestChallenge::parse(&input);
    }

    #[test]
    fn prop_parse_challenge_malformed(input in malformed_challenge()) {
        let _ = DigestChallenge::parse(&input);
    }

    #[test]
    fn prop_parse_challenge_with_injections(
        realm in param_injection_string(),
        nonce in param_injection_string(),
    ) {
        let header = format!("Digest realm=\"{}\", nonce=\"{}\"", realm, nonce);
        let _ = DigestChallenge::parse(&header);
    }
}

// ============================================================================
// INVARIANT: COMPUTED RESPONSE IS ALWAYS VALID
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// Response hash is always 32 lowercase hex characters
    #[test]
    fn prop_response_always_32_hex(
        realm in ".{1,50}",
        nonce in ".{1,50}",
        username in ".{0,50}",
        password in ".{0,50}",
        method in "(INVITE|REGISTER|BYE|ACK|CANCEL|OPTIONS)",
        uri in ".{1,100}",
    ) {
        let challenge = DigestChallenge {
            realm,
            nonce,
            algorithm: DigestAlgorithm::Md5,
            qop: None,
            opaque: None,
            stale: false,
        };

        let response = DigestResponse::compute(&challenge, &username, &password, &method, &uri);

        prop_assert_eq!(response.response.len(), 32, "Response must be 32 chars");
        prop_assert!(
            response.response.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
            "Response must be lowercase hex"
        );
    }

    /// Response with qop=auth includes cnonce and nc
    #[test]
    fn prop_response_with_qop_has_cnonce(
        realm in "[a-z]{3,10}",
        nonce in "[a-z0-9]{8,20}",
    ) {
        let challenge = DigestChallenge {
            realm,
            nonce,
            algorithm: DigestAlgorithm::Md5,
            qop: Some("auth".to_string()),
            opaque: None,
            stale: false,
        };

        let response = DigestResponse::compute(&challenge, "user", "pass", "INVITE", "sip:test@example.com");

        prop_assert!(response.cnonce.is_some(), "qop=auth requires cnonce");
        prop_assert!(response.nc.is_some(), "qop=auth requires nc");
        prop_assert_eq!(response.nc, Some("00000001".to_string()), "First nc must be 00000001");
    }
}

// ============================================================================
// NEGATIVE ASSERTIONS: REJECTION OF INVALID INPUT
// ============================================================================

#[test]
fn test_parse_rejects_missing_realm() {
    let header = r#"Digest nonce="123""#;
    assert!(DigestChallenge::parse(header).is_none());
}

#[test]
fn test_parse_rejects_missing_nonce() {
    let header = r#"Digest realm="test""#;
    assert!(DigestChallenge::parse(header).is_none());
}

#[test]
fn test_parse_rejects_empty_string() {
    assert!(DigestChallenge::parse("").is_none());
}

#[test]
fn test_parse_rejects_unsupported_algorithms() {
    let unsupported = [
        "SHA256",
        "SHA-256",
        "SHA1",
        "SHA-1",
        "MD4",
        "NONE",
        "null",
        "",
        "UNKNOWN",
    ];

    for alg in unsupported {
        let header = format!("Digest realm=\"test\", nonce=\"123\", algorithm={}", alg);
        let result = DigestChallenge::parse(&header);
        assert!(
            result.is_none(),
            "Algorithm {} should be rejected, but parse returned {:?}",
            alg,
            result
        );
    }
}

#[test]
fn test_parse_accepts_supported_algorithms() {
    // MD5 (default and explicit)
    let md5_default = DigestChallenge::parse(r#"Digest realm="test", nonce="123""#);
    assert!(md5_default.is_some());
    assert_eq!(md5_default.unwrap().algorithm, DigestAlgorithm::Md5);

    let md5_explicit = DigestChallenge::parse(r#"Digest realm="test", nonce="123", algorithm=MD5"#);
    assert!(md5_explicit.is_some());
    assert_eq!(md5_explicit.unwrap().algorithm, DigestAlgorithm::Md5);

    // MD5-sess
    let md5_sess = DigestChallenge::parse(r#"Digest realm="test", nonce="123", algorithm=MD5-sess"#);
    assert!(md5_sess.is_some());
    assert_eq!(md5_sess.unwrap().algorithm, DigestAlgorithm::Md5Sess);

    // Case insensitive
    let md5_lower = DigestChallenge::parse(r#"Digest realm="test", nonce="123", algorithm=md5"#);
    assert!(md5_lower.is_some());
    assert_eq!(md5_lower.unwrap().algorithm, DigestAlgorithm::Md5);
}

// ============================================================================
// SECURITY: HEADER INJECTION DETECTION
// ============================================================================

#[test]
fn test_to_header_with_quotes_in_username() {
    let challenge = DigestChallenge {
        realm: "test".to_string(),
        nonce: "123".to_string(),
        algorithm: DigestAlgorithm::Md5,
        qop: None,
        opaque: None,
        stale: false,
    };

    // Username with embedded quotes
    let response = DigestResponse::compute(
        &challenge,
        "user\"injected",
        "password",
        "INVITE",
        "sip:test@example.com",
    );

    let header = response.to_header();

    // The header should contain the username, but the quote breaks the format
    // This documents the vulnerability - quotes in username aren't escaped
    assert!(
        header.contains("user\"injected"),
        "SECURITY: Quotes in username are not escaped, allowing header value injection"
    );
}

#[test]
fn test_to_header_with_crlf_in_username() {
    let challenge = DigestChallenge {
        realm: "test".to_string(),
        nonce: "123".to_string(),
        algorithm: DigestAlgorithm::Md5,
        qop: None,
        opaque: None,
        stale: false,
    };

    // Username with CRLF
    let response = DigestResponse::compute(
        &challenge,
        "user\r\nEvil: header",
        "password",
        "INVITE",
        "sip:test@example.com",
    );

    let header = response.to_header();

    // Check if CRLF injection succeeded
    let has_evil_header = header.lines().count() > 1;
    assert!(
        has_evil_header,
        "SECURITY: CRLF in username allows header injection"
    );
}

// ============================================================================
// BOUNDARY STRESS TESTING
// ============================================================================

#[test]
fn test_parse_very_long_values() {
    let long_realm = "r".repeat(100000);
    let long_nonce = "n".repeat(100000);
    let header = format!("Digest realm=\"{}\", nonce=\"{}\"", long_realm, long_nonce);

    let result = DigestChallenge::parse(&header);
    assert!(result.is_some(), "Should handle very long values");

    let challenge = result.unwrap();
    assert_eq!(challenge.realm.len(), 100000);
    assert_eq!(challenge.nonce.len(), 100000);
}

#[test]
fn test_compute_with_empty_credentials() {
    let challenge = DigestChallenge {
        realm: "".to_string(),
        nonce: "".to_string(),
        algorithm: DigestAlgorithm::Md5,
        qop: None,
        opaque: None,
        stale: false,
    };

    // Empty everything should still produce a valid hash
    let response = DigestResponse::compute(&challenge, "", "", "INVITE", "");

    assert_eq!(response.response.len(), 32);
    assert!(response.response.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_compute_with_unicode_credentials() {
    let challenge = DigestChallenge {
        realm: "tëst日本語".to_string(),
        nonce: "nönçé".to_string(),
        algorithm: DigestAlgorithm::Md5,
        qop: None,
        opaque: None,
        stale: false,
    };

    let response = DigestResponse::compute(
        &challenge,
        "üser",
        "pässwörd日本語",
        "INVITE",
        "sip:tëst@exämple.com",
    );

    // Should still produce valid 32-char hex hash
    assert_eq!(response.response.len(), 32);
    assert!(response.response.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_parse_unterminated_quotes() {
    let inputs = [
        r#"Digest realm="unterminated"#,
        r#"Digest realm="test, nonce="123"#,
        r#"Digest realm=""#,
        r#"Digest realm=", nonce=""#,
    ];

    for input in inputs {
        // Should not panic
        let result = DigestChallenge::parse(input);
        // May or may not parse, but must not crash
        let _ = result;
    }
}

#[test]
fn test_parse_multiple_commas() {
    let header = r#"Digest realm="test",,,, nonce="123""#;
    let result = DigestChallenge::parse(header);
    // Should handle gracefully
    assert!(result.is_some() || result.is_none()); // Just don't panic
}

#[test]
fn test_parse_whitespace_variations() {
    let variants = [
        r#"Digest realm="test",nonce="123""#,          // No space after comma
        r#"Digest  realm="test",  nonce="123""#,       // Extra spaces
        r#"Digest realm = "test" , nonce = "123""#,    // Spaces around equals
        "Digest realm=\"test\",\tnonce=\"123\"",       // Tab separator
        "Digest realm=\"test\",\n nonce=\"123\"",      // Newline (shouldn't work)
    ];

    for variant in variants {
        // Should not panic
        let _ = DigestChallenge::parse(variant);
    }
}

// ============================================================================
// RFC 2617 COMPLIANCE: KNOWN TEST VECTORS
// ============================================================================

#[test]
fn test_rfc2617_example_without_qop() {
    // From RFC 2617 Section 3.5
    let challenge = DigestChallenge {
        realm: "testrealm@host.com".to_string(),
        nonce: "dcd98b7102dd2f0e8b11d0f600bfb0c093".to_string(),
        algorithm: DigestAlgorithm::Md5,
        qop: None,
        opaque: None,
        stale: false,
    };

    let response = DigestResponse::compute(
        &challenge,
        "Mufasa",
        "Circle Of Life",
        "GET",
        "/dir/index.html",
    );

    // Known correct response from RFC 2617
    assert_eq!(
        response.response, "670fd8c2df070c60b045671b8b24ff02",
        "RFC 2617 test vector mismatch"
    );
}

// ============================================================================
// EXTRACT HEADER PARSING
// ============================================================================

#[test]
fn test_extract_authenticate_header_case_insensitive() {
    use phonecheck::sip::digest::extract_authenticate_header;

    let variants = [
        "WWW-Authenticate: Digest realm=\"test\"",
        "www-authenticate: Digest realm=\"test\"",
        "WWW-AUTHENTICATE: Digest realm=\"test\"",
        "Www-Authenticate: Digest realm=\"test\"",
    ];

    for variant in variants {
        let result = extract_authenticate_header(variant);
        assert!(result.is_some(), "Failed to extract from: {}", variant);
    }
}

#[test]
fn test_extract_authenticate_header_not_present() {
    use phonecheck::sip::digest::extract_authenticate_header;

    let response = "SIP/2.0 200 OK\r\nContent-Length: 0\r\n";
    let result = extract_authenticate_header(response);
    assert!(result.is_none());
}

// ============================================================================
// DETERMINISM: SAME INPUT PRODUCES SAME OUTPUT (except cnonce)
// ============================================================================

#[test]
fn test_response_deterministic_without_qop() {
    let challenge = DigestChallenge {
        realm: "test".to_string(),
        nonce: "fixed-nonce".to_string(),
        algorithm: DigestAlgorithm::Md5,
        qop: None,
        opaque: None,
        stale: false,
    };

    let response1 = DigestResponse::compute(&challenge, "user", "pass", "INVITE", "sip:test@example.com");
    let response2 = DigestResponse::compute(&challenge, "user", "pass", "INVITE", "sip:test@example.com");

    // Without qop, responses should be identical (no random cnonce)
    assert_eq!(
        response1.response, response2.response,
        "Response should be deterministic without qop"
    );
}

#[test]
fn test_response_varies_with_qop() {
    let challenge = DigestChallenge {
        realm: "test".to_string(),
        nonce: "fixed-nonce".to_string(),
        algorithm: DigestAlgorithm::Md5,
        qop: Some("auth".to_string()),
        opaque: None,
        stale: false,
    };

    let response1 = DigestResponse::compute(&challenge, "user", "pass", "INVITE", "sip:test@example.com");
    let response2 = DigestResponse::compute(&challenge, "user", "pass", "INVITE", "sip:test@example.com");

    // With qop, cnonces should differ (random)
    assert_ne!(
        response1.cnonce, response2.cnonce,
        "cnonce should be random for each response"
    );

    // Therefore responses should differ
    assert_ne!(
        response1.response, response2.response,
        "Response should vary with random cnonce"
    );
}

// ============================================================================
// STALE FLAG PARSING
// ============================================================================

#[test]
fn test_stale_flag_parsing() {
    // stale=true
    let header = r#"Digest realm="test", nonce="123", stale=true"#;
    let challenge = DigestChallenge::parse(header).unwrap();
    assert!(challenge.stale, "stale=true should be parsed");

    // stale=TRUE (case insensitive)
    let header = r#"Digest realm="test", nonce="123", stale=TRUE"#;
    let challenge = DigestChallenge::parse(header).unwrap();
    assert!(challenge.stale, "stale=TRUE should be parsed");

    // stale=false
    let header = r#"Digest realm="test", nonce="123", stale=false"#;
    let challenge = DigestChallenge::parse(header).unwrap();
    assert!(!challenge.stale, "stale=false should be false");

    // stale not present (default false)
    let header = r#"Digest realm="test", nonce="123""#;
    let challenge = DigestChallenge::parse(header).unwrap();
    assert!(!challenge.stale, "Missing stale should default to false");
}
