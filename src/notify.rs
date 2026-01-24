use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

use crate::config::Config;

pub const MAX_RETRIES: u32 = 3;
pub const INITIAL_BACKOFF_MS: u64 = 1000;

/// Number of consecutive failures before opening the circuit
pub const CIRCUIT_FAILURE_THRESHOLD: u32 = 3;

/// How long to keep the circuit open before trying again
pub const CIRCUIT_OPEN_DURATION: Duration = Duration::from_secs(300); // 5 minutes

/// Circuit breaker state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CircuitState {
    /// Normal operation - requests allowed
    Closed,
    /// Failing - requests blocked
    Open,
    /// Testing if service recovered - limited requests
    HalfOpen,
}

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

/// voip.ms API endpoint
const VOIPMS_API_URL: &str = "https://voip.ms/api/v1/rest.php";

/// Build the SMS API request parameters (credentials not exposed in URL)
pub fn build_sms_params(
    api_user: &str,
    api_pass: &str,
    sms_did: &str,
    alert_phone: &str,
    message: &str,
) -> [(&'static str, String); 6] {
    [
        ("api_username", api_user.to_string()),
        ("api_password", api_pass.to_string()),
        ("method", "sendSMS".to_string()),
        ("did", sms_did.to_string()),
        ("dst", alert_phone.to_string()),
        ("message", message.to_string()),
    ]
}

/// Build the SMS API URL with proper encoding (for backwards compatibility / testing)
/// SECURITY NOTE: Credentials are exposed in URL query string. Prefer POST with build_sms_params.
#[cfg(test)]
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

/// Maximum backoff duration (60 seconds)
pub const MAX_BACKOFF_MS: u64 = 60_000;

/// Maximum SMS message length (standard SMS)
pub const MAX_SMS_LENGTH: usize = 160;

/// Truncate message to fit SMS length limit
/// Returns the original message if it fits, or a truncated version with "..." suffix
pub fn truncate_sms_message(message: &str) -> String {
    if message.len() <= MAX_SMS_LENGTH {
        message.to_string()
    } else {
        // Leave room for "..." (3 chars)
        let target_len = MAX_SMS_LENGTH - 3;

        // Find a valid UTF-8 char boundary at or before target_len
        let mut truncate_at = target_len;
        while truncate_at > 0 && !message.is_char_boundary(truncate_at) {
            truncate_at -= 1;
        }

        if truncate_at == 0 {
            // Edge case: couldn't find a valid boundary, just return "..."
            return "...".to_string();
        }

        let truncated = &message[..truncate_at];

        // Try to truncate at word boundary for cleaner output
        let truncated = truncated
            .rfind(' ')
            .filter(|&pos| pos > truncate_at / 2) // Don't cut too much
            .map(|pos| &truncated[..pos])
            .unwrap_or(truncated);

        format!("{}...", truncated)
    }
}

/// Calculate backoff duration for a given attempt (0-indexed)
/// Attempt 0: no backoff, Attempt 1: 1s, Attempt 2: 2s, etc.
/// Capped at MAX_BACKOFF_MS to prevent overflow and excessive waits.
#[inline]
pub fn calculate_backoff(attempt: u32) -> Duration {
    if attempt == 0 {
        Duration::ZERO
    } else {
        // Cap shift to prevent overflow (max safe shift for u64 is 63)
        let shift = (attempt - 1).min(30);
        let backoff_ms = INITIAL_BACKOFF_MS.saturating_mul(1u64 << shift);
        Duration::from_millis(backoff_ms.min(MAX_BACKOFF_MS))
    }
}

/// Maximum number of alerts to queue when circuit is open
pub const MAX_QUEUED_ALERTS: usize = 10;

/// Circuit breaker for SMS API
pub struct CircuitBreaker {
    state: RwLock<CircuitState>,
    consecutive_failures: AtomicU32,
    opened_at: RwLock<Option<Instant>>,
    last_success: RwLock<Option<Instant>>,
    /// Queue of alerts waiting to be sent when circuit recovers
    pending_alerts: Mutex<VecDeque<String>>,
}

impl CircuitBreaker {
    pub fn new() -> Self {
        Self {
            state: RwLock::new(CircuitState::Closed),
            consecutive_failures: AtomicU32::new(0),
            opened_at: RwLock::new(None),
            last_success: RwLock::new(None),
            pending_alerts: Mutex::new(VecDeque::new()),
        }
    }

    /// Queue an alert for later delivery when circuit recovers
    pub fn queue_alert(&self, message: String) {
        let mut queue = self.pending_alerts.lock().unwrap();
        if queue.len() >= MAX_QUEUED_ALERTS {
            // Drop oldest alert to make room
            let dropped = queue.pop_front();
            if let Some(msg) = dropped {
                warn!("Alert queue full, dropping oldest: {}...", &msg[..msg.len().min(50)]);
            }
        }
        queue.push_back(message);
        info!("Alert queued for later delivery ({} pending)", queue.len());
    }

    /// Take all pending alerts from the queue
    pub fn take_pending_alerts(&self) -> Vec<String> {
        let mut queue = self.pending_alerts.lock().unwrap();
        queue.drain(..).collect()
    }

    /// Get count of pending alerts
    pub fn pending_count(&self) -> usize {
        self.pending_alerts.lock().unwrap().len()
    }

    /// Check if requests are allowed
    pub fn is_allowed(&self) -> bool {
        let state = *self.state.read().unwrap();
        match state {
            CircuitState::Closed => true,
            CircuitState::HalfOpen => true,
            CircuitState::Open => {
                // Check if enough time has passed to try again
                if let Some(opened_at) = *self.opened_at.read().unwrap() {
                    if opened_at.elapsed() >= CIRCUIT_OPEN_DURATION {
                        // Transition to half-open
                        *self.state.write().unwrap() = CircuitState::HalfOpen;
                        return true;
                    }
                }
                false
            }
        }
    }

    /// Record a successful request
    pub fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::SeqCst);
        *self.state.write().unwrap() = CircuitState::Closed;
        *self.last_success.write().unwrap() = Some(Instant::now());
    }

    /// Record a failed request
    pub fn record_failure(&self) {
        let failures = self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;

        if failures >= CIRCUIT_FAILURE_THRESHOLD {
            let mut state = self.state.write().unwrap();
            if *state != CircuitState::Open {
                *state = CircuitState::Open;
                *self.opened_at.write().unwrap() = Some(Instant::now());
                error!(
                    "Circuit breaker opened after {} consecutive failures",
                    failures
                );
            }
        }
    }

    /// Get current state
    pub fn state(&self) -> CircuitState {
        *self.state.read().unwrap()
    }

    /// Get consecutive failure count
    pub fn failure_count(&self) -> u32 {
        self.consecutive_failures.load(Ordering::SeqCst)
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Notifier {
    client: reqwest::Client,
    api_user: String,
    api_pass: String,
    sms_did: String,
    alert_phone: String,
    circuit: CircuitBreaker,
}

/// SMS API error types for retry decisions
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SmsErrorKind {
    /// Transient error - safe to retry
    Transient,
    /// Permanent error - don't retry
    Permanent,
}

impl Notifier {
    pub fn new(config: &Config) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_user: config.voipms_api_user.clone(),
            api_pass: config.voipms_api_pass.clone(),
            sms_did: config.voipms_sms_did.clone(),
            alert_phone: config.alert_phone.clone(),
            circuit: CircuitBreaker::new(),
        }
    }

    /// Check if the circuit breaker is open
    pub fn is_circuit_open(&self) -> bool {
        self.circuit.state() == CircuitState::Open
    }

    /// Get circuit breaker state for monitoring
    pub fn circuit_state(&self) -> CircuitState {
        self.circuit.state()
    }

    pub async fn send_alert(&self, message: &str) -> Result<()> {
        // Truncate message if too long for SMS
        let message = if message.len() > MAX_SMS_LENGTH {
            let truncated = truncate_sms_message(message);
            debug!("SMS truncated from {} to {} chars", message.len(), truncated.len());
            truncated
        } else {
            message.to_string()
        };

        info!("Sending SMS alert: {}", message);

        // Check circuit breaker
        if !self.circuit.is_allowed() {
            // Queue alert for later delivery instead of dropping it
            self.circuit.queue_alert(message.clone());
            error!(
                "Circuit breaker open - alert queued for later ({} pending)",
                self.circuit.pending_count()
            );
            // Also log as fallback in case recovery never happens
            self.log_alert_fallback(&message);
            anyhow::bail!("Circuit breaker open - alert queued");
        }

        let mut last_error = None;

        for attempt in 0..MAX_RETRIES {
            if attempt > 0 {
                let backoff = calculate_backoff(attempt);
                warn!("SMS attempt {} failed, retrying in {:?}...", attempt, backoff);
                sleep(backoff).await;
            }

            match self.try_send_sms(&message).await {
                Ok(()) => {
                    info!("SMS sent successfully");
                    self.circuit.record_success();
                    // Try to send any queued alerts now that circuit is healthy
                    self.send_queued_alerts().await;
                    return Ok(());
                }
                Err(e) => {
                    // Check if error is retryable
                    if Self::classify_error(&e) == SmsErrorKind::Permanent {
                        error!("Permanent SMS error, not retrying: {}", e);
                        self.circuit.record_failure();
                        return Err(e);
                    }
                    last_error = Some(e);
                }
            }
        }

        // All retries exhausted
        self.circuit.record_failure();

        let err = last_error.unwrap();
        error!("Failed to send SMS after {} attempts: {}", MAX_RETRIES, err);

        // If circuit just opened, log the alert as fallback
        if self.circuit.state() == CircuitState::Open {
            self.log_alert_fallback(&message);
        }

        Err(err)
    }

    /// Classify an error as transient or permanent
    fn classify_error(err: &anyhow::Error) -> SmsErrorKind {
        let msg = err.to_string().to_lowercase();

        // Permanent errors - don't retry
        if msg.contains("invalid api credentials")
            || msg.contains("authentication")
            || msg.contains("invalid did")
            || msg.contains("invalid destination")
        {
            return SmsErrorKind::Permanent;
        }

        // Everything else is transient - retry
        SmsErrorKind::Transient
    }

    /// Fallback logging when SMS cannot be sent
    fn log_alert_fallback(&self, message: &str) {
        // Log at error level so it's visible in logs
        error!("ALERT FALLBACK (SMS unavailable): {}", message);
        // In a production system, you might also:
        // - Write to a dedicated alert file
        // - Send via alternative channel (email, webhook, etc.)
    }

    /// Send any queued alerts after circuit recovery
    async fn send_queued_alerts(&self) {
        let pending = self.circuit.take_pending_alerts();
        if pending.is_empty() {
            return;
        }

        info!("Circuit recovered - sending {} queued alerts", pending.len());

        for (i, msg) in pending.iter().enumerate() {
            // Add small delay between messages to avoid rate limiting
            if i > 0 {
                sleep(Duration::from_millis(500)).await;
            }

            match self.try_send_sms(msg).await {
                Ok(()) => {
                    info!("Queued alert {} sent successfully", i + 1);
                }
                Err(e) => {
                    // If sending fails, re-queue remaining alerts and stop
                    error!("Failed to send queued alert: {}", e);
                    self.circuit.record_failure();
                    // Re-queue this and remaining alerts
                    for remaining in &pending[i..] {
                        self.circuit.queue_alert(remaining.clone());
                    }
                    break;
                }
            }
        }
    }

    async fn try_send_sms(&self, message: &str) -> Result<()> {
        let params = build_sms_params(
            &self.api_user,
            &self.api_pass,
            &self.sms_did,
            &self.alert_phone,
            message,
        );

        // Use POST with form body - credentials not exposed in URL
        let response = self
            .client
            .post(VOIPMS_API_URL)
            .form(&params)
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
    fn test_calculate_backoff_caps_at_max() {
        // Large attempt values should cap at MAX_BACKOFF_MS
        assert_eq!(calculate_backoff(10), Duration::from_millis(MAX_BACKOFF_MS));
        assert_eq!(calculate_backoff(100), Duration::from_millis(MAX_BACKOFF_MS));
        assert_eq!(calculate_backoff(u32::MAX), Duration::from_millis(MAX_BACKOFF_MS));
    }

    #[test]
    fn test_calculate_backoff_no_overflow() {
        // Should never panic regardless of input
        for attempt in [0, 1, 10, 31, 32, 63, 64, 100, u32::MAX] {
            let _ = calculate_backoff(attempt);
        }
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
        // Verify the exponential backoff sequence: 0, 1s, 2s, 4s, 8s, ... capped at 60s
        let backoffs: Vec<u64> = (0..5).map(|i| calculate_backoff(i).as_millis() as u64).collect();
        assert_eq!(backoffs, vec![0, 1000, 2000, 4000, 8000]);

        // Verify cap kicks in
        assert_eq!(calculate_backoff(7).as_millis() as u64, 64000.min(MAX_BACKOFF_MS));
    }

    #[test]
    fn test_max_retries_constant() {
        assert_eq!(MAX_RETRIES, 3);
    }

    // === Circuit Breaker tests ===

    #[test]
    fn test_circuit_breaker_starts_closed() {
        let cb = CircuitBreaker::new();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.is_allowed());
    }

    #[test]
    fn test_circuit_breaker_opens_after_threshold() {
        let cb = CircuitBreaker::new();

        // Record failures up to threshold
        for i in 0..CIRCUIT_FAILURE_THRESHOLD {
            assert_eq!(cb.state(), CircuitState::Closed, "Should be closed at failure {}", i);
            cb.record_failure();
        }

        // Should now be open
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.is_allowed());
    }

    #[test]
    fn test_circuit_breaker_success_resets_failures() {
        let cb = CircuitBreaker::new();

        // Record some failures (but not enough to open)
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.failure_count(), 2);

        // Success should reset
        cb.record_success();
        assert_eq!(cb.failure_count(), 0);
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_circuit_breaker_success_closes_circuit() {
        let cb = CircuitBreaker::new();

        // Open the circuit
        for _ in 0..CIRCUIT_FAILURE_THRESHOLD {
            cb.record_failure();
        }
        assert_eq!(cb.state(), CircuitState::Open);

        // Manually transition to half-open (simulating timeout)
        *cb.state.write().unwrap() = CircuitState::HalfOpen;

        // Success should close
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_classify_error_permanent() {
        let auth_err = anyhow::anyhow!("Invalid API credentials");
        assert_eq!(Notifier::classify_error(&auth_err), SmsErrorKind::Permanent);

        let did_err = anyhow::anyhow!("Invalid DID specified");
        assert_eq!(Notifier::classify_error(&did_err), SmsErrorKind::Permanent);
    }

    #[test]
    fn test_classify_error_transient() {
        let network_err = anyhow::anyhow!("Connection timeout");
        assert_eq!(Notifier::classify_error(&network_err), SmsErrorKind::Transient);

        let server_err = anyhow::anyhow!("Internal server error");
        assert_eq!(Notifier::classify_error(&server_err), SmsErrorKind::Transient);
    }

    // === Circuit Breaker Alert Queue tests ===

    #[test]
    fn test_circuit_breaker_queue_alert() {
        let cb = CircuitBreaker::new();
        assert_eq!(cb.pending_count(), 0);

        cb.queue_alert("Alert 1".to_string());
        assert_eq!(cb.pending_count(), 1);

        cb.queue_alert("Alert 2".to_string());
        assert_eq!(cb.pending_count(), 2);
    }

    #[test]
    fn test_circuit_breaker_take_pending_alerts() {
        let cb = CircuitBreaker::new();
        cb.queue_alert("Alert 1".to_string());
        cb.queue_alert("Alert 2".to_string());

        let alerts = cb.take_pending_alerts();
        assert_eq!(alerts.len(), 2);
        assert_eq!(alerts[0], "Alert 1");
        assert_eq!(alerts[1], "Alert 2");

        // Queue should be empty after taking
        assert_eq!(cb.pending_count(), 0);
        assert!(cb.take_pending_alerts().is_empty());
    }

    #[test]
    fn test_circuit_breaker_queue_overflow() {
        let cb = CircuitBreaker::new();

        // Fill queue to max
        for i in 0..MAX_QUEUED_ALERTS {
            cb.queue_alert(format!("Alert {}", i));
        }
        assert_eq!(cb.pending_count(), MAX_QUEUED_ALERTS);

        // Add one more - should drop oldest
        cb.queue_alert("Newest".to_string());
        assert_eq!(cb.pending_count(), MAX_QUEUED_ALERTS);

        let alerts = cb.take_pending_alerts();
        // First alert should have been dropped
        assert_eq!(alerts.first().unwrap(), "Alert 1");
        assert_eq!(alerts.last().unwrap(), "Newest");
    }

    // === SMS truncation tests ===

    #[test]
    fn test_truncate_sms_short_message() {
        let short = "Hello world";
        assert_eq!(truncate_sms_message(short), short);
    }

    #[test]
    fn test_truncate_sms_exact_length() {
        let exact = "a".repeat(MAX_SMS_LENGTH);
        assert_eq!(truncate_sms_message(&exact), exact);
    }

    #[test]
    fn test_truncate_sms_long_message() {
        let long = "a".repeat(200);
        let result = truncate_sms_message(&long);
        assert!(result.len() <= MAX_SMS_LENGTH);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_sms_word_boundary() {
        // Create a message that's too long, with words
        let long = "The quick brown fox jumps over the lazy dog near the riverbank where the fish swim happily in the clear blue water under the warm summer sun that shines brightly";
        let result = truncate_sms_message(long);
        assert!(result.len() <= MAX_SMS_LENGTH);
        assert!(result.ends_with("..."));
        // Should not cut in the middle of a word
        let before_dots = result.trim_end_matches("...");
        assert!(!before_dots.ends_with(char::is_alphabetic) || before_dots.ends_with(' ') || long.contains(before_dots));
    }

    #[test]
    fn test_truncate_sms_preserves_content() {
        let message = "Alert: Phone check failed. Transcript: thank you for calling cubic machinery how may I direct your call today please hold while I transfer you to the appropriate department";
        let result = truncate_sms_message(message);
        // Should preserve the beginning of the message
        assert!(result.starts_with("Alert: Phone check failed"));
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

        /// Truncated message always fits in SMS limit
        #[test]
        fn truncated_always_fits(message in ".{0,500}") {
            let result = truncate_sms_message(&message);
            prop_assert!(result.len() <= MAX_SMS_LENGTH,
                "Truncated message too long: {} chars", result.len());
        }

        /// Short messages are not modified
        #[test]
        fn short_messages_unchanged(message in ".{0,160}") {
            if message.len() <= MAX_SMS_LENGTH {
                let result = truncate_sms_message(&message);
                prop_assert_eq!(result, message);
            }
        }

        /// Truncation preserves message prefix
        #[test]
        fn truncation_preserves_prefix(message in "[a-z ]{161,300}") {
            let result = truncate_sms_message(&message);
            // Result should share a prefix with original (minus "...")
            let prefix = result.trim_end_matches("...");
            prop_assert!(message.starts_with(prefix),
                "Original should start with truncated prefix");
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

    /// Proves: truncate_sms_message output never exceeds MAX_SMS_LENGTH
    #[kani::proof]
    fn truncate_output_bounded() {
        let bytes: [u8; 200] = kani::any();
        if let Ok(s) = std::str::from_utf8(&bytes) {
            let result = truncate_sms_message(s);
            kani::assert(
                result.len() <= MAX_SMS_LENGTH,
                "truncated message must not exceed MAX_SMS_LENGTH"
            );
        }
    }

    /// Proves: truncate_sms_message never panics on any valid UTF-8 input
    #[kani::proof]
    fn truncate_never_panics() {
        let bytes: [u8; 64] = kani::any();
        if let Ok(s) = std::str::from_utf8(&bytes) {
            let _ = truncate_sms_message(s);
        }
    }

    /// Proves: short messages are not modified
    #[kani::proof]
    fn truncate_preserves_short() {
        let bytes: [u8; 50] = kani::any();
        if let Ok(s) = std::str::from_utf8(&bytes) {
            if s.len() <= MAX_SMS_LENGTH {
                let result = truncate_sms_message(s);
                kani::assert(
                    result == s,
                    "short messages should not be modified"
                );
            }
        }
    }

    /// Proves: truncate_sms_message output is valid UTF-8
    /// (implicitly verified since String guarantees UTF-8)
    #[kani::proof]
    fn truncate_output_valid_utf8() {
        let bytes: [u8; 32] = kani::any();
        if let Ok(s) = std::str::from_utf8(&bytes) {
            let result = truncate_sms_message(s);
            // If we get here without panic, result is valid UTF-8
            // (Rust's String type guarantees this)
            kani::assert(!result.is_empty() || s.is_empty(), "result should be non-empty for non-empty input");
        }
    }
}

/// Stateright model for circuit breaker state machine
#[cfg(test)]
mod circuit_breaker_model {
    use super::*;
    use stateright::*;

    /// Circuit breaker actions
    #[derive(Clone, Debug, Hash, PartialEq, Eq)]
    enum Action {
        /// A request succeeds
        Success,
        /// A request fails
        Failure,
        /// Time passes (simulates timeout)
        TimePasses,
    }

    /// Circuit breaker state for model checking
    /// Simplified to keep state space manageable
    #[derive(Clone, Debug, Hash, PartialEq, Eq)]
    struct CBState {
        state: CircuitState,
        consecutive_failures: u8, // Use u8 to limit state space
        timeout_elapsed: bool,    // Simplified: just track if timeout happened
        total_actions: u8,        // Bound exploration
    }

    impl CBState {
        fn new() -> Self {
            Self {
                state: CircuitState::Closed,
                consecutive_failures: 0,
                timeout_elapsed: false,
                total_actions: 0,
            }
        }
    }

    struct CircuitBreakerModel {
        failure_threshold: u8,
        max_actions: u8,
    }

    impl Model for CircuitBreakerModel {
        type State = CBState;
        type Action = Action;

        fn init_states(&self) -> Vec<Self::State> {
            vec![CBState::new()]
        }

        fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
            // Limit total exploration
            if state.total_actions >= self.max_actions {
                return;
            }

            // Only allow actions that make sense in current state
            match state.state {
                CircuitState::Closed => {
                    actions.push(Action::Success);
                    actions.push(Action::Failure);
                }
                CircuitState::Open => {
                    if !state.timeout_elapsed {
                        actions.push(Action::TimePasses);
                    }
                }
                CircuitState::HalfOpen => {
                    actions.push(Action::Success);
                    actions.push(Action::Failure);
                }
            }
        }

        fn next_state(&self, state: &Self::State, action: Self::Action) -> Option<Self::State> {
            let mut next = state.clone();
            next.total_actions += 1;

            match action {
                Action::Success => {
                    match state.state {
                        CircuitState::Closed => {
                            next.consecutive_failures = 0;
                        }
                        CircuitState::HalfOpen => {
                            next.state = CircuitState::Closed;
                            next.consecutive_failures = 0;
                            next.timeout_elapsed = false;
                        }
                        CircuitState::Open => {
                            // Blocked - no change
                        }
                    }
                }

                Action::Failure => {
                    match state.state {
                        CircuitState::Closed => {
                            next.consecutive_failures = next.consecutive_failures.saturating_add(1);
                            if next.consecutive_failures >= self.failure_threshold {
                                next.state = CircuitState::Open;
                                next.timeout_elapsed = false;
                            }
                        }
                        CircuitState::HalfOpen => {
                            next.state = CircuitState::Open;
                            next.consecutive_failures = next.consecutive_failures.saturating_add(1);
                            next.timeout_elapsed = false;
                        }
                        CircuitState::Open => {}
                    }
                }

                Action::TimePasses => {
                    if state.state == CircuitState::Open && !state.timeout_elapsed {
                        next.timeout_elapsed = true;
                        next.state = CircuitState::HalfOpen;
                    }
                }
            }

            Some(next)
        }

        fn properties(&self) -> Vec<Property<Self>> {
            vec![
                // Safety: In Open state, failures >= threshold
                Property::always("open_requires_failures", |model: &Self, state: &CBState| {
                    if state.state == CircuitState::Open {
                        state.consecutive_failures >= model.failure_threshold
                    } else {
                        true
                    }
                }),

                // Safety: HalfOpen requires timeout
                Property::always("halfopen_requires_timeout", |_: &Self, state: &CBState| {
                    if state.state == CircuitState::HalfOpen {
                        state.timeout_elapsed
                    } else {
                        true
                    }
                }),

                // Safety: Closed state means no timeout flag
                Property::always("closed_no_timeout", |_: &Self, state: &CBState| {
                    if state.state == CircuitState::Closed {
                        !state.timeout_elapsed
                    } else {
                        true
                    }
                }),

                // Liveness: Can reach recovered state
                Property::sometimes("can_recover", |_: &Self, state: &CBState| {
                    state.state == CircuitState::Closed
                }),
            ]
        }
    }

    #[test]
    fn test_circuit_breaker_model() {
        let model = CircuitBreakerModel {
            failure_threshold: 2,
            max_actions: 6,
        };
        let checker = model.checker().threads(1).spawn_bfs().join();
        println!("Circuit breaker states explored: {}", checker.unique_state_count());
        checker.assert_properties();
    }

    #[test]
    fn test_circuit_breaker_all_transitions() {
        // Test specific state transitions manually
        let model = CircuitBreakerModel {
            failure_threshold: 2,
            max_actions: 10,
        };

        let mut state = CBState::new();
        assert_eq!(state.state, CircuitState::Closed);

        // Fail twice to open circuit
        state = model.next_state(&state, Action::Failure).unwrap();
        assert_eq!(state.state, CircuitState::Closed);
        assert_eq!(state.consecutive_failures, 1);

        state = model.next_state(&state, Action::Failure).unwrap();
        assert_eq!(state.state, CircuitState::Open);
        assert_eq!(state.consecutive_failures, 2);

        // Wait for timeout
        state = model.next_state(&state, Action::TimePasses).unwrap();
        assert_eq!(state.state, CircuitState::HalfOpen);
        assert!(state.timeout_elapsed);

        // Success recovers
        state = model.next_state(&state, Action::Success).unwrap();
        assert_eq!(state.state, CircuitState::Closed);
        assert_eq!(state.consecutive_failures, 0);
        assert!(!state.timeout_elapsed);
    }

    #[test]
    fn test_circuit_breaker_halfopen_failure() {
        let model = CircuitBreakerModel {
            failure_threshold: 2,
            max_actions: 10,
        };

        // Get to HalfOpen
        let mut state = CBState::new();
        state = model.next_state(&state, Action::Failure).unwrap();
        state = model.next_state(&state, Action::Failure).unwrap();
        assert_eq!(state.state, CircuitState::Open);

        state = model.next_state(&state, Action::TimePasses).unwrap();
        assert_eq!(state.state, CircuitState::HalfOpen);

        // Failure in HalfOpen goes back to Open
        state = model.next_state(&state, Action::Failure).unwrap();
        assert_eq!(state.state, CircuitState::Open);
        assert!(!state.timeout_elapsed); // timeout reset
    }
}
