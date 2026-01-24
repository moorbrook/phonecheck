# Graceful Shutdown Research

## Recommended Approach

Use `tokio::signal::ctrl_c` with `tokio::sync::watch` or `CancellationToken` from tokio-util.

### Existing Crates (if more complex needs)

1. **[tokio-graceful-shutdown](https://crates.io/crates/tokio-graceful-shutdown)** - Full subsystem management
   - Handles SIGINT/SIGTERM automatically
   - Clean abstractions over shutdown boilerplate
   - Supports nested subsystems

2. **[tokio-graceful](https://github.com/plabayo/tokio-graceful)** - Lightweight guard-based
   - Lock-free guard creation
   - `Shutdown::default()` for system signals

3. **[tokio-shutdown](https://docs.rs/tokio-shutdown)** - Minimal approach
   - Wait for stop signal across threads

## Implementation for PhoneCheck

Since PhoneCheck has simple needs (scheduler + occasional SIP call), use the basic Tokio approach:

### Simple Pattern with CancellationToken

```rust
use tokio::signal;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<()> {
    let shutdown = CancellationToken::new();
    let shutdown_clone = shutdown.clone();

    // Spawn signal handler
    tokio::spawn(async move {
        let _ = signal::ctrl_c().await;
        info!("Shutdown signal received");
        shutdown_clone.cancel();
    });

    // Main loop with shutdown check
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                info!("Shutting down gracefully...");
                break;
            }
            _ = run_scheduled_check() => {}
        }
    }

    // Cleanup: wait for in-flight calls to complete
    cleanup().await;
    Ok(())
}
```

### Alternative: watch channel

```rust
use tokio::sync::watch;

let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

// Signal handler
tokio::spawn(async move {
    let _ = signal::ctrl_c().await;
    let _ = shutdown_tx.send(true);
});

// In task
loop {
    tokio::select! {
        _ = shutdown_rx.changed() => {
            if *shutdown_rx.borrow() { break; }
        }
        result = do_work() => { /* handle result */ }
    }
}
```

## Key Points for PhoneCheck

1. **Signal handling**: Listen for SIGINT (Ctrl+C) and SIGTERM
2. **In-flight calls**: Let active SIP calls complete (send BYE) before exit
3. **Scheduler state**: No persistent state needed, safe to stop mid-wait
4. **Cleanup checklist**:
   - Send BYE if call in progress
   - Close UDP sockets
   - Flush any buffered logs

## Sources
- [Tokio Graceful Shutdown Guide](https://tokio.rs/tokio/topics/shutdown)
- [tokio-graceful-shutdown crate](https://crates.io/crates/tokio-graceful-shutdown)
- [tokio-graceful crate](https://github.com/plabayo/tokio-graceful)
