# Health Check and Metrics Research

## Current State

PhoneCheck has no health check endpoint or metrics collection. For production monitoring, these are essential.

## Recommended Approach

For a simple daemon like PhoneCheck, use a lightweight HTTP server for health checks and Prometheus metrics.

### Option 1: Minimal with axum (Recommended)

Add a small HTTP server alongside the main scheduler:

```rust
use axum::{routing::get, Router, response::IntoResponse, Json};
use serde::Serialize;

#[derive(Serialize)]
struct HealthStatus {
    status: &'static str,
    last_check: Option<String>,
    last_result: Option<bool>,
}

async fn health_check() -> impl IntoResponse {
    Json(HealthStatus {
        status: "healthy",
        last_check: None,  // TODO: populate from shared state
        last_result: None,
    })
}

async fn run_health_server() {
    let app = Router::new()
        .route("/health", get(health_check));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
```

### Option 2: With Prometheus Metrics

Use [axum-prometheus](https://crates.io/crates/axum-prometheus) or [metrics-exporter-prometheus](https://crates.io/crates/metrics-exporter-prometheus):

```rust
use metrics::{counter, gauge};

// In check function
counter!("phonecheck_calls_total", "result" => "success").increment(1);
counter!("phonecheck_calls_total", "result" => "failure").increment(1);
gauge!("phonecheck_last_check_timestamp").set(timestamp);
gauge!("phonecheck_last_check_success").set(1.0); // or 0.0
```

### Useful Metrics for PhoneCheck

| Metric | Type | Description |
|--------|------|-------------|
| `phonecheck_calls_total` | Counter | Total calls made (labels: result=success/failure) |
| `phonecheck_call_duration_seconds` | Histogram | Call duration |
| `phonecheck_transcription_duration_seconds` | Histogram | Whisper processing time |
| `phonecheck_phrase_match` | Counter | Phrase matches (labels: matched=true/false) |
| `phonecheck_sms_alerts_total` | Counter | SMS alerts sent |
| `phonecheck_last_check_timestamp` | Gauge | Unix timestamp of last check |
| `phonecheck_uptime_seconds` | Gauge | Service uptime |

## Dependencies

```toml
[dependencies]
axum = "0.7"
metrics = "0.22"
metrics-exporter-prometheus = "0.14"
serde = { version = "1", features = ["derive"] }
```

## Implementation Notes

1. **Shared state**: Use `Arc<RwLock<HealthState>>` to share state between scheduler and HTTP server
2. **Graceful shutdown**: Both scheduler and HTTP server should respond to shutdown signal
3. **Port configuration**: Make health check port configurable via env var

## Liveness vs Readiness

For Kubernetes deployments:
- **/health/live**: Is the process running? (always returns 200 unless crashed)
- **/health/ready**: Is the service ready to perform checks? (SIP connection OK, Whisper model loaded)

## Sources
- [Real-World Observability in Rust](https://medium.com/@wedevare/rust-real-world-observability-health-checks-metrics-tracing-and-logs-fd229ea8ec96)
- [axum-prometheus](https://lib.rs/crates/axum-prometheus)
- [metrics-exporter-prometheus](https://crates.io/crates/metrics-exporter-prometheus)
- [autometrics](https://docs.rs/autometrics)
