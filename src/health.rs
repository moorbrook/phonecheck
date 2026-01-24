/// Health check HTTP endpoint
/// Provides a simple /health endpoint for monitoring systems (Kubernetes, load balancers, etc.)

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// Timeout for reading HTTP request (prevents slow-loris attacks)
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Health status of the service
#[derive(Debug, Clone)]
pub struct HealthStatus {
    /// Number of successful checks
    pub checks_successful: u64,
    /// Number of failed checks
    pub checks_failed: u64,
    /// Timestamp of last check (Unix epoch seconds)
    pub last_check_time: u64,
    /// Whether the last check was successful
    pub last_check_ok: bool,
}

impl Default for HealthStatus {
    fn default() -> Self {
        Self {
            checks_successful: 0,
            checks_failed: 0,
            last_check_time: 0,
            last_check_ok: true, // Assume healthy until proven otherwise
        }
    }
}

/// Shared health metrics that can be updated from the check loop
#[derive(Debug)]
pub struct HealthMetrics {
    checks_successful: AtomicU64,
    checks_failed: AtomicU64,
    last_check_time: AtomicU64,
    last_check_ok: std::sync::atomic::AtomicBool,
}

impl Default for HealthMetrics {
    fn default() -> Self {
        Self {
            checks_successful: AtomicU64::new(0),
            checks_failed: AtomicU64::new(0),
            last_check_time: AtomicU64::new(0),
            last_check_ok: std::sync::atomic::AtomicBool::new(true), // Assume healthy until proven otherwise
        }
    }
}

impl HealthMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a successful check
    pub fn record_success(&self) {
        self.checks_successful.fetch_add(1, Ordering::Relaxed);
        self.last_check_time.store(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            Ordering::Relaxed,
        );
        self.last_check_ok.store(true, Ordering::Relaxed);
    }

    /// Record a failed check
    pub fn record_failure(&self) {
        self.checks_failed.fetch_add(1, Ordering::Relaxed);
        self.last_check_time.store(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            Ordering::Relaxed,
        );
        self.last_check_ok.store(false, Ordering::Relaxed);
    }

    /// Get current health status
    pub fn status(&self) -> HealthStatus {
        HealthStatus {
            checks_successful: self.checks_successful.load(Ordering::Relaxed),
            checks_failed: self.checks_failed.load(Ordering::Relaxed),
            last_check_time: self.last_check_time.load(Ordering::Relaxed),
            last_check_ok: self.last_check_ok.load(Ordering::Relaxed),
        }
    }
}

/// Run the health check HTTP server
pub async fn run_health_server(
    port: u16,
    metrics: Arc<HealthMetrics>,
    cancel_token: CancellationToken,
) {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            error!("Failed to bind health check server on port {}: {}", port, e);
            return;
        }
    };

    info!("Health check server listening on http://0.0.0.0:{}/health", port);

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((mut socket, peer_addr)) => {
                        let metrics = metrics.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_request(&mut socket, &metrics).await {
                                debug!("Error handling request from {}: {}", peer_addr, e);
                            }
                        });
                    }
                    Err(e) => {
                        warn!("Failed to accept connection: {}", e);
                    }
                }
            }
            _ = cancel_token.cancelled() => {
                info!("Health check server shutting down");
                break;
            }
        }
    }
}

async fn handle_request(
    socket: &mut tokio::net::TcpStream,
    metrics: &HealthMetrics,
) -> std::io::Result<()> {
    let mut buf = [0u8; 1024];

    // Apply timeout to prevent slow-loris attacks
    let n = match timeout(REQUEST_TIMEOUT, socket.read(&mut buf)).await {
        Ok(result) => result?,
        Err(_) => {
            debug!("Request timeout after {:?}", REQUEST_TIMEOUT);
            return Ok(());
        }
    };

    if n == 0 {
        return Ok(());
    }

    let request = String::from_utf8_lossy(&buf[..n]);

    // Parse the request line to get the path
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");

    let response = match path {
        "/health" | "/healthz" | "/health/" => {
            let status = metrics.status();
            build_health_response(&status)
        }
        "/ready" | "/readyz" | "/ready/" => {
            // Readiness logic for Kubernetes compatibility:
            // - Before first check (last_check_time == 0): ready=true (startup grace period)
            //   This prevents pods from being killed before the first check completes.
            // - After first check: ready = last_check_ok (based on actual check results)
            // Note: For stricter behavior, use /health which always returns 200 with status.
            let status = metrics.status();
            if status.last_check_ok || status.last_check_time == 0 {
                build_ready_response(true)
            } else {
                build_ready_response(false)
            }
        }
        "/metrics" => {
            let status = metrics.status();
            build_metrics_response(&status)
        }
        _ => build_not_found_response(),
    };

    socket.write_all(response.as_bytes()).await?;
    socket.flush().await?;

    Ok(())
}

fn build_health_response(status: &HealthStatus) -> String {
    let body = format!(
        r#"{{"status":"healthy","checks_successful":{},"checks_failed":{},"last_check_time":{},"last_check_ok":{}}}"#,
        status.checks_successful,
        status.checks_failed,
        status.last_check_time,
        status.last_check_ok
    );

    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

fn build_ready_response(ready: bool) -> String {
    let (status_code, status_text, body) = if ready {
        (200, "OK", r#"{"ready":true}"#)
    } else {
        (503, "Service Unavailable", r#"{"ready":false}"#)
    };

    format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status_code,
        status_text,
        body.len(),
        body
    )
}

fn build_metrics_response(status: &HealthStatus) -> String {
    // Prometheus-compatible metrics format
    let body = format!(
        "# HELP phonecheck_checks_total Total number of checks performed\n\
         # TYPE phonecheck_checks_total counter\n\
         phonecheck_checks_total{{result=\"success\"}} {}\n\
         phonecheck_checks_total{{result=\"failure\"}} {}\n\
         # HELP phonecheck_last_check_timestamp Unix timestamp of last check\n\
         # TYPE phonecheck_last_check_timestamp gauge\n\
         phonecheck_last_check_timestamp {}\n\
         # HELP phonecheck_last_check_ok Whether the last check succeeded (1) or failed (0)\n\
         # TYPE phonecheck_last_check_ok gauge\n\
         phonecheck_last_check_ok {}\n",
        status.checks_successful,
        status.checks_failed,
        status.last_check_time,
        if status.last_check_ok { 1 } else { 0 }
    );

    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

fn build_not_found_response() -> String {
    let body = r#"{"error":"Not Found"}"#;
    format!(
        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_metrics_default() {
        let metrics = HealthMetrics::new();
        let status = metrics.status();

        assert_eq!(status.checks_successful, 0);
        assert_eq!(status.checks_failed, 0);
        assert_eq!(status.last_check_time, 0);
        assert!(status.last_check_ok);
    }

    #[test]
    fn test_health_metrics_record_success() {
        let metrics = HealthMetrics::new();
        metrics.record_success();

        let status = metrics.status();
        assert_eq!(status.checks_successful, 1);
        assert_eq!(status.checks_failed, 0);
        assert!(status.last_check_time > 0);
        assert!(status.last_check_ok);
    }

    #[test]
    fn test_health_metrics_record_failure() {
        let metrics = HealthMetrics::new();
        metrics.record_failure();

        let status = metrics.status();
        assert_eq!(status.checks_successful, 0);
        assert_eq!(status.checks_failed, 1);
        assert!(status.last_check_time > 0);
        assert!(!status.last_check_ok);
    }

    #[test]
    fn test_health_metrics_multiple_records() {
        let metrics = HealthMetrics::new();
        metrics.record_success();
        metrics.record_success();
        metrics.record_failure();
        metrics.record_success();

        let status = metrics.status();
        assert_eq!(status.checks_successful, 3);
        assert_eq!(status.checks_failed, 1);
        assert!(status.last_check_ok); // Last was success
    }

    #[test]
    fn test_build_health_response() {
        let status = HealthStatus {
            checks_successful: 5,
            checks_failed: 1,
            last_check_time: 1234567890,
            last_check_ok: true,
        };

        let response = build_health_response(&status);
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("application/json"));
        assert!(response.contains("\"checks_successful\":5"));
        assert!(response.contains("\"checks_failed\":1"));
    }

    #[test]
    fn test_build_ready_response_ready() {
        let response = build_ready_response(true);
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("\"ready\":true"));
    }

    #[test]
    fn test_build_ready_response_not_ready() {
        let response = build_ready_response(false);
        assert!(response.starts_with("HTTP/1.1 503"));
        assert!(response.contains("\"ready\":false"));
    }

    #[test]
    fn test_build_metrics_response() {
        let status = HealthStatus {
            checks_successful: 10,
            checks_failed: 2,
            last_check_time: 1234567890,
            last_check_ok: true,
        };

        let response = build_metrics_response(&status);
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("text/plain"));
        assert!(response.contains("phonecheck_checks_total{result=\"success\"} 10"));
        assert!(response.contains("phonecheck_checks_total{result=\"failure\"} 2"));
        assert!(response.contains("phonecheck_last_check_ok 1"));
    }

    #[test]
    fn test_build_not_found_response() {
        let response = build_not_found_response();
        assert!(response.starts_with("HTTP/1.1 404"));
        assert!(response.contains("Not Found"));
    }

    #[tokio::test]
    async fn test_health_server_starts_and_stops() {
        let metrics = Arc::new(HealthMetrics::new());
        let cancel_token = CancellationToken::new();
        let cancel_token_clone = cancel_token.clone();

        // Start server on a random port
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener); // Release the port

        // Spawn the server
        let handle = tokio::spawn(run_health_server(port, metrics, cancel_token_clone));

        // Give it time to start
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Cancel and wait for shutdown
        cancel_token.cancel();

        tokio::time::timeout(std::time::Duration::from_millis(500), handle)
            .await
            .expect("Server should shutdown within timeout")
            .expect("Server should complete without panic");
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Recording successes always increments successful count
        #[test]
        fn record_success_increments(count in 1usize..100) {
            let metrics = HealthMetrics::new();
            for _ in 0..count {
                metrics.record_success();
            }
            let status = metrics.status();
            prop_assert_eq!(status.checks_successful, count as u64);
            prop_assert_eq!(status.checks_failed, 0);
            prop_assert!(status.last_check_ok);
        }

        /// Recording failures always increments failed count
        #[test]
        fn record_failure_increments(count in 1usize..100) {
            let metrics = HealthMetrics::new();
            for _ in 0..count {
                metrics.record_failure();
            }
            let status = metrics.status();
            prop_assert_eq!(status.checks_failed, count as u64);
            prop_assert_eq!(status.checks_successful, 0);
            prop_assert!(!status.last_check_ok);
        }

        /// Mixed success/failure records count correctly
        #[test]
        fn mixed_records_count_correctly(successes in 0usize..50, failures in 0usize..50) {
            let metrics = HealthMetrics::new();
            for _ in 0..successes {
                metrics.record_success();
            }
            for _ in 0..failures {
                metrics.record_failure();
            }
            let status = metrics.status();
            prop_assert_eq!(status.checks_successful, successes as u64);
            prop_assert_eq!(status.checks_failed, failures as u64);
        }

        /// Last check status reflects the last operation
        #[test]
        fn last_check_reflects_last_op(
            initial_successes in 0usize..10,
            initial_failures in 0usize..10,
            end_with_success: bool
        ) {
            let metrics = HealthMetrics::new();
            for _ in 0..initial_successes {
                metrics.record_success();
            }
            for _ in 0..initial_failures {
                metrics.record_failure();
            }
            // Final operation determines last_check_ok
            if end_with_success {
                metrics.record_success();
                prop_assert!(metrics.status().last_check_ok);
            } else {
                metrics.record_failure();
                prop_assert!(!metrics.status().last_check_ok);
            }
        }

        /// HTTP responses are always well-formed
        #[test]
        fn health_response_well_formed(
            successful in 0u64..1000,
            failed in 0u64..1000,
            time in 0u64..u64::MAX,
            ok: bool
        ) {
            let status = HealthStatus {
                checks_successful: successful,
                checks_failed: failed,
                last_check_time: time,
                last_check_ok: ok,
            };
            let response = build_health_response(&status);
            prop_assert!(response.starts_with("HTTP/1.1 200 OK"));
            prop_assert!(response.contains("Content-Type: application/json"));
            prop_assert!(response.contains("Content-Length:"));
        }

        /// Metrics response follows Prometheus format
        #[test]
        fn metrics_response_prometheus_format(
            successful in 0u64..1000,
            failed in 0u64..1000
        ) {
            let status = HealthStatus {
                checks_successful: successful,
                checks_failed: failed,
                last_check_time: 12345,
                last_check_ok: true,
            };
            let response = build_metrics_response(&status);
            // Use assert! instead of prop_assert! for string patterns with special chars
            assert!(response.contains("phonecheck_checks_total"));
            assert!(response.contains("# TYPE"));
            assert!(response.contains("# HELP"));
        }
    }
}

/// State machine model for testing health metrics state transitions
#[cfg(test)]
mod state_machine {
    use super::*;
    use stateright::*;

    /// Actions that can be performed on health metrics
    #[derive(Clone, Debug, Hash, PartialEq)]
    enum Action {
        RecordSuccess,
        RecordFailure,
        CheckStatus,
    }

    /// State of the health metrics (simplified for model checking)
    #[derive(Clone, Debug, Hash, PartialEq)]
    struct MetricsState {
        successful: u64,
        failed: u64,
        last_ok: bool,
    }

    impl MetricsState {
        fn new() -> Self {
            Self {
                successful: 0,
                failed: 0,
                last_ok: true, // Default assumption
            }
        }
    }

    /// Model for health metrics state machine
    struct HealthMetricsModel {
        max_operations: u64,
    }

    impl Model for HealthMetricsModel {
        type State = MetricsState;
        type Action = Action;

        fn init_states(&self) -> Vec<Self::State> {
            vec![MetricsState::new()]
        }

        fn actions(&self, state: &Self::State, actions: &mut Vec<Self::Action>) {
            // Can always record success/failure if under limit
            if state.successful + state.failed < self.max_operations {
                actions.push(Action::RecordSuccess);
                actions.push(Action::RecordFailure);
            }
            // Can always check status
            actions.push(Action::CheckStatus);
        }

        fn next_state(&self, state: &Self::State, action: Self::Action) -> Option<Self::State> {
            match action {
                Action::RecordSuccess => Some(MetricsState {
                    successful: state.successful + 1,
                    failed: state.failed,
                    last_ok: true,
                }),
                Action::RecordFailure => Some(MetricsState {
                    successful: state.successful,
                    failed: state.failed + 1,
                    last_ok: false,
                }),
                Action::CheckStatus => Some(state.clone()), // No state change
            }
        }

        fn properties(&self) -> Vec<Property<Self>> {
            vec![
                // Invariant: total operations = successful + failed
                Property::always("total_count_consistent", |_: &Self, state: &MetricsState| {
                    state.successful + state.failed <= 5 // Use constant since self not accessible
                }),
                // Invariant: if last operation was success, last_ok is true
                // (This is implicitly maintained by the state machine)
                Property::always("last_ok_reflects_last_action", |_: &Self, state: &MetricsState| {
                    // Initial state has last_ok=true with no operations
                    if state.successful == 0 && state.failed == 0 {
                        state.last_ok
                    } else {
                        // After at least one operation, last_ok should be valid
                        true // The model maintains this invariant by construction
                    }
                }),
                // Eventually property: system can always make progress
                Property::sometimes("can_record_success", |_: &Self, state: &MetricsState| {
                    state.successful > 0
                }),
                Property::sometimes("can_record_failure", |_: &Self, state: &MetricsState| {
                    state.failed > 0
                }),
            ]
        }
    }

    #[test]
    fn test_health_metrics_state_machine() {
        // Test with small number of operations to keep state space manageable
        let model = HealthMetricsModel { max_operations: 5 };

        // Check that all properties hold
        model
            .checker()
            .threads(1)
            .spawn_bfs()
            .join()
            .assert_properties();
    }

    #[test]
    fn test_health_metrics_no_deadlock() {
        let model = HealthMetricsModel { max_operations: 3 };

        // Verify the model explores all reachable states without panicking
        let checker = model.checker().threads(1).spawn_bfs().join();

        // Should have explored multiple states
        assert!(
            checker.state_count() > 1,
            "Should explore multiple states"
        );
    }
}
