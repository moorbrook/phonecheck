use anyhow::{Context, Result};
use fs2::FileExt;
use std::fs::File;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use phonecheck::cli::{parse_args, print_help};
use phonecheck::config::Config;
use phonecheck::health::{self, HealthMetrics};
use phonecheck::notify::Notifier;
use phonecheck::orchestrator;
use phonecheck::redact;
use phonecheck::scheduler::run_scheduler;
use phonecheck::speech::SpeechRecognizer;

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

    // Wrap config in Arc for sharing (do this early)
    let config = Arc::new(config);

    // Initialize speech recognizer (Mutex for interior mutability - embedding model needs &mut)
    let recognizer = Arc::new(std::sync::Mutex::new(SpeechRecognizer::new(
        &config.whisper_model_path,
    )?));

    // Initialize notifier
    let notifier = Arc::new(Notifier::new(&config));
    info!("Pushover notifier configured");

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
        orchestrator::run_check(&config, recognizer.as_ref(), &notifier, &health_metrics, cancel_token, args.save_audio.as_deref()).await;
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
            orchestrator::run_check(&config, recognizer.as_ref(), &notifier, &health_metrics, cancel_token, None).await;
        }
    })
    .await;

    health_cancel.cancel();

    Ok(())
}
