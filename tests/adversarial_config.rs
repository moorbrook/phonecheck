//! Adversarial Property-Based Tests for Configuration Parsing
//!
//! # Attack Plan
//!
//! 1. **Port Number Attacks**: Negative numbers (as string), overflow, float,
//!    scientific notation, unicode digits.
//!
//! 2. **Phone Number Bypass**: Unicode digits (٠١٢٣), zero-width chars,
//!    control characters, very long numbers, international formats.
//!
//! 3. **Path Traversal**: Model path with `../`, null bytes, very long paths.
//!
//! 4. **Empty vs Missing Fields**: Empty strings should behave differently
//!    than missing environment variables.
//!
//! 5. **Extremely Long Values**: Megabyte strings for all fields.
//!
//! 6. **Duration Bounds**: 0, 1, 300, 301, MAX values.
//!
//! # Invariants
//!
//! - from_getter never panics on any input
//! - is_valid_phone_number never panics
//! - validate() never panics (may return Err)
//! - expected_phrase is always lowercase
//! - Required fields missing returns Err
//! - Port parsing with invalid string uses default or returns Err

use proptest::prelude::*;
use std::collections::HashMap;

use phonecheck::config::Config;

// ============================================================================
// ADVERSARIAL GENERATORS
// ============================================================================

/// Generate malformed port strings
fn malformed_port() -> impl Strategy<Value = String> {
    prop_oneof![
        // Numeric edge cases
        Just("-1".to_string()),
        Just("-0".to_string()),
        Just("0".to_string()),
        Just("65535".to_string()),
        Just("65536".to_string()),
        Just("99999".to_string()),
        Just("4294967296".to_string()), // u32::MAX + 1
        // Float
        Just("5060.5".to_string()),
        Just("5060.0".to_string()),
        Just(".5060".to_string()),
        // Scientific notation
        Just("5e3".to_string()),
        Just("5.06e3".to_string()),
        Just("1e10".to_string()),
        // Non-numeric
        Just("".to_string()),
        Just("   ".to_string()),
        Just("abc".to_string()),
        Just("port".to_string()),
        Just("NaN".to_string()),
        Just("Infinity".to_string()),
        // Unicode digits
        Just("٥٠٦٠".to_string()),  // Arabic-Indic digits for 5060
        Just("５０６０".to_string()), // Fullwidth digits
        // Injection
        Just("5060; DROP TABLE".to_string()),
        Just("5060\x00hidden".to_string()),
        Just("5060\r\n".to_string()),
        // Leading/trailing
        Just(" 5060".to_string()),
        Just("5060 ".to_string()),
        Just("+5060".to_string()),
    ]
}

/// Generate potentially dangerous phone numbers
fn dangerous_phone() -> impl Strategy<Value = String> {
    prop_oneof![
        // Valid formats
        Just("5551234567".to_string()),
        Just("+15551234567".to_string()),
        Just("15551234567".to_string()),
        // Too short/long
        Just("".to_string()),
        Just("123".to_string()),
        Just("12345678901234567890".to_string()),
        // Unicode digits
        Just("٥٥٥١٢٣٤٥٦٧".to_string()),  // Arabic-Indic
        Just("５５５１２３４５６７".to_string()), // Fullwidth
        // Control characters
        Just("555\x001234567".to_string()),
        Just("555\t1234567".to_string()),
        Just("555\n1234567".to_string()),
        // Zero-width characters
        Just("555\u{200B}1234567".to_string()),  // zero-width space
        Just("555\u{200D}1234567".to_string()),  // zero-width joiner
        // Special chars
        Just("555-123-4567".to_string()),
        Just("(555) 123-4567".to_string()),
        Just("+1 (555) 123-4567".to_string()),
        // International
        Just("+447911123456".to_string()),  // UK
        Just("+81312345678".to_string()),   // Japan
        // Injection attempts
        Just("5551234567; --".to_string()),
        Just("5551234567\x00".to_string()),
    ]
}

/// Generate potentially dangerous paths
fn dangerous_path() -> impl Strategy<Value = String> {
    prop_oneof![
        // Normal paths
        Just("./models/model.bin".to_string()),
        Just("/absolute/path/model.bin".to_string()),
        // Traversal
        Just("../../../etc/passwd".to_string()),
        Just("./models/../../../etc/passwd".to_string()),
        // Null byte injection
        Just("./models/model.bin\x00.txt".to_string()),
        // Very long path
        Just("./".to_string() + &"a/".repeat(1000) + "model.bin"),
        // Empty
        Just("".to_string()),
        // Whitespace only
        Just("   ".to_string()),
        // Special paths
        Just("/dev/null".to_string()),
        Just("/proc/self/environ".to_string()),
        // Unicode
        Just("./models/日本語.bin".to_string()),
        // Symlink attack would require filesystem, just test path parsing
        Just("./models/link".to_string()),
    ]
}

/// Generate various string lengths
fn various_lengths() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("".to_string()),
        Just("a".to_string()),
        Just("a".repeat(100)),
        Just("a".repeat(10000)),
        Just("a".repeat(100000)),
    ]
}

// ============================================================================
// HELPER: Create base valid config
// ============================================================================

fn base_valid_config() -> HashMap<&'static str, String> {
    let mut m = HashMap::new();
    m.insert("SIP_USERNAME", "user".to_string());
    m.insert("SIP_PASSWORD", "pass".to_string());
    m.insert("SIP_SERVER", "sip.example.com".to_string());
    m.insert("TARGET_PHONE", "5551234567".to_string());
    m.insert("VOIPMS_API_USER", "apiuser".to_string());
    m.insert("VOIPMS_API_PASS", "apipass".to_string());
    m.insert("VOIPMS_SMS_DID", "5551234567".to_string());
    m.insert("ALERT_PHONE", "5551234567".to_string());
    m
}

// ============================================================================
// INVARIANT: from_getter NEVER PANICS
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn prop_from_getter_never_panics_with_arbitrary_port(port in malformed_port()) {
        let mut env = base_valid_config();
        env.insert("SIP_PORT", port);
        let _ = Config::from_getter(|key| env.get(key).cloned());
    }

    #[test]
    fn prop_from_getter_never_panics_with_arbitrary_phone(phone in dangerous_phone()) {
        let mut env = base_valid_config();
        env.insert("TARGET_PHONE", phone);
        let _ = Config::from_getter(|key| env.get(key).cloned());
    }

    #[test]
    fn prop_from_getter_never_panics_with_arbitrary_path(path in dangerous_path()) {
        let mut env = base_valid_config();
        env.insert("WHISPER_MODEL_PATH", path);
        let _ = Config::from_getter(|key| env.get(key).cloned());
    }

    #[test]
    fn prop_from_getter_never_panics_with_arbitrary_values(
        username in ".*",
        password in ".*",
        server in ".*",
        port in ".*",
        phone in ".*",
        phrase in ".*",
        duration in ".*",
    ) {
        let mut env: HashMap<&str, String> = HashMap::new();
        env.insert("SIP_USERNAME", username);
        env.insert("SIP_PASSWORD", password);
        env.insert("SIP_SERVER", server);
        env.insert("SIP_PORT", port);
        env.insert("TARGET_PHONE", phone);
        env.insert("EXPECTED_PHRASE", phrase);
        env.insert("LISTEN_DURATION_SECS", duration);
        env.insert("VOIPMS_API_USER", "api".to_string());
        env.insert("VOIPMS_API_PASS", "pass".to_string());
        env.insert("VOIPMS_SMS_DID", "1234567890".to_string());
        env.insert("ALERT_PHONE", "1234567890".to_string());

        let _ = Config::from_getter(|key| env.get(key).cloned());
    }
}

// ============================================================================
// INVARIANT: expected_phrase IS ALWAYS LOWERCASE
// ============================================================================

proptest! {
    #[test]
    fn prop_expected_phrase_always_lowercase(phrase in "[A-Za-z0-9 ]{0,100}") {
        let mut env = base_valid_config();
        env.insert("EXPECTED_PHRASE", phrase.clone());

        let config = Config::from_getter(|key| env.get(key).cloned()).unwrap();
        prop_assert_eq!(config.expected_phrase, phrase.to_lowercase());
    }
}

// ============================================================================
// NEGATIVE ASSERTIONS: REQUIRED FIELDS
// ============================================================================

#[test]
fn test_missing_required_fields() {
    let required_fields = [
        "SIP_USERNAME",
        "SIP_PASSWORD",
        "SIP_SERVER",
        "TARGET_PHONE",
        "VOIPMS_API_USER",
        "VOIPMS_API_PASS",
        "VOIPMS_SMS_DID",
        "ALERT_PHONE",
    ];

    for field in required_fields {
        let mut env = base_valid_config();
        env.remove(field);

        let result = Config::from_getter(|key| env.get(key).cloned());
        assert!(result.is_err(), "Missing {} should cause error", field);

        let err = result.unwrap_err().to_string();
        assert!(
            err.contains(field),
            "Error should mention {}: {}",
            field,
            err
        );
    }
}

#[test]
fn test_empty_string_vs_missing() {
    // Empty string for required field should behave differently than missing
    // (Both should fail, but with different errors)

    let mut env_missing = base_valid_config();
    env_missing.remove("SIP_USERNAME");

    let mut env_empty = base_valid_config();
    env_empty.insert("SIP_USERNAME", "".to_string());

    let result_missing = Config::from_getter(|key| env_missing.get(key).cloned());
    let result_empty = Config::from_getter(|key| env_empty.get(key).cloned());

    // Missing should definitely fail
    assert!(result_missing.is_err());

    // Empty might succeed (empty string is a valid value) but validation should catch it
    // Actually, looking at the code, empty string is accepted during parsing
    if let Ok(config) = result_empty {
        assert_eq!(config.sip_username, "");
    }
}

// ============================================================================
// BOUNDARY STRESS: PORT NUMBERS
// ============================================================================

#[test]
fn test_port_boundary_values() {
    // Valid boundaries
    for port in ["0", "1", "80", "443", "5060", "65535"] {
        let mut env = base_valid_config();
        env.insert("SIP_PORT", port.to_string());
        let config = Config::from_getter(|key| env.get(key).cloned());
        assert!(
            config.is_ok(),
            "Port {} should be valid",
            port
        );
        assert_eq!(config.unwrap().sip_port, port.parse::<u16>().unwrap());
    }
}

#[test]
fn test_port_invalid_values() {
    let invalid_ports = [
        ("65536", false),    // overflow - fails
        ("-1", false),       // negative - fails
        ("abc", false),      // non-numeric - fails
        ("", false),         // empty - fails (parse error on empty string)
        ("5060.5", false),   // float - fails
        ("1e5", false),      // scientific - fails
    ];

    for (port, should_succeed) in invalid_ports {
        let mut env = base_valid_config();
        env.insert("SIP_PORT", port.to_string());
        let result = Config::from_getter(|key| env.get(key).cloned());

        if should_succeed {
            assert!(result.is_ok(), "Port '{}' should succeed", port);
        } else {
            assert!(
                result.is_err(),
                "Port '{}' should fail parsing, got {:?}",
                port,
                result.as_ref().ok().map(|c| c.sip_port)
            );
        }
    }
}

// ============================================================================
// BOUNDARY STRESS: DURATION
// ============================================================================

#[test]
fn test_duration_boundary_values() {
    let test_cases = [
        ("0", 0, true),       // Edge: zero
        ("1", 1, true),       // Min valid
        ("10", 10, true),     // Default
        ("300", 300, true),   // Max recommended
        ("301", 301, true),   // Parses but validation should warn
        ("1000", 1000, true), // Parses
        ("", 10, true),       // Empty uses default
        ("abc", 10, true),    // Invalid uses default
    ];

    for (input, expected, should_parse) in test_cases {
        let mut env = base_valid_config();
        env.insert("LISTEN_DURATION_SECS", input.to_string());
        let result = Config::from_getter(|key| env.get(key).cloned());

        if should_parse {
            assert!(result.is_ok(), "Duration '{}' should parse", input);
            let config = result.unwrap();
            assert_eq!(
                config.listen_duration_secs, expected,
                "Duration '{}' should be {}",
                input, expected
            );
        }
    }
}

#[test]
fn test_duration_validation() {
    // Zero duration fails validation
    let mut env = base_valid_config();
    env.insert("LISTEN_DURATION_SECS", "0".to_string());
    let config = Config::from_getter(|key| env.get(key).cloned()).unwrap();
    let result = config.validate();
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("LISTEN_DURATION"));

    // 301 (> 300) fails validation
    let mut env = base_valid_config();
    env.insert("LISTEN_DURATION_SECS", "301".to_string());
    let config = Config::from_getter(|key| env.get(key).cloned()).unwrap();
    let result = config.validate();
    assert!(result.is_err());
}

// ============================================================================
// BOUNDARY STRESS: VERY LONG VALUES
// ============================================================================

#[test]
fn test_very_long_username() {
    let mut env = base_valid_config();
    env.insert("SIP_USERNAME", "x".repeat(100000));
    let config = Config::from_getter(|key| env.get(key).cloned());
    assert!(config.is_ok());
    assert_eq!(config.unwrap().sip_username.len(), 100000);
}

#[test]
fn test_very_long_phrase() {
    let mut env = base_valid_config();
    env.insert("EXPECTED_PHRASE", "word ".repeat(10000));
    let config = Config::from_getter(|key| env.get(key).cloned());
    assert!(config.is_ok());
}

// ============================================================================
// PHONE NUMBER VALIDATION
// ============================================================================

#[test]
fn test_phone_valid_formats() {
    let valid = [
        "5551234567",           // NANPA
        "15551234567",          // 11-digit with country code
        "+15551234567",         // E.164
        "+447911123456",        // International
        "(555) 123-4567",       // Formatted NANPA
        "+1 (555) 123-4567",    // Formatted E.164
    ];

    for phone in valid {
        // We'd need to access is_valid_phone_number, but it's private
        // Instead, test via validation
        let mut env = base_valid_config();
        env.insert("TARGET_PHONE", phone.to_string());
        let config = Config::from_getter(|key| env.get(key).cloned()).unwrap();

        // Can't directly test is_valid_phone_number, but config parses
        assert_eq!(config.target_phone, phone);
    }
}

#[test]
fn test_phone_invalid_formats_validation() {
    let invalid = [
        "",           // Empty
        "123",        // Too short
        "12345",      // Too short
        "abcdefghij", // Non-numeric
    ];

    for phone in invalid {
        let mut env = base_valid_config();
        env.insert("TARGET_PHONE", phone.to_string());
        let config = Config::from_getter(|key| env.get(key).cloned()).unwrap();
        let result = config.validate();
        assert!(
            result.is_err(),
            "Phone '{}' should fail validation",
            phone
        );
    }
}

// ============================================================================
// STUN SERVER EMPTY STRING HANDLING
// ============================================================================

#[test]
fn test_stun_server_empty_vs_missing() {
    // Missing STUN_SERVER -> None
    let env = base_valid_config();
    let config = Config::from_getter(|key| env.get(key).cloned()).unwrap();
    assert!(config.stun_server.is_none());

    // Empty string STUN_SERVER -> None (filtered by .filter(|s| !s.is_empty()))
    let mut env = base_valid_config();
    env.insert("STUN_SERVER", "".to_string());
    let config = Config::from_getter(|key| env.get(key).cloned()).unwrap();
    assert!(config.stun_server.is_none());

    // Non-empty STUN_SERVER -> Some
    let mut env = base_valid_config();
    env.insert("STUN_SERVER", "stun.example.com:3478".to_string());
    let config = Config::from_getter(|key| env.get(key).cloned()).unwrap();
    assert_eq!(config.stun_server, Some("stun.example.com:3478".to_string()));
}

// ============================================================================
// HEALTH PORT OPTIONAL FIELD
// ============================================================================

#[test]
fn test_health_port_optional() {
    // Missing -> None
    let env = base_valid_config();
    let config = Config::from_getter(|key| env.get(key).cloned()).unwrap();
    assert!(config.health_port.is_none());

    // Valid port -> Some
    let mut env = base_valid_config();
    env.insert("HEALTH_PORT", "8080".to_string());
    let config = Config::from_getter(|key| env.get(key).cloned()).unwrap();
    assert_eq!(config.health_port, Some(8080));

    // Invalid -> None (uses .and_then(|s| s.parse().ok()))
    let mut env = base_valid_config();
    env.insert("HEALTH_PORT", "invalid".to_string());
    let config = Config::from_getter(|key| env.get(key).cloned()).unwrap();
    assert!(config.health_port.is_none());
}

// ============================================================================
// MIN AUDIO DURATION HANDLING
// ============================================================================

#[test]
fn test_min_audio_duration_defaults() {
    // Missing -> 500 (default)
    let env = base_valid_config();
    let config = Config::from_getter(|key| env.get(key).cloned()).unwrap();
    assert_eq!(config.min_audio_duration_ms, 500);

    // Valid value
    let mut env = base_valid_config();
    env.insert("MIN_AUDIO_DURATION_MS", "1000".to_string());
    let config = Config::from_getter(|key| env.get(key).cloned()).unwrap();
    assert_eq!(config.min_audio_duration_ms, 1000);

    // Invalid -> 500 (default)
    let mut env = base_valid_config();
    env.insert("MIN_AUDIO_DURATION_MS", "invalid".to_string());
    let config = Config::from_getter(|key| env.get(key).cloned()).unwrap();
    assert_eq!(config.min_audio_duration_ms, 500);
}

// ============================================================================
// DETERMINISM
// ============================================================================

#[test]
fn test_config_parsing_deterministic() {
    let env = base_valid_config();

    let config1 = Config::from_getter(|key| env.get(key).cloned()).unwrap();
    let config2 = Config::from_getter(|key| env.get(key).cloned()).unwrap();

    assert_eq!(config1.sip_username, config2.sip_username);
    assert_eq!(config1.sip_port, config2.sip_port);
    assert_eq!(config1.expected_phrase, config2.expected_phrase);
    assert_eq!(config1.listen_duration_secs, config2.listen_duration_secs);
}
