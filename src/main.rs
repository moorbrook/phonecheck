mod config;
mod health;
mod notify;
mod redact;
mod rtp;
mod scheduler;
mod sip;
mod speech;
mod stun;

use anyhow::{Context, Result};
use fs2::FileExt;
use std::fs::File;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use config::Config;
use health::HealthMetrics;
use notify::Notifier;
use scheduler::run_scheduler;
use sip::SipClient;
use speech::SpeechRecognizer;

/// Parse command line arguments
struct Args {
    once: bool,
    validate: bool,
    help: bool,
}

fn parse_args() -> Args {
    let args: Vec<String> = std::env::args().collect();
    let mut result = Args {
        once: false,
        validate: false,
        help: false,
    };

    for arg in args.iter().skip(1) {
        match arg.as_str() {
            "--once" => result.once = true,
            "--validate" => result.validate = true,
            "--help" | "-h" => result.help = true,
            _ => {}
        }
    }

    result
}

fn print_help() {
    println!("PhoneCheck - PBX Health Monitor\n");
    println!("USAGE:");
    println!("    phonecheck [OPTIONS]\n");
    println!("OPTIONS:");
    println!("    --once              Run a single check and exit");
    println!("    --validate          Validate configuration and exit");
    println!("    --help, -h          Show this help message\n");
    println!("ENVIRONMENT:");
    println!("    See .env.example for required configuration variables");
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = parse_args();

    if args.help {
        print_help();
        return Ok(());
    }

    // Acquire singleton lock (skip for --validate since it doesn't make calls)
    let _lock_file = if !args.validate {
        let lock_path = std::env::temp_dir().join("phonecheck.lock");
        let file = File::create(&lock_path)
            .with_context(|| format!("Failed to create lock file: {:?}", lock_path))?;
        file.try_lock_exclusive()
            .context("Another instance of phonecheck is already running")?;
        Some(file)
    } else {
        None
    };

    // Load .env file if present
    let _ = dotenvy::dotenv();

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
    info!("  Target phone: {}", redact::phone_number(&config.target_phone));
    info!("  SIP server: {}:{}", config.sip_server, config.sip_port);
    info!("  Expected phrase: \"{}\"", config.expected_phrase);
    info!("  Listen duration: {}s", config.listen_duration_secs);

    // Handle --validate mode
    if args.validate {
        info!("Validating configuration...");
        match config.validate() {
            Ok(()) => {
                info!("Configuration is valid");
                return Ok(());
            }
            Err(e) => {
                error!("{}", e);
                std::process::exit(1);
            }
        }
    }

    // Initialize speech recognizer
    let recognizer = Arc::new(SpeechRecognizer::new(
        &config.whisper_model_path,
        config.expected_phrase.clone(),
    )?);

    // Initialize notifier
    let notifier = Arc::new(Notifier::new(&config));
    info!("SMS notifier configured");

    // Wrap config in Arc for sharing
    let config = Arc::new(config);

    // Initialize health metrics
    let health_metrics = Arc::new(HealthMetrics::new());

    // Start health check server if configured
    let health_cancel = CancellationToken::new();
    if let Some(port) = config.health_port {
        let metrics = health_metrics.clone();
        let cancel = health_cancel.clone();
        tokio::spawn(async move {
            health::run_health_server(port, metrics, cancel).await;
        });
    }

    // Run a single check (for testing) or start scheduler
    if args.once {
        info!("Running single check (--once mode)");
        let cancel_token = CancellationToken::new();
        run_check(&config, &recognizer, &notifier, &health_metrics, cancel_token).await;
        health_cancel.cancel();
        return Ok(());
    }

    // Start scheduler - the closure receives a cancellation token for graceful shutdown
    run_scheduler(|cancel_token| {
        let config = config.clone();
        let recognizer = recognizer.clone();
        let notifier = notifier.clone();
        let health_metrics = health_metrics.clone();
        async move {
            run_check(&config, &recognizer, &notifier, &health_metrics, cancel_token).await;
        }
    })
    .await;

    health_cancel.cancel();

    Ok(())
}

async fn run_check(
    config: &Config,
    recognizer: &SpeechRecognizer,
    notifier: &Notifier,
    health_metrics: &HealthMetrics,
    cancel_token: CancellationToken,
) {
    info!("Starting PBX health check...");

    // Create SIP client
    let sip_client = match SipClient::new(config.clone()).await {
        Ok(client) => client,
        Err(e) => {
            error!("Failed to create SIP client: {}", e);
            health_metrics.record_failure();
            send_alert(notifier, &format!("PhoneCheck ERROR: Failed to connect to SIP server: {}", e)).await;
            return;
        }
    };

    // Make the test call with cancellation support
    let listen_duration = Duration::from_secs(config.listen_duration_secs);
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

    // Check audio with speech recognition
    let check_result = match recognizer.check_audio(&call_result.audio_samples) {
        Ok(result) => result,
        Err(e) => {
            error!("Speech recognition failed: {}", e);
            health_metrics.record_failure();
            send_alert(notifier, &format!("PhoneCheck ALERT: Speech recognition failed - {}", e)).await;
            return;
        }
    };

    info!("Transcribed: \"{}\"", check_result.transcript);

    if check_result.phrase_found {
        info!("SUCCESS: Expected phrase detected - PBX is healthy");
        health_metrics.record_success();
    } else {
        warn!(
            "ALERT: Expected phrase NOT detected. Heard: \"{}\"",
            check_result.transcript
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

async fn send_alert(notifier: &Notifier, message: &str) {
    if let Err(e) = notifier.send_alert(message).await {
        error!("Failed to send SMS alert: {}", e);
        // Log the original message so it's not lost
        error!("Original alert: {}", message);
    }
}
