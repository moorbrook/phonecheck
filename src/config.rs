//! Configuration module
//!
//! Provides typed access to environment variables for PhoneCheck.

use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::env;
use std::net::ToSocketAddrs;
use std::path::Path;

/// Typed configuration keys
///
/// Using an enum for config keys provides compile-time safety
/// and prevents typos compared to string literals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfigKey {
    // SIP credentials
    SipUsername,
    SipPassword,
    SipServer,
    SipPort,

    // Target to call
    TargetPhone,

    // Detection settings
    ExpectedPhrase,
    ListenDurationSecs,

    // Pushover notifications
    PushoverUserKey,
    PushoverApiToken,

    // Whisper model path (GGML format, e.g., ggml-base.en.bin)
    WhisperModelPath,

    // STUN server for NAT traversal (optional)
    StunServer,

    // Minimum audio duration in milliseconds
    MinAudioDurationMs,

    // Health check HTTP server port (optional, disabled if not set)
    HealthPort,
}

impl ConfigKey {
    /// Get the environment variable name for this key
    pub fn env_var(&self) -> &'static str {
        match self {
            ConfigKey::SipUsername => "SIP_USERNAME",
            ConfigKey::SipPassword => "SIP_PASSWORD",
            ConfigKey::SipServer => "SIP_SERVER",
            ConfigKey::SipPort => "SIP_PORT",
            ConfigKey::TargetPhone => "TARGET_PHONE",
            ConfigKey::ExpectedPhrase => "EXPECTED_PHRASE",
            ConfigKey::ListenDurationSecs => "LISTEN_DURATION_SECS",
            ConfigKey::PushoverUserKey => "PUSHOVER_USER_KEY",
            ConfigKey::PushoverApiToken => "PUSHOVER_API_TOKEN",
            ConfigKey::WhisperModelPath => "WHISPER_MODEL_PATH",
            ConfigKey::StunServer => "STUN_SERVER",
            ConfigKey::MinAudioDurationMs => "MIN_AUDIO_DURATION_MS",
            ConfigKey::HealthPort => "HEALTH_PORT",
        }
    }

    /// Check if this key is required (no default value)
    pub fn is_required(&self) -> bool {
        matches!(
            self,
            ConfigKey::SipUsername
                | ConfigKey::SipPassword
                | ConfigKey::SipServer
                | ConfigKey::TargetPhone
                | ConfigKey::PushoverUserKey
                | ConfigKey::PushoverApiToken
        )
    }

    /// Get default value for this key (if any)
    pub fn default_value(&self) -> Option<&'static str> {
        match self {
            ConfigKey::SipPort => Some("5060"),
            ConfigKey::ExpectedPhrase => Some("thank you for calling cubic machinery"),
            ConfigKey::ListenDurationSecs => Some("10"),
            ConfigKey::WhisperModelPath => Some("./models/ggml-base.en.bin"),
            ConfigKey::MinAudioDurationMs => Some("500"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    // SIP credentials
    pub sip_username: String,
    /// SIP password - used for RFC 2617/7616 digest authentication
    /// When the SIP server responds with 401/407, the client will retry
    /// using this password to compute the digest response.
    pub sip_password: String,
    pub sip_server: String,
    pub sip_port: u16,

    // Target to call
    pub target_phone: String,

    // Detection settings
    pub expected_phrase: String,
    pub listen_duration_secs: u64,

    // Pushover notifications
    pub pushover_user_key: String,
    pub pushover_api_token: String,

    // Whisper model path (GGML format, e.g., ggml-base.en.bin)
    pub whisper_model_path: String,

    // STUN server for NAT traversal (optional)
    pub stun_server: Option<String>,

    // Minimum audio duration in milliseconds to consider "audio received"
    // Default: 500ms (catches brief noise vs actual greeting)
    pub min_audio_duration_ms: u64,

    // Health check HTTP server port (optional, disabled if not set)
    // When set, exposes /health, /ready, and /metrics endpoints
    pub health_port: Option<u16>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok(); // Load .env if present, ignore if missing
        Self::from_getter(|key| env::var(key.env_var()).ok())
    }

    /// Parse config from a custom getter function (for testing)
    pub fn from_getter<F>(get: F) -> Result<Self>
    where
        F: Fn(ConfigKey) -> Option<String>,
    {
        Ok(Config {
            sip_username: get(ConfigKey::SipUsername).context(ConfigKey::SipUsername.env_var())?,
            sip_password: get(ConfigKey::SipPassword).context(ConfigKey::SipPassword.env_var())?,
            sip_server: get(ConfigKey::SipServer).context(ConfigKey::SipServer.env_var())?,
            sip_port: get(ConfigKey::SipPort)
                .unwrap_or_else(|| ConfigKey::SipPort.default_value().unwrap().to_string())
                .parse()
                .context(format!("{} must be a valid port number", ConfigKey::SipPort.env_var()))?,

            target_phone: get(ConfigKey::TargetPhone)
                .context(ConfigKey::TargetPhone.env_var())?,

            expected_phrase: get(ConfigKey::ExpectedPhrase)
                .unwrap_or_else(|| ConfigKey::ExpectedPhrase.default_value().unwrap().to_string())
                .to_lowercase(),
            listen_duration_secs: get(ConfigKey::ListenDurationSecs)
                .unwrap_or_else(|| ConfigKey::ListenDurationSecs.default_value().unwrap().to_string())
                .parse()
                .unwrap_or(10),

            pushover_user_key: get(ConfigKey::PushoverUserKey)
                .context(ConfigKey::PushoverUserKey.env_var())?,
            pushover_api_token: get(ConfigKey::PushoverApiToken)
                .context(ConfigKey::PushoverApiToken.env_var())?,

            whisper_model_path: get(ConfigKey::WhisperModelPath)
                .unwrap_or_else(|| {
                    ConfigKey::WhisperModelPath
                        .default_value()
                        .unwrap()
                        .to_string()
                }),

            stun_server: get(ConfigKey::StunServer).filter(|s| !s.is_empty()),

            min_audio_duration_ms: get(ConfigKey::MinAudioDurationMs)
                .and_then(|s| s.parse().ok())
                .unwrap_or(500),

            health_port: get(ConfigKey::HealthPort).and_then(|s| s.parse().ok()),
        })
    }

    /// Create config from a HashMap (convenience for testing)
    #[cfg(test)]
    pub fn from_map(map: &HashMap<&str, &str>) -> Result<Self> {
        Self::from_getter(|key| map.get(key.env_var()).map(|v| v.to_string()))
    }

    /// Validate configuration values at startup.
    /// Returns Ok(()) if all validations pass, or Err with details of what failed.
    pub fn validate(&self) -> Result<()> {
        let mut errors: Vec<String> = Vec::new();

        // Validate Whisper model path exists
        if !Path::new(&self.whisper_model_path).exists() {
            errors.push(format!(
                "Whisper model not found at '{}'. Download from HuggingFace.",
                self.whisper_model_path
            ));
        }

        // Validate SIP server can be resolved
        let sip_addr = format!("{}:{}", self.sip_server, self.sip_port);
        if sip_addr.to_socket_addrs().is_err() {
            errors.push(format!(
                "Cannot resolve SIP server '{}'. Check DNS or network.",
                self.sip_server
            ));
        }

        // Validate phone number (10 digits for voip.ms)
        if !Self::is_valid_phone(&self.target_phone) {
            errors.push(format!(
                "TARGET_PHONE '{}' invalid. Expected 10 digits.",
                self.target_phone
            ));
        }

        // Validate expected phrase is not empty
        if self.expected_phrase.trim().is_empty() {
            errors.push("EXPECTED_PHRASE cannot be empty.".to_string());
        }

        // Validate listen duration is reasonable
        if self.listen_duration_secs == 0 {
            errors.push("LISTEN_DURATION_SECS must be greater than 0.".to_string());
        } else if self.listen_duration_secs > 300 {
            errors.push(format!(
                "LISTEN_DURATION_SECS={} seems too long (max recommended: 300).",
                self.listen_duration_secs
            ));
        }

        if errors.is_empty() {
            Ok(())
        } else {
            bail!(
                "Configuration validation failed:\n  - {}",
                errors.join("\n  - ")
            )
        }
    }

    /// Check if a phone number is valid (10 digits for voip.ms)
    fn is_valid_phone(phone: &str) -> bool {
        let digit_count = phone.chars().filter(|c| c.is_ascii_digit()).count();
        digit_count == 10
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_valid_env() -> HashMap<&'static str, &'static str> {
        let mut m = HashMap::new();
        m.insert("SIP_USERNAME", "testuser");
        m.insert("SIP_PASSWORD", "testpass");
        m.insert("SIP_SERVER", "sip.example.com");
        m.insert("TARGET_PHONE", "5551234567");
        m.insert("PUSHOVER_USER_KEY", "user123");
        m.insert("PUSHOVER_API_TOKEN", "token456");
        m
    }

    #[test]
    fn test_valid_minimal_config() {
        let env = minimal_valid_env();
        let config = Config::from_map(&env).expect("should parse valid config");

        assert_eq!(config.sip_username, "testuser");
        assert_eq!(config.sip_port, 5060); // default
        assert_eq!(config.listen_duration_secs, 10); // default
        assert_eq!(
            config.whisper_model_path,
            "./models/ggml-base.en.bin"
        ); // default
    }

    #[test]
    fn test_custom_port() {
        let mut env = minimal_valid_env();
        env.insert("SIP_PORT", "5061");
        let config = Config::from_map(&env).expect("should parse");
        assert_eq!(config.sip_port, 5061);
    }

    #[test]
    fn test_invalid_port_not_numeric() {
        let mut env = minimal_valid_env();
        env.insert("SIP_PORT", "not_a_number");
        let result = Config::from_map(&env);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("SIP_PORT"), "error should mention SIP_PORT: {}", err);
    }

    #[test]
    fn test_missing_required_sip_username() {
        let mut env = minimal_valid_env();
        env.remove("SIP_USERNAME");
        let result = Config::from_map(&env);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("SIP_USERNAME"), "error should mention SIP_USERNAME");
    }

    #[test]
    fn test_missing_required_sip_password() {
        let mut env = minimal_valid_env();
        env.remove("SIP_PASSWORD");
        let result = Config::from_map(&env);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("SIP_PASSWORD"), "error should mention SIP_PASSWORD");
    }

    #[test]
    fn test_missing_required_target_phone() {
        let mut env = minimal_valid_env();
        env.remove("TARGET_PHONE");
        let result = Config::from_map(&env);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("TARGET_PHONE"), "error should mention TARGET_PHONE");
    }

    #[test]
    fn test_missing_required_pushover_fields() {
        for field in ["PUSHOVER_USER_KEY", "PUSHOVER_API_TOKEN"] {
            let mut env = minimal_valid_env();
            env.remove(field);
            let result = Config::from_map(&env);
            assert!(result.is_err(), "{} should be required", field);
            let err = result.unwrap_err().to_string();
            assert!(err.contains(field), "error should mention {}: {}", field, err);
        }
    }

    #[test]
    fn test_expected_phrase_lowercased() {
        let mut env = minimal_valid_env();
        env.insert("EXPECTED_PHRASE", "Hello WORLD");
        let config = Config::from_map(&env).expect("should parse");
        assert_eq!(config.expected_phrase, "hello world");
    }

    #[test]
    fn test_listen_duration_custom() {
        let mut env = minimal_valid_env();
        env.insert("LISTEN_DURATION_SECS", "30");
        let config = Config::from_map(&env).expect("should parse");
        assert_eq!(config.listen_duration_secs, 30);
    }

    #[test]
    fn test_listen_duration_invalid_uses_default() {
        let mut env = minimal_valid_env();
        env.insert("LISTEN_DURATION_SECS", "not_a_number");
        let config = Config::from_map(&env).expect("should parse with default");
        assert_eq!(config.listen_duration_secs, 10); // falls back to default
    }

    #[test]
    fn test_whisper_model_path_custom() {
        let mut env = minimal_valid_env();
        env.insert("WHISPER_MODEL_PATH", "/custom/path/model.bin");
        let config = Config::from_map(&env).expect("should parse");
        assert_eq!(config.whisper_model_path, "/custom/path/model.bin");
    }

    #[test]
    fn test_port_boundary_values() {
        // Test valid boundary values
        for port in ["1", "80", "443", "5060", "65535"] {
            let mut env = minimal_valid_env();
            env.insert("SIP_PORT", port);
            let config = Config::from_map(&env).expect(&format!("port {} should be valid", port));
            assert_eq!(config.sip_port, port.parse::<u16>().unwrap());
        }
    }

    #[test]
    fn test_port_zero_is_valid() {
        // Port 0 is technically valid (means "any available port")
        let mut env = minimal_valid_env();
        env.insert("SIP_PORT", "0");
        let config = Config::from_map(&env).expect("port 0 should be valid");
        assert_eq!(config.sip_port, 0);
    }

    #[test]
    fn test_phone_validation_valid() {
        assert!(Config::is_valid_phone("5551234567"));
        assert!(Config::is_valid_phone("555-123-4567")); // with dashes (ignored)
        assert!(Config::is_valid_phone("(555) 123-4567")); // formatted (ignored)
    }

    #[test]
    fn test_phone_validation_invalid() {
        assert!(!Config::is_valid_phone("555")); // too short
        assert!(!Config::is_valid_phone("15551234567")); // 11 digits (too long)
        assert!(!Config::is_valid_phone("")); // empty
        assert!(!Config::is_valid_phone("abcdefghij")); // non-numeric
    }

    #[test]
    fn test_validation_empty_expected_phrase() {
        let mut env = minimal_valid_env();
        env.insert("EXPECTED_PHRASE", "   ");
        let config = Config::from_map(&env).expect("should parse");
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("EXPECTED_PHRASE"),
            "error should mention empty phrase: {}",
            err
        );
    }

    #[test]
    fn test_validation_zero_listen_duration() {
        let mut env = minimal_valid_env();
        env.insert("LISTEN_DURATION_SECS", "0");
        let config = Config::from_map(&env).expect("should parse");
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("LISTEN_DURATION_SECS"),
            "error should mention duration: {}",
            err
        );
    }

    #[test]
    fn test_validation_excessive_listen_duration() {
        let mut env = minimal_valid_env();
        env.insert("LISTEN_DURATION_SECS", "500");
        let config = Config::from_map(&env).expect("should parse");
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("too long"), "error should mention duration too long: {}", err);
    }

    #[test]
    fn test_validation_invalid_target_phone() {
        let mut env = minimal_valid_env();
        env.insert("TARGET_PHONE", "123"); // invalid
        let config = Config::from_map(&env).expect("should parse");
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("TARGET_PHONE"), "error should mention invalid phone: {}", err);
    }

    #[test]
    fn test_config_key_env_var() {
        // Test that all keys have env var names
        use ConfigKey::*;
        for key in [
            SipUsername,
            SipPassword,
            SipServer,
            SipPort,
            TargetPhone,
            ExpectedPhrase,
            ListenDurationSecs,
            PushoverUserKey,
            PushoverApiToken,
            WhisperModelPath,
            StunServer,
            MinAudioDurationMs,
            HealthPort,
        ] {
            assert!(!key.env_var().is_empty(), "{:?} env var is empty", key);
        }
    }

    #[test]
    fn test_config_key_is_required() {
        // Test that required keys are correctly identified
        use ConfigKey::*;
        assert!(SipUsername.is_required());
        assert!(SipPassword.is_required());
        assert!(SipServer.is_required());
        assert!(TargetPhone.is_required());
        assert!(PushoverUserKey.is_required());
        assert!(PushoverApiToken.is_required());

        assert!(!SipPort.is_required()); // has default
        assert!(!ExpectedPhrase.is_required()); // has default
        assert!(!StunServer.is_required()); // optional
    }

    #[test]
    fn test_config_key_default_value() {
        // Test that keys with defaults return them
        use ConfigKey::*;
        assert_eq!(SipPort.default_value(), Some("5060"));
        assert_eq!(ExpectedPhrase.default_value(), Some("thank you for calling cubic machinery"));
        assert_eq!(ListenDurationSecs.default_value(), Some("10"));
        assert_eq!(WhisperModelPath.default_value(), Some("./models/ggml-base.en.bin"));
        assert_eq!(MinAudioDurationMs.default_value(), Some("500"));

        // Keys without defaults
        assert!(SipUsername.default_value().is_none());
        assert!(TargetPhone.default_value().is_none());
        assert!(StunServer.default_value().is_none());
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    fn valid_env_strategy() -> impl Strategy<Value = HashMap<&'static str, String>> {
        (
            "[a-z]{3,10}",           // sip_username
            "[a-z0-9]{8,16}",        // sip_password
            "[a-z]+\\.[a-z]{2,4}",   // sip_server
            1u16..=65535u16,         // sip_port
            "[0-9]{10}",             // target_phone
            "[a-z ]{5,30}",          // expected_phrase
            1u64..=300u64,           // listen_duration
            "[a-z0-9]{20,30}",       // pushover_user_key
            "[a-z0-9]{20,30}",       // pushover_api_token
        )
            .prop_map(
                |(user, pass, server, port, phone, phrase, duration, po_user, po_token)| {
                    let mut m = HashMap::new();
                    m.insert("SIP_USERNAME", user);
                    m.insert("SIP_PASSWORD", pass);
                    m.insert("SIP_SERVER", server);
                    m.insert("SIP_PORT", port.to_string());
                    m.insert("TARGET_PHONE", phone);
                    m.insert("EXPECTED_PHRASE", phrase);
                    m.insert("LISTEN_DURATION_SECS", duration.to_string());
                    m.insert("PUSHOVER_USER_KEY", po_user);
                    m.insert("PUSHOVER_API_TOKEN", po_token);
                    m
                },
            )
    }

    proptest! {
        #[test]
        fn valid_configs_parse_successfully(env in valid_env_strategy()) {
            let result = Config::from_getter(|key| {
                env.get(key.env_var()).cloned()
            });
            prop_assert!(result.is_ok(), "valid config should parse: {:?}", result.err());
        }

        #[test]
        fn port_parsing_never_panics(port_str in ".*") {
            // This should never panic, only return Ok or Err
            let mut env: HashMap<&str, String> = HashMap::new();
            env.insert("SIP_USERNAME", "user".to_string());
            env.insert("SIP_PASSWORD", "pass".to_string());
            env.insert("SIP_SERVER", "server.com".to_string());
            env.insert("SIP_PORT", port_str);
            env.insert("TARGET_PHONE", "1234567890".to_string());
            env.insert("PUSHOVER_USER_KEY", "userkey123".to_string());
            env.insert("PUSHOVER_API_TOKEN", "token456".to_string());

            let _ = Config::from_getter(|key| env.get(key.env_var()).cloned());
            // If we get here without panicking, the test passes
        }

        #[test]
        fn expected_phrase_always_lowercased(phrase in "[A-Za-z ]{1,50}") {
            let mut env: HashMap<&str, String> = HashMap::new();
            env.insert("SIP_USERNAME", "user".to_string());
            env.insert("SIP_PASSWORD", "pass".to_string());
            env.insert("SIP_SERVER", "server.com".to_string());
            env.insert("TARGET_PHONE", "1234567890".to_string());
            env.insert("EXPECTED_PHRASE", phrase.clone());
            env.insert("PUSHOVER_USER_KEY", "userkey123".to_string());
            env.insert("PUSHOVER_API_TOKEN", "token456".to_string());

            let config = Config::from_getter(|key| env.get(key.env_var()).cloned()).unwrap();
            prop_assert_eq!(config.expected_phrase, phrase.to_lowercase());
        }
    }
}

/// Kani formal verification proofs
#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    fn port_parsing_never_panics() {
        let port_str: [u8; 8] = kani::any();
        // Convert to string, handling invalid UTF-8
        if let Ok(s) = std::str::from_utf8(&port_str) {
            let _ = s.parse::<u16>();
            // If we get here without panicking, the proof passes
        }
    }

    #[kani::proof]
    fn valid_port_range() {
        let port: u16 = kani::any();
        let port_str = port.to_string();
        let parsed: u16 = port_str.parse().unwrap();
        kani::assert(parsed == port, "round-trip must preserve value");
    }
}
