mod config;
mod health;
mod notify;
mod redact;
mod rtp;
mod scheduler;
mod sip;
mod speech;
mod stun;

use anyhow::Result;
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

/// Find an available port in the RTP range (16384-32767) and return bound socket
/// Returns both the socket and port to avoid race conditions between finding and rebinding
async fn find_available_rtp_socket() -> Result<(tokio::net::UdpSocket, u16)> {
    use tokio::net::UdpSocket;

    // Try ports in the standard RTP range
    for port in (16384..32768).step_by(2) {
        if let Ok(socket) = UdpSocket::bind(format!("0.0.0.0:{}", port)).await {
            return Ok((socket, port));
        }
    }

    // Fallback: let OS assign
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    let port = socket.local_addr()?.port();
    Ok((socket, port))
}

/// Parse command line arguments
struct Args {
    once: bool,
    record_pcap: Option<String>,
    validate: bool,
    help: bool,
}

fn parse_args() -> Args {
    let args: Vec<String> = std::env::args().collect();
    let mut result = Args {
        once: false,
        record_pcap: None,
        validate: false,
        help: false,
    };

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--once" => result.once = true,
            "--record-pcap" => {
                if i + 1 < args.len() {
                    i += 1;
                    result.record_pcap = Some(args[i].clone());
                    result.once = true; // Recording implies single run
                }
            }
            "--validate" => result.validate = true,
            "--help" | "-h" => result.help = true,
            _ => {}
        }
        i += 1;
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
    println!("    --record-pcap FILE  Record RTP packets to pcap file (implies --once)");
    println!("                        Requires: cargo build --features record");
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

    // Handle --record-pcap mode
    if let Some(pcap_file) = args.record_pcap {
        info!("Recording mode: RTP packets will be saved to {}", pcap_file);
        return run_with_recording(&config, &pcap_file).await;
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

/// Run a call with RTP packet recording enabled
async fn run_with_recording(config: &Config, pcap_file: &str) -> Result<()> {
    info!("Starting call with RTP recording...");

    // Create SIP client
    let sip_client = SipClient::new(config.clone()).await?;

    // Bind RTP socket FIRST to avoid race condition, then use for both recording and call
    let (rtp_socket, rtp_port) = find_available_rtp_socket().await?;
    info!("RTP port will be: {}", rtp_port);

    // Create RTP receiver from the already-bound socket
    let rtp_receiver = rtp::RtpReceiver::from_socket(rtp_socket);

    // Start recording in a background task
    // pcap captures at the network level, filtering by the port we've already bound
    let pcap_path = pcap_file.to_string();
    let listen_duration = Duration::from_secs(config.listen_duration_secs);
    let record_duration = listen_duration + Duration::from_secs(5);

    let record_handle = tokio::task::spawn_blocking(move || {
        rtp::recorder::record_rtp_to_file(&pcap_path, rtp_port, record_duration)
    });

    // Small delay to ensure pcap is ready
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Make the test call using the pre-bound receiver (no race condition)
    let cancel_token = CancellationToken::new();
    let call_result = sip_client
        .make_test_call_with_receiver(listen_duration, rtp_receiver, cancel_token)
        .await;

    // Wait a bit for any trailing packets
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Recording will stop on its own after duration

    match call_result {
        Ok(result) => {
            if result.connected {
                info!("Call completed. Audio samples: {}", result.audio_samples.len());
            } else {
                warn!("Call did not connect: {:?}", result.error);
            }
        }
        Err(e) => {
            error!("Call failed: {}", e);
        }
    }

    // Wait for recording to finish
    match record_handle.await {
        Ok(Ok(count)) => info!("Recording complete: {} packets saved to {}", count, pcap_file),
        Ok(Err(e)) => error!("Recording error: {}", e),
        Err(e) => error!("Recording task error: {}", e),
    }

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
