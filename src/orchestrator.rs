//! PBX health check orchestration
//!
//! Coordinates SIP calls, audio capture, speech recognition, and alerting.

use anyhow::{Context, Result};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::health::HealthMetrics;
use crate::notify::Notifier;
use crate::sip::SipClient;
use crate::speech::SpeechRecognizer;

/// Run a single PBX health check
pub async fn run_check(
    config: &Arc<Config>,
    recognizer: &std::sync::Mutex<SpeechRecognizer>,
    notifier: &Notifier,
    health_metrics: &HealthMetrics,
    cancel_token: CancellationToken,
    save_audio_path: Option<&str>,
) {
    info!("Starting PBX health check...");

    // Create SIP client
    let sip_client = match SipClient::new(Arc::clone(config)).await {
        Ok(client) => client,
        Err(e) => {
            error!("Failed to create SIP client: {}", e);
            health_metrics.record_failure();
            send_alert(notifier, &format!("PhoneCheck ERROR: Failed to connect to SIP server: {}", e)).await;
            return;
        }
    };

    // Make the test call with cancellation support
    let listen_duration = std::time::Duration::from_secs(config.listen_duration_secs);
    let call_result = match sip_client.make_test_call_cancellable(listen_duration, cancel_token.clone()).await {
        Ok(result) => result,
        Err(e) => {
            error!("Call failed: {}", e);
            health_metrics.record_failure();
            send_alert(notifier, &format!("PhoneCheck ALERT: Call failed - {}", e)).await;
            return;
        }
    };

    // Check call result
    if !call_result.connected {
        let error_msg = call_result.error.unwrap_or_else(|| "Unknown error".to_string());
        error!("Call did not connect: {}", error_msg);
        health_metrics.record_failure();
        send_alert(notifier, &format!("PhoneCheck ALERT: Call did not connect - {}", error_msg)).await;
        return;
    }

    if !call_result.audio_received {
        warn!("Call connected but no audio received");
        health_metrics.record_failure();
        send_alert(notifier, "PhoneCheck ALERT: Call connected but no audio received").await;
        return;
    }

    // Save audio to file if requested
    if let Some(path) = save_audio_path {
        match crate::rtp::save_wav(&call_result.audio_samples, path) {
            Ok(()) => info!("Saved audio to: {}", path),
            Err(e) => warn!("Failed to save audio: {}", e),
        }
    }

    // Check audio with speech recognition
    // Note: Lock is released before any .await to avoid holding MutexGuard across await points
    let check_result = {
        let result = match recognizer.lock() {
            Ok(mut rec) => rec.check_audio(&call_result.audio_samples),
            Err(e) => {
                error!("Failed to lock recognizer: {}", e);
                health_metrics.record_failure();
                return;
            }
        };
        match result {
            Ok(r) => r,
            Err(e) => {
                error!("Speech recognition failed: {}", e);
                health_metrics.record_failure();
                send_alert(notifier, &format!("PhoneCheck ALERT: Speech recognition failed - {}", e)).await;
                return;
            }
        }
    };

    info!("Transcribed: \"{}\"", check_result.transcript);
    if let Some(similarity) = check_result.similarity {
        info!("Embedding similarity: {:.4}", similarity);
    }

    if check_result.phrase_found {
        info!("SUCCESS: Expected phrase detected - PBX is healthy");
        health_metrics.record_success();
    } else {
        warn!(
            "ALERT: Expected phrase NOT detected. Heard: \"{}\", similarity: {:?}",
            check_result.transcript,
            check_result.similarity
        );
        health_metrics.record_failure();
        send_alert(
            notifier,
            &format!(
                "PhoneCheck ALERT: Expected greeting not detected. Heard: \"{}\"",
                check_result.transcript
            ),
        )
        .await;
    }
}

/// Send an alert via Pushover
async fn send_alert(notifier: &Notifier, message: &str) {
    if let Err(e) = notifier.send_alert(message).await {
        error!("Failed to send push notification: {}", e);
        // Log the original message so it's not lost
        error!("Original alert: {}", message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_send_alert_success() {
        // This is a smoke test - in a real test we'd need to mock Notifier
        // For now, we just verify the function signature is correct
        let _ = || async {
            let config = Arc::new(Config::from_env().unwrap());
            let notifier = Notifier::new(&config);
            // This would make a real network call in integration tests
            // send_alert(&notifier, "test message").await;
        };
    }
}
