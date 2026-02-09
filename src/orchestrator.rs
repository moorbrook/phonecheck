//! PBX health check orchestration
//!
//! Coordinates SIP calls, audio capture, speech recognition, and alerting.

use anyhow::Result;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::health::HealthMetrics;
use crate::notify::Notifier;
use crate::sip::{CallResult, SipClient};
use crate::speech::{CheckResult, SpeechRecognizer};

/// Run a single PBX health check
pub async fn run_check(
    config: &Arc<Config>,
    recognizer_mutex: &std::sync::Mutex<SpeechRecognizer>,
    notifier: &Notifier,
    health_metrics: &HealthMetrics,
    cancel_token: CancellationToken,
    save_audio_path: Option<&str>,
) {
    info!("Starting PBX health check...");

    let call_result = match perform_call(config, cancel_token).await {
        Ok(res) => res,
        Err(e) => {
            handle_failure(health_metrics, notifier, &format!("PhoneCheck ERROR: {}", e)).await;
            return;
        }
    };

    if !validate_call_result(&call_result, health_metrics, notifier).await {
        return;
    }

    if let Some(path) = save_audio_path {
        save_audio(&call_result.audio_samples, path);
    }

    let check_result = match process_audio(recognizer_mutex, &call_result.audio_samples) {
        Ok(res) => res,
        Err(e) => {
            handle_failure(health_metrics, notifier, &format!("PhoneCheck ALERT: Speech recognition failed - {}", e)).await;
            return;
        }
    };

    report_result(check_result, health_metrics, notifier).await;
}

async fn perform_call(config: &Arc<Config>, cancel_token: CancellationToken) -> Result<CallResult> {
    let sip_client = SipClient::new(Arc::clone(config)).await?;
    let listen_duration = std::time::Duration::from_secs(config.listen_duration_secs);
    let result = sip_client.make_test_call_cancellable(listen_duration, cancel_token.clone()).await?;

    // Retry once on timeout/unreachable (transient network issue, stale NAT mapping)
    if !result.connected {
        if let Some(ref err) = result.error {
            if err.contains("timeout") || err.contains("No response") {
                warn!("First attempt failed ({}), retrying with fresh connection...", err);
                let sip_client = SipClient::new(Arc::clone(config)).await?;
                return sip_client.make_test_call_cancellable(listen_duration, cancel_token).await;
            }
        }
    }

    Ok(result)
}

async fn validate_call_result(
    result: &CallResult,
    health_metrics: &HealthMetrics,
    notifier: &Notifier,
) -> bool {
    if !result.connected {
        let error_msg = result.error.as_deref().unwrap_or("Unknown error");
        error!("Call did not connect: {}", error_msg);
        handle_failure(health_metrics, notifier, &format!("PhoneCheck ALERT: Call did not connect - {}", error_msg)).await;
        return false;
    }

    if !result.audio_received {
        warn!("Call connected but no audio received");
        handle_failure(health_metrics, notifier, "PhoneCheck ALERT: Call connected but no audio received").await;
        return false;
    }

    true
}

fn save_audio(samples: &[f32], path: &str) {
    match crate::rtp::save_wav(samples, path) {
        Ok(()) => info!("Saved audio to: {}", path),
        Err(e) => warn!("Failed to save audio: {}", e),
    }
}

fn process_audio(
    recognizer_mutex: &std::sync::Mutex<SpeechRecognizer>,
    samples: &[f32],
) -> Result<CheckResult> {
    let mut recognizer = recognizer_mutex.lock().map_err(|e| anyhow::anyhow!("Failed to lock recognizer: {}", e))?;
    recognizer.check_audio(samples)
}

async fn report_result(
    result: CheckResult,
    health_metrics: &HealthMetrics,
    notifier: &Notifier,
) {
    info!("Transcribed: \"{}\"", result.transcript);
    if let Some(similarity) = result.similarity {
        info!("Embedding similarity: {:.4}", similarity);
    }

    if result.phrase_found {
        info!("SUCCESS: Expected phrase detected - PBX is healthy");
        health_metrics.record_success();
    } else {
        warn!(
            "ALERT: Expected phrase NOT detected. Heard: \"{}\", similarity: {:?}",
            result.transcript,
            result.similarity
        );
        handle_failure(
            health_metrics,
            notifier,
            &format!(
                "PhoneCheck ALERT: Expected greeting not detected. Heard: \"{}\"",
                result.transcript
            ),
        )
        .await;
    }
}

async fn handle_failure(health_metrics: &HealthMetrics, notifier: &Notifier, message: &str) {
    let was_healthy = health_metrics.status().last_check_ok;
    health_metrics.record_failure();

    if was_healthy {
        // First failure after success — send alert
        if let Err(e) = notifier.send_alert(message).await {
            error!("Failed to send push notification: {}", e);
            error!("Original alert: {}", message);
        }
    } else {
        // Consecutive failure — suppress to avoid alert spam
        warn!("Consecutive failure (alert suppressed): {}", message);
    }
}
