mod config;
mod notify;
mod rtp;
mod scheduler;
mod sip;
mod speech;

use anyhow::Result;
use std::time::Duration;
use tracing::{error, info, warn};

use config::Config;
use notify::Notifier;
use scheduler::run_scheduler;
use sip::SipClient;
use speech::SpeechRecognizer;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("phonecheck=info".parse().unwrap()),
        )
        .init();

    info!("PhoneCheck PBX Monitor v{}", env!("CARGO_PKG_VERSION"));

    // Load configuration
    let config = Config::from_env()?;
    info!("Configuration loaded");
    info!("  Target phone: {}", config.target_phone);
    info!("  SIP server: {}:{}", config.sip_server, config.sip_port);
    info!("  Expected phrase: \"{}\"", config.expected_phrase);
    info!("  Listen duration: {}s", config.listen_duration_secs);

    // Initialize speech recognizer
    let recognizer = SpeechRecognizer::new(
        &config.whisper_model_path,
        config.expected_phrase.clone(),
    )?;

    // Initialize notifier
    let notifier = Notifier::new(&config);
    info!("SMS notifier configured");

    // Run a single check (for testing) or start scheduler
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "--once" {
        info!("Running single check (--once mode)");
        run_check(&config, &recognizer, &notifier).await;
        return Ok(());
    }

    // Start scheduler
    run_scheduler(|| run_check(&config, &recognizer, &notifier)).await;

    Ok(())
}

async fn run_check(config: &Config, recognizer: &SpeechRecognizer, notifier: &Notifier) {
    info!("Starting PBX health check...");

    // Create SIP client
    let sip_client = match SipClient::new(config.clone()).await {
        Ok(client) => client,
        Err(e) => {
            error!("Failed to create SIP client: {}", e);
            send_alert(notifier, &format!("PhoneCheck ERROR: Failed to connect to SIP server: {}", e)).await;
            return;
        }
    };

    // Make the test call
    let listen_duration = Duration::from_secs(config.listen_duration_secs);
    let call_result = match sip_client.make_test_call(listen_duration).await {
        Ok(result) => result,
        Err(e) => {
            error!("Call failed: {}", e);
            send_alert(notifier, &format!("PhoneCheck ALERT: Call failed - {}", e)).await;
            return;
        }
    };

    // Check call result
    if !call_result.connected {
        let error_msg = call_result.error.unwrap_or_else(|| "Unknown error".to_string());
        error!("Call did not connect: {}", error_msg);
        send_alert(notifier, &format!("PhoneCheck ALERT: Call did not connect - {}", error_msg)).await;
        return;
    }

    if !call_result.audio_received {
        warn!("Call connected but no audio received");
        send_alert(notifier, "PhoneCheck ALERT: Call connected but no audio received").await;
        return;
    }

    // Check audio with speech recognition
    let check_result = match recognizer.check_audio(&call_result.audio_samples) {
        Ok(result) => result,
        Err(e) => {
            error!("Speech recognition failed: {}", e);
            send_alert(notifier, &format!("PhoneCheck ALERT: Speech recognition failed - {}", e)).await;
            return;
        }
    };

    info!("Transcribed: \"{}\"", check_result.transcript);

    if check_result.phrase_found {
        info!("SUCCESS: Expected phrase detected - PBX is healthy");
    } else {
        warn!(
            "ALERT: Expected phrase NOT detected. Heard: \"{}\"",
            check_result.transcript
        );
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

async fn send_alert(notifier: &Notifier, message: &str) {
    if let Err(e) = notifier.send_alert(message).await {
        error!("Failed to send SMS alert: {}", e);
        // Log the original message so it's not lost
        error!("Original alert: {}", message);
    }
}
