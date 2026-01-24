use anyhow::{Context, Result};
use serde::Deserialize;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{error, info, warn};

use crate::config::Config;

pub const MAX_RETRIES: u32 = 3;
pub const INITIAL_BACKOFF_MS: u64 = 1000;

#[derive(Debug, Deserialize, PartialEq)]
pub struct VoipMsResponse {
    pub status: String,
    #[serde(default)]
    pub message: Option<String>,
}

impl VoipMsResponse {
    pub fn success() -> Self {
        Self {
            status: "success".to_string(),
            message: None,
        }
    }

    pub fn error(msg: &str) -> Self {
        Self {
            status: "error".to_string(),
            message: Some(msg.to_string()),
        }
    }
}

/// Build the SMS API URL with proper encoding
pub fn build_sms_url(
    api_user: &str,
    api_pass: &str,
    sms_did: &str,
    alert_phone: &str,
    message: &str,
) -> String {
    format!(
        "https://voip.ms/api/v1/rest.php?\
        api_username={}&\
        api_password={}&\
        method=sendSMS&\
        did={}&\
        dst={}&\
        message={}",
        urlencoding::encode(api_user),
        urlencoding::encode(api_pass),
        urlencoding::encode(sms_did),
        urlencoding::encode(alert_phone),
        urlencoding::encode(message),
    )
}

/// Calculate backoff duration for a given attempt (0-indexed)
/// Attempt 0: no backoff, Attempt 1: 1s, Attempt 2: 2s, etc.
#[inline]
pub fn calculate_backoff(attempt: u32) -> Duration {
    if attempt == 0 {
        Duration::ZERO
    } else {
        Duration::from_millis(INITIAL_BACKOFF_MS * (1 << (attempt - 1)))
    }
}

pub struct Notifier {
    client: reqwest::Client,
    api_user: String,
    api_pass: String,
    sms_did: String,
    alert_phone: String,
}

impl Notifier {
    pub fn new(config: &Config) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_user: config.voipms_api_user.clone(),
            api_pass: config.voipms_api_pass.clone(),
            sms_did: config.voipms_sms_did.clone(),
            alert_phone: config.alert_phone.clone(),
        }
    }

    pub async fn send_alert(&self, message: &str) -> Result<()> {
        info!("Sending SMS alert: {}", message);

        let mut last_error = None;

        for attempt in 0..MAX_RETRIES {
            if attempt > 0 {
                let backoff = calculate_backoff(attempt);
                warn!("SMS attempt {} failed, retrying in {:?}...", attempt, backoff);
                sleep(backoff).await;
            }

            match self.try_send_sms(message).await {
                Ok(()) => {
                    info!("SMS sent successfully");
                    return Ok(());
                }
                Err(e) => {
                    last_error = Some(e);
                }
            }
        }

        let err = last_error.unwrap();
        error!("Failed to send SMS after {} attempts: {}", MAX_RETRIES, err);
        Err(err)
    }

    async fn try_send_sms(&self, message: &str) -> Result<()> {
        let url = build_sms_url(
            &self.api_user,
            &self.api_pass,
            &self.sms_did,
            &self.alert_phone,
            message,
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to send SMS request")?;

        let result: VoipMsResponse = response
            .json()
            .await
            .context("Failed to parse voip.ms response")?;

        if result.status == "success" {
            Ok(())
        } else {
            let err_msg = result.message.unwrap_or_else(|| "Unknown error".to_string());
            anyhow::bail!("SMS API error: {}", err_msg)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_sms_url_basic() {
        let url = build_sms_url("user", "pass", "5551234", "5559876", "Hello");
        assert!(url.starts_with("https://voip.ms/api/v1/rest.php?"));
        assert!(url.contains("api_username=user"));
        assert!(url.contains("api_password=pass"));
        assert!(url.contains("method=sendSMS"));
        assert!(url.contains("did=5551234"));
        assert!(url.contains("dst=5559876"));
        assert!(url.contains("message=Hello"));
    }

    #[test]
    fn test_build_sms_url_encodes_special_chars() {
        let url = build_sms_url(
            "user@example.com",
            "p@ss&word",
            "555",
            "666",
            "Hello World! Special chars: &=?",
        );
        // @ should be encoded as %40
        assert!(url.contains("api_username=user%40example.com"));
        // & should be encoded as %26
        assert!(url.contains("api_password=p%40ss%26word"));
        // Space should be encoded as %20
        assert!(url.contains("message=Hello%20World"));
        // & = ? should be encoded
        assert!(url.contains("%26"));
        assert!(url.contains("%3D"));
        assert!(url.contains("%3F"));
    }

    #[test]
    fn test_build_sms_url_unicode() {
        let url = build_sms_url("user", "pass", "555", "666", "Hello 世界 🌍");
        // Should not panic and should contain encoded unicode
        assert!(url.contains("message=Hello%20"));
        // The URL should be valid
        assert!(!url.contains("世界")); // Should be encoded
    }

    #[test]
    fn test_calculate_backoff() {
        assert_eq!(calculate_backoff(0), Duration::ZERO);
        assert_eq!(calculate_backoff(1), Duration::from_millis(1000));
        assert_eq!(calculate_backoff(2), Duration::from_millis(2000));
        assert_eq!(calculate_backoff(3), Duration::from_millis(4000));
    }

    #[test]
    fn test_voipms_response_success() {
        let json = r#"{"status": "success"}"#;
        let response: VoipMsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.status, "success");
        assert_eq!(response.message, None);
    }

    #[test]
    fn test_voipms_response_error() {
        let json = r#"{"status": "error", "message": "Invalid API credentials"}"#;
        let response: VoipMsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.status, "error");
        assert_eq!(response.message, Some("Invalid API credentials".to_string()));
    }

    #[test]
    fn test_voipms_response_error_no_message() {
        let json = r#"{"status": "error"}"#;
        let response: VoipMsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.status, "error");
        assert_eq!(response.message, None);
    }

    #[test]
    fn test_voipms_response_helpers() {
        let success = VoipMsResponse::success();
        assert_eq!(success.status, "success");

        let error = VoipMsResponse::error("test error");
        assert_eq!(error.status, "error");
        assert_eq!(error.message, Some("test error".to_string()));
    }

    #[test]
    fn test_backoff_sequence() {
        // Verify the exponential backoff sequence: 0, 1s, 2s, 4s, 8s, ...
        let backoffs: Vec<u64> = (0..5).map(|i| calculate_backoff(i).as_millis() as u64).collect();
        assert_eq!(backoffs, vec![0, 1000, 2000, 4000, 8000]);
    }

    #[test]
    fn test_max_retries_constant() {
        assert_eq!(MAX_RETRIES, 3);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// URL encoding should never panic for any UTF-8 string
        #[test]
        fn url_encoding_never_panics(message in ".*") {
            let _ = build_sms_url("user", "pass", "555", "666", &message);
            // If we get here, no panic occurred
        }

        /// Any valid ASCII message should be properly encoded
        #[test]
        fn ascii_messages_encode_properly(message in "[a-zA-Z0-9 .,!?]{0,200}") {
            let url = build_sms_url("user", "pass", "555", "666", &message);
            // URL should not contain raw spaces
            if message.contains(' ') {
                prop_assert!(!url.ends_with(&message) || !message.contains(' '));
            }
        }

        /// Backoff calculation should never overflow for reasonable retry counts
        #[test]
        fn backoff_never_overflows(attempt in 0u32..20) {
            let backoff = calculate_backoff(attempt);
            prop_assert!(backoff.as_millis() < u64::MAX as u128);
        }

        /// API credentials with special chars should encode correctly
        #[test]
        fn special_chars_in_credentials(
            user in "[a-z@.]{1,20}",
            pass in "[a-zA-Z0-9&=?#]{1,20}"
        ) {
            let url = build_sms_url(&user, &pass, "555", "666", "test");
            // Should not contain raw special chars that break URLs
            let query_part = url.split('?').nth(1).unwrap();
            // Count ampersands - should only be parameter separators
            let param_count = query_part.matches('&').count();
            // We have 6 parameters separated by 5 ampersands
            prop_assert_eq!(param_count, 5, "URL should have exactly 5 & separators");
        }
    }
}

/// Kani formal verification proofs
#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    fn backoff_calculation_never_overflows() {
        let attempt: u32 = kani::any();
        kani::assume(attempt < 32); // Reasonable bound - 32 retries is far beyond MAX_RETRIES

        // This should not panic due to overflow
        let _ = calculate_backoff(attempt);
    }

    #[kani::proof]
    fn max_practical_backoff() {
        // For MAX_RETRIES=3, verify backoffs are reasonable
        for attempt in 0..=MAX_RETRIES {
            let backoff = calculate_backoff(attempt);
            kani::assert(
                backoff.as_millis() <= 4000,
                "backoff should be at most 4 seconds for MAX_RETRIES=3"
            );
        }
    }
}
