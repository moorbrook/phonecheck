# SMS API Retry and Circuit Breaker Research

## Current Implementation

PhoneCheck's `notify.rs` has basic exponential backoff with a fixed number of retries. This research explores improvements.

## Recommended Crates

### 1. [tokio-retry2](https://docs.rs/tokio-retry2) (Recommended)

Enhanced retry with conditional exit based on error type.

```rust
use tokio_retry2::{Retry, RetryError};
use tokio_retry2::strategy::{ExponentialBackoff, jitter};

async fn send_sms_with_retry(message: &str) -> Result<(), Error> {
    let strategy = ExponentialBackoff::from_millis(1000)
        .max_delay(Duration::from_secs(60))
        .map(jitter)
        .take(5);

    Retry::spawn(strategy, || async {
        send_sms(message).await.map_err(|e| {
            // Don't retry on authentication errors
            if e.is_auth_error() {
                RetryError::permanent(e)
            } else {
                RetryError::transient(e)
            }
        })
    }).await
}
```

### 2. [backoff](https://docs.rs/backoff)

Standard exponential backoff implementation.

```rust
use backoff::{ExponentialBackoff, retry};

let backoff = ExponentialBackoff {
    initial_interval: Duration::from_millis(500),
    max_interval: Duration::from_secs(60),
    max_elapsed_time: Some(Duration::from_secs(300)),
    ..Default::default()
};
```

### 3. [failsafe-rs](https://github.com/dmexe/failsafe-rs) (Circuit Breaker)

Full circuit breaker implementation.

```rust
use failsafe::{CircuitBreaker, Config};

let config = Config::new()
    .failure_policy(consecutive_failures(3))
    .success_policy(consecutive_successes(2))
    .half_open_max_calls(1);

let circuit = CircuitBreaker::new(config);

// In send_sms:
circuit.call(|| send_sms_impl(message)).await
```

### 4. [backon](https://docs.rs/backon)

Ergonomic retry without ceremony.

```rust
use backon::{Retryable, ExponentialBuilder};

async fn send() -> Result<()> {
    send_sms(msg)
        .retry(&ExponentialBuilder::default())
        .await
}
```

## Circuit Breaker States

```
         ┌─────────────────────────────────────────┐
         │                                         │
         │  ┌────────┐      failures >= threshold  │
         │  │ CLOSED │─────────────────────────────┼────►┌──────┐
         │  └────────┘                             │     │ OPEN │
         │       ▲                                 │     └──┬───┘
         │       │                                 │        │
         │  success_threshold                      │   timeout expires
         │  reached                                │        │
         │       │                                 │        ▼
         │  ┌────┴─────┐                           │  ┌───────────┐
         │  │HALF_OPEN │◄──────────────────────────┼──│  (wait)   │
         │  └────┬─────┘                           │  └───────────┘
         │       │                                 │
         │       │ failure                         │
         │       └─────────────────────────────────┼────►┌──────┐
         │                                         │     │ OPEN │
         │                                         │     └──────┘
         └─────────────────────────────────────────┘
```

## Recommended Implementation for PhoneCheck

### Simple Approach (Current + Improvements)

Keep the existing backoff but add:
1. Error classification (retryable vs permanent)
2. Circuit breaker for repeated failures
3. Alerting escalation

```rust
pub struct SmsNotifier {
    circuit_state: CircuitState,
    consecutive_failures: u32,
    last_success: Option<Instant>,
}

enum CircuitState {
    Closed,
    Open { until: Instant },
    HalfOpen,
}

impl SmsNotifier {
    const FAILURE_THRESHOLD: u32 = 3;
    const OPEN_DURATION: Duration = Duration::from_secs(300); // 5 minutes

    pub async fn send_alert(&mut self, message: &str) -> Result<()> {
        // Check circuit state
        match &self.circuit_state {
            CircuitState::Open { until } if Instant::now() < *until => {
                warn!("Circuit open, SMS not sent: {}", message);
                return Err(Error::CircuitOpen);
            }
            CircuitState::Open { .. } => {
                self.circuit_state = CircuitState::HalfOpen;
            }
            _ => {}
        }

        // Try to send
        let result = self.send_with_retry(message).await;

        // Update circuit state
        match &result {
            Ok(_) => {
                self.consecutive_failures = 0;
                self.last_success = Some(Instant::now());
                self.circuit_state = CircuitState::Closed;
            }
            Err(_) => {
                self.consecutive_failures += 1;
                if self.consecutive_failures >= Self::FAILURE_THRESHOLD {
                    self.circuit_state = CircuitState::Open {
                        until: Instant::now() + Self::OPEN_DURATION,
                    };
                    error!("Circuit opened after {} failures", self.consecutive_failures);
                }
            }
        }

        result
    }
}
```

### Error Classification

```rust
fn is_retryable_error(status: u16, body: &str) -> bool {
    match status {
        // Rate limited - retry with backoff
        429 => true,
        // Server errors - retry
        500..=599 => true,
        // Auth errors - don't retry
        401 | 403 => false,
        // Bad request - don't retry
        400 => false,
        // Success
        200..=299 => false,
        _ => true, // Unknown - try again
    }
}
```

### Alert Escalation

When circuit opens, escalate via alternative channel:
1. Log to error file
2. Send email (if configured)
3. Call secondary alert endpoint

## Configuration

```toml
[alert]
max_retries = 5
initial_backoff_ms = 1000
max_backoff_ms = 60000
circuit_failure_threshold = 3
circuit_open_duration_secs = 300
```

## Sources
- [tokio-retry2](https://docs.rs/tokio-retry2/latest/tokio_retry2/)
- [failsafe-rs](https://github.com/dmexe/failsafe-rs)
- [backoff crate](https://docs.rs/backoff)
- [backon](https://rustmagazine.org/issue-2/how-i-designed-the-api-for-backon-a-user-friendly-retry-crate/)
- [tower-circuitbreaker](https://lib.rs/crates/tower-circuitbreaker)
