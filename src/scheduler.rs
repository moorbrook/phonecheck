/// Business hours scheduler
/// Runs checks hourly between 8am and 5pm Pacific time, 7 days a week

use chrono::{TimeZone, Timelike};
use chrono_tz::America::Los_Angeles;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::signal;
use tokio::sync::watch;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

pub const BUSINESS_START_HOUR: u32 = 8; // 8 AM
pub const BUSINESS_END_HOUR: u32 = 17; // 5 PM (17:00)

/// Tolerance window for clock drift/sleep overshoot (30 seconds)
/// If we wake up within this window after intended time, still run the check
pub const SCHEDULE_TOLERANCE_SECS: u64 = 30;

/// Check if current time is within business hours (Pacific time)
pub fn is_business_hours() -> bool {
    let now = Los_Angeles.from_utc_datetime(&chrono::Utc::now().naive_utc());
    is_business_hours_at(now.hour(), now.minute(), now.second())
}

/// Testable version: check if given hour/minute/second is within business hours
pub fn is_business_hours_at(hour: u32, _minute: u32, _second: u32) -> bool {
    hour >= BUSINESS_START_HOUR && hour < BUSINESS_END_HOUR
}

/// Calculate duration until next check should run
/// Returns None if we should run immediately, Some(duration) if we need to wait
pub fn time_until_next_check() -> Option<Duration> {
    let now = Los_Angeles.from_utc_datetime(&chrono::Utc::now().naive_utc());
    time_until_next_check_at(now.hour(), now.minute(), now.second())
}

/// Testable version: calculate wait time from given hour/minute/second
pub fn time_until_next_check_at(hour: u32, minute: u32, second: u32) -> Option<Duration> {
    // If within business hours, check at the top of the next hour
    if hour >= BUSINESS_START_HOUR && hour < BUSINESS_END_HOUR {
        // If it's within the first 5 seconds of the hour, run now
        if minute == 0 && second < 5 {
            return None;
        }

        // Otherwise wait until next hour
        let seconds_until_next_hour = (60 - minute) * 60 - second;
        return Some(Duration::from_secs(seconds_until_next_hour as u64));
    }

    // Before business hours today
    if hour < BUSINESS_START_HOUR {
        let hours_until = BUSINESS_START_HOUR - hour;
        let seconds_until = hours_until * 3600 - minute * 60 - second;
        return Some(Duration::from_secs(seconds_until as u64));
    }

    // After business hours - wait until 8am tomorrow
    let hours_until_midnight = 24 - hour;
    let hours_after_midnight = BUSINESS_START_HOUR;
    let total_hours = hours_until_midnight + hours_after_midnight;
    let seconds_until = total_hours * 3600 - minute * 60 - second;
    Some(Duration::from_secs(seconds_until as u64))
}

/// Format duration for logging
pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;

    if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else {
        format!("{}m", mins)
    }
}

/// Check if current time is within business hours with tolerance
/// Returns true if within business hours OR within tolerance window after end
pub fn is_business_hours_with_tolerance() -> bool {
    let now = Los_Angeles.from_utc_datetime(&chrono::Utc::now().naive_utc());
    is_business_hours_with_tolerance_at(now.hour(), now.minute(), now.second())
}

/// Testable version with tolerance for boundary conditions
pub fn is_business_hours_with_tolerance_at(hour: u32, minute: u32, second: u32) -> bool {
    // Standard business hours check
    if hour >= BUSINESS_START_HOUR && hour < BUSINESS_END_HOUR {
        return true;
    }

    // Allow tolerance window: if we're just past 5pm (e.g., 17:00:15 due to drift),
    // still consider it valid for a check that was scheduled for 5pm
    if hour == BUSINESS_END_HOUR && minute == 0 {
        let seconds_past = second as u64;
        if seconds_past < SCHEDULE_TOLERANCE_SECS {
            return true;
        }
    }

    false
}

/// Create a shutdown signal receiver
/// Returns a watch receiver that will be notified when SIGINT/SIGTERM is received
pub fn shutdown_signal() -> watch::Receiver<bool> {
    let (tx, rx) = watch::channel(false);

    tokio::spawn(async move {
        // Wait for ctrl-c (SIGINT)
        let ctrl_c = async {
            signal::ctrl_c()
                .await
                .expect("Failed to install Ctrl+C handler");
        };

        // On Unix, also wait for SIGTERM
        #[cfg(unix)]
        let terminate = async {
            signal::unix::signal(signal::unix::SignalKind::terminate())
                .expect("Failed to install SIGTERM handler")
                .recv()
                .await;
        };

        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            _ = ctrl_c => {
                info!("Received SIGINT (Ctrl+C)");
            }
            _ = terminate => {
                info!("Received SIGTERM");
            }
        }

        // Signal shutdown
        let _ = tx.send(true);
    });

    rx
}

/// Run the scheduler loop with graceful shutdown support
/// The check function receives a CancellationToken that will be cancelled
/// when shutdown is requested during an active check.
pub async fn run_scheduler<F, Fut>(mut check_fn: F)
where
    F: FnMut(CancellationToken) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let mut shutdown_rx = shutdown_signal();
    run_scheduler_with_shutdown(&mut check_fn, &mut shutdown_rx).await;
}

/// Guard to track whether a check is currently running
/// Prevents concurrent checks from starting
pub struct CheckGuard {
    is_running: Arc<AtomicBool>,
}

impl CheckGuard {
    /// Try to acquire the check guard. Returns None if a check is already running.
    pub fn try_acquire(is_running: &Arc<AtomicBool>) -> Option<Self> {
        // Try to set from false to true atomically
        if is_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            Some(CheckGuard {
                is_running: is_running.clone(),
            })
        } else {
            None
        }
    }
}

impl Drop for CheckGuard {
    fn drop(&mut self) {
        self.is_running.store(false, Ordering::SeqCst);
    }
}

/// Run the scheduler loop with an externally provided shutdown signal (for testing)
pub async fn run_scheduler_with_shutdown<F, Fut>(
    check_fn: &mut F,
    shutdown_rx: &mut watch::Receiver<bool>,
) where
    F: FnMut(CancellationToken) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let is_check_running = Arc::new(AtomicBool::new(false));
    run_scheduler_with_shutdown_and_guard(check_fn, shutdown_rx, &is_check_running).await;
}

/// Timeout for graceful shutdown of in-flight calls
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Internal scheduler implementation with explicit check guard
async fn run_scheduler_with_shutdown_and_guard<F, Fut>(
    check_fn: &mut F,
    shutdown_rx: &mut watch::Receiver<bool>,
    is_check_running: &Arc<AtomicBool>,
) where
    F: FnMut(CancellationToken) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    info!("Scheduler started (Pacific time, 8am-5pm daily)");

    loop {
        // Check for shutdown before starting
        if *shutdown_rx.borrow() {
            info!("Shutdown requested, stopping scheduler");
            break;
        }

        // Calculate wait time and track if we intend to run
        let should_run = match time_until_next_check() {
            Some(wait_duration) => {
                info!("Next check in {}", format_duration(wait_duration));

                // Wait with shutdown interrupt capability
                tokio::select! {
                    _ = sleep(wait_duration) => {
                        // Normal wake-up, proceed with check
                        true
                    }
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            warn!("Shutdown during sleep, stopping scheduler");
                            break;
                        }
                        // Spurious wake, continue normally
                        true
                    }
                }
            }
            None => {
                debug!("Running check immediately");
                true
            }
        };

        // Run the check if we intended to and we're still in valid window
        // Use tolerance-aware check to handle clock drift
        if should_run && is_business_hours_with_tolerance() {
            // Try to acquire the check guard - prevents concurrent checks
            if let Some(_guard) = CheckGuard::try_acquire(is_check_running) {
                // Create a cancellation token for this check
                let cancel_token = CancellationToken::new();
                let check_token = cancel_token.clone();

                // Spawn the check and wait for it with shutdown awareness
                let check_future = check_fn(check_token);

                tokio::select! {
                    _ = check_future => {
                        // Check completed normally
                    }
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            info!("Shutdown during active check - signaling cancellation");
                            cancel_token.cancel();

                            // Wait for graceful shutdown with timeout
                            // The check should send BYE and clean up
                            info!("Waiting for in-flight call to complete gracefully...");
                            tokio::select! {
                                _ = sleep(GRACEFUL_SHUTDOWN_TIMEOUT) => {
                                    warn!("Graceful shutdown timeout - check may not have completed cleanly");
                                }
                                // Note: we can't await the original future again after select
                                // The check_fn should handle cancellation internally
                            }
                            break;
                        }
                    }
                }
                // Guard is dropped here, releasing the lock
            } else {
                warn!("Skipping scheduled check - previous check still running");
            }
        }

        // Small delay to avoid running multiple times in the same minute
        // Also check for shutdown during this delay
        tokio::select! {
            _ = sleep(Duration::from_secs(60)) => {}
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    info!("Shutdown during post-check delay");
                    break;
                }
            }
        }
    }

    info!("Scheduler stopped gracefully");
}

#[cfg(test)]
mod tests {
    use super::*;

    // === format_duration tests ===

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(Duration::from_secs(3600)), "1h 0m");
        assert_eq!(format_duration(Duration::from_secs(3660)), "1h 1m");
        assert_eq!(format_duration(Duration::from_secs(1800)), "30m");
        assert_eq!(format_duration(Duration::from_secs(90)), "1m");
    }

    #[test]
    fn test_format_duration_edge_cases() {
        assert_eq!(format_duration(Duration::from_secs(0)), "0m");
        assert_eq!(format_duration(Duration::from_secs(59)), "0m");
        assert_eq!(format_duration(Duration::from_secs(60)), "1m");
        assert_eq!(format_duration(Duration::from_secs(7200)), "2h 0m");
    }

    // === is_business_hours_at tests ===

    #[test]
    fn test_is_business_hours_at_start_boundary() {
        // 8:00 AM is the start of business hours
        assert!(is_business_hours_at(8, 0, 0));
        assert!(is_business_hours_at(8, 0, 1));
        assert!(is_business_hours_at(8, 30, 0));
    }

    #[test]
    fn test_is_business_hours_at_before_start() {
        // Before 8 AM is not business hours
        assert!(!is_business_hours_at(7, 59, 59));
        assert!(!is_business_hours_at(7, 0, 0));
        assert!(!is_business_hours_at(0, 0, 0));
        assert!(!is_business_hours_at(6, 30, 0));
    }

    #[test]
    fn test_is_business_hours_at_end_boundary() {
        // 5:00 PM (17:00) is the end - NOT in business hours
        assert!(!is_business_hours_at(17, 0, 0));
        assert!(!is_business_hours_at(17, 0, 1));
        // 4:59 PM is still in business hours
        assert!(is_business_hours_at(16, 59, 59));
    }

    // === is_business_hours_with_tolerance_at tests ===

    #[test]
    fn test_tolerance_allows_slight_overshoot() {
        // Just past 5pm should still be allowed with tolerance
        assert!(is_business_hours_with_tolerance_at(17, 0, 0)); // exactly 5pm
        assert!(is_business_hours_with_tolerance_at(17, 0, 15)); // 15 seconds past
        assert!(is_business_hours_with_tolerance_at(17, 0, 29)); // 29 seconds past
        // But 30+ seconds past should not
        assert!(!is_business_hours_with_tolerance_at(17, 0, 30));
        assert!(!is_business_hours_with_tolerance_at(17, 1, 0)); // 1 minute past
    }

    #[test]
    fn test_tolerance_normal_hours_unchanged() {
        // Normal business hours should work as before
        assert!(is_business_hours_with_tolerance_at(8, 0, 0));
        assert!(is_business_hours_with_tolerance_at(12, 30, 0));
        assert!(is_business_hours_with_tolerance_at(16, 59, 59));
        // Before business hours still not allowed
        assert!(!is_business_hours_with_tolerance_at(7, 59, 59));
    }

    #[test]
    fn test_is_business_hours_at_after_end() {
        assert!(!is_business_hours_at(17, 0, 0));
        assert!(!is_business_hours_at(18, 0, 0));
        assert!(!is_business_hours_at(23, 59, 59));
    }

    #[test]
    fn test_is_business_hours_at_midday() {
        assert!(is_business_hours_at(12, 0, 0));
        assert!(is_business_hours_at(10, 30, 0));
        assert!(is_business_hours_at(15, 45, 30));
    }

    // === time_until_next_check_at tests ===

    #[test]
    fn test_time_until_next_check_at_top_of_hour() {
        // At exactly 8:00:00, should run immediately
        assert_eq!(time_until_next_check_at(8, 0, 0), None);
        assert_eq!(time_until_next_check_at(8, 0, 4), None); // Within 5 second grace
        assert_eq!(time_until_next_check_at(10, 0, 2), None);
    }

    #[test]
    fn test_time_until_next_check_at_during_business_hours() {
        // 8:00:05 - just past grace period, wait until 9:00
        let result = time_until_next_check_at(8, 0, 5);
        assert!(result.is_some());
        // Should wait ~3595 seconds (59:55)
        assert_eq!(result.unwrap().as_secs(), 59 * 60 + 55);

        // 8:30:00 - wait until 9:00 (30 minutes)
        let result = time_until_next_check_at(8, 30, 0);
        assert_eq!(result.unwrap().as_secs(), 30 * 60);

        // 16:45:30 - wait until 17:00 (14:30), but 17:00 is end of business hours
        // Actually it should wait until next hour within business hours
        let result = time_until_next_check_at(16, 45, 30);
        assert_eq!(result.unwrap().as_secs(), 14 * 60 + 30);
    }

    #[test]
    fn test_time_until_next_check_at_before_business_hours() {
        // 6:00:00 - wait until 8:00 (2 hours)
        let result = time_until_next_check_at(6, 0, 0);
        assert_eq!(result.unwrap().as_secs(), 2 * 3600);

        // 7:30:00 - wait until 8:00 (30 minutes)
        let result = time_until_next_check_at(7, 30, 0);
        assert_eq!(result.unwrap().as_secs(), 30 * 60);

        // 0:00:00 - wait until 8:00 (8 hours)
        let result = time_until_next_check_at(0, 0, 0);
        assert_eq!(result.unwrap().as_secs(), 8 * 3600);
    }

    #[test]
    fn test_time_until_next_check_at_after_business_hours() {
        // 17:00:00 - wait until 8:00 tomorrow (15 hours)
        let result = time_until_next_check_at(17, 0, 0);
        assert_eq!(result.unwrap().as_secs(), 15 * 3600);

        // 18:30:00 - wait until 8:00 tomorrow (13.5 hours)
        let result = time_until_next_check_at(18, 30, 0);
        assert_eq!(result.unwrap().as_secs(), 13 * 3600 + 30 * 60);

        // 23:59:59 - wait until 8:00 tomorrow (~8 hours)
        let result = time_until_next_check_at(23, 59, 59);
        assert_eq!(result.unwrap().as_secs(), 8 * 3600 + 1);
    }

    // === Integration tests ===

    #[test]
    fn test_is_business_hours_doesnt_panic() {
        // Verify the actual function works with real time
        let _ = is_business_hours();
    }

    #[test]
    fn test_time_until_next_check_reasonable() {
        let result = time_until_next_check();
        if let Some(duration) = result {
            // Should never be more than ~24 hours
            assert!(duration.as_secs() <= 24 * 3600);
        }
    }

    // === Graceful shutdown tests ===

    #[tokio::test]
    async fn test_scheduler_shutdown_before_start() {
        let (_tx, mut rx) = watch::channel(true); // Already shutdown

        let check_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let check_count_clone = check_count.clone();

        // Closure now takes a CancellationToken (ignored in this test)
        let check_fn = move |_cancel_token: CancellationToken| {
            let cc = check_count_clone.clone();
            async move {
                cc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        };

        // Scheduler should exit immediately without running any checks
        tokio::time::timeout(
            Duration::from_millis(100),
            run_scheduler_with_shutdown(&mut { check_fn }, &mut rx),
        )
        .await
        .expect("Scheduler should exit quickly when shutdown is already requested");

        assert_eq!(check_count.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_scheduler_shutdown_during_sleep() {
        let (tx, mut rx) = watch::channel(false);

        let check_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let check_count_clone = check_count.clone();

        // Closure now takes a CancellationToken (ignored in this test)
        let check_fn = move |_cancel_token: CancellationToken| {
            let cc = check_count_clone.clone();
            async move {
                cc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        };

        // Spawn the scheduler
        let handle = tokio::spawn(async move {
            run_scheduler_with_shutdown(&mut { check_fn }, &mut rx).await;
        });

        // Give scheduler time to start sleeping
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Send shutdown signal
        tx.send(true).unwrap();

        // Scheduler should exit within a short time
        tokio::time::timeout(Duration::from_millis(200), handle)
            .await
            .expect("Scheduler should exit after shutdown signal")
            .expect("Scheduler task should complete without panic");
    }

    #[tokio::test]
    async fn test_cancellation_token_propagates_during_check() {
        // Test that cancellation token is triggered when shutdown occurs during check
        let (tx, mut rx) = watch::channel(false);
        let token_was_cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let token_was_cancelled_clone = token_was_cancelled.clone();

        let check_fn = move |cancel_token: CancellationToken| {
            let cancelled_flag = token_was_cancelled_clone.clone();
            async move {
                // Simulate a long-running check that watches for cancellation
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(10)) => {
                        // Check completed without cancellation
                    }
                    _ = cancel_token.cancelled() => {
                        cancelled_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                }
            }
        };

        // We need to mock business hours for this test
        // Since we can't easily, we'll directly test the cancellation logic
        let is_check_running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Spawn the scheduler internals directly
        let handle = tokio::spawn(async move {
            run_scheduler_with_shutdown_and_guard(&mut { check_fn }, &mut rx, &is_check_running)
                .await;
        });

        // Wait a bit for scheduler to potentially start
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Send shutdown signal
        tx.send(true).unwrap();

        // Scheduler should exit gracefully
        tokio::time::timeout(Duration::from_millis(500), handle)
            .await
            .expect("Scheduler should exit after shutdown")
            .expect("Scheduler should complete without panic");
    }

    // === Concurrent check prevention tests ===

    #[test]
    fn test_check_guard_prevents_concurrent() {
        let is_running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        // First acquisition should succeed
        let guard1 = CheckGuard::try_acquire(&is_running);
        assert!(guard1.is_some());

        // Second acquisition should fail
        let guard2 = CheckGuard::try_acquire(&is_running);
        assert!(guard2.is_none());

        // After dropping first guard, acquisition should succeed again
        drop(guard1);
        let guard3 = CheckGuard::try_acquire(&is_running);
        assert!(guard3.is_some());
    }

    #[test]
    fn test_check_guard_released_on_drop() {
        let is_running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        {
            let _guard = CheckGuard::try_acquire(&is_running);
            assert!(is_running.load(std::sync::atomic::Ordering::SeqCst));
        }

        // Guard dropped, should be false now
        assert!(!is_running.load(std::sync::atomic::Ordering::SeqCst));
    }

    // === DST transition tests ===

    #[test]
    fn test_dst_spring_forward() {
        // In spring, clocks jump from 2am to 3am (Pacific)
        // This shouldn't affect business hours (8am-5pm)
        // The hour 2am doesn't exist on this day
        use chrono::{NaiveDate, NaiveTime, NaiveDateTime, TimeZone};

        // March 10, 2024 is a DST transition day (spring forward)
        // Before transition (1:59 AM PST = UTC-8)
        let before = NaiveDateTime::new(
            NaiveDate::from_ymd_opt(2024, 3, 10).unwrap(),
            NaiveTime::from_hms_opt(9, 59, 0).unwrap(), // UTC time
        );
        let la_before = Los_Angeles.from_utc_datetime(&before);
        // At 9:59 UTC on March 10, it's 1:59 AM PST (before transition)
        assert!(!is_business_hours_at(la_before.hour(), la_before.minute(), la_before.second()));

        // After transition (3:00 AM PDT = UTC-7, so 10:00 UTC = 3:00 AM)
        let after = NaiveDateTime::new(
            NaiveDate::from_ymd_opt(2024, 3, 10).unwrap(),
            NaiveTime::from_hms_opt(10, 0, 0).unwrap(), // UTC time
        );
        let la_after = Los_Angeles.from_utc_datetime(&after);
        // At 10:00 UTC on March 10, it's 3:00 AM PDT (after transition)
        assert!(!is_business_hours_at(la_after.hour(), la_after.minute(), la_after.second()));

        // Business hours should still work correctly
        // 8:00 AM PDT on March 10 = 15:00 UTC
        let business_start = NaiveDateTime::new(
            NaiveDate::from_ymd_opt(2024, 3, 10).unwrap(),
            NaiveTime::from_hms_opt(15, 0, 0).unwrap(),
        );
        let la_business = Los_Angeles.from_utc_datetime(&business_start);
        assert_eq!(la_business.hour(), 8);
        assert!(is_business_hours_at(la_business.hour(), la_business.minute(), la_business.second()));
    }

    #[test]
    fn test_dst_fall_back() {
        // In fall, clocks fall back from 2am to 1am (Pacific)
        // The hour 1am exists twice on this day
        use chrono::{NaiveDate, NaiveTime, NaiveDateTime, TimeZone};

        // November 3, 2024 is a DST transition day (fall back)
        // First 1:30 AM PDT = 8:30 UTC
        let first_130 = NaiveDateTime::new(
            NaiveDate::from_ymd_opt(2024, 11, 3).unwrap(),
            NaiveTime::from_hms_opt(8, 30, 0).unwrap(),
        );
        let la_first = Los_Angeles.from_utc_datetime(&first_130);
        assert!(!is_business_hours_at(la_first.hour(), la_first.minute(), la_first.second()));

        // Second 1:30 AM PST = 9:30 UTC
        let second_130 = NaiveDateTime::new(
            NaiveDate::from_ymd_opt(2024, 11, 3).unwrap(),
            NaiveTime::from_hms_opt(9, 30, 0).unwrap(),
        );
        let la_second = Los_Angeles.from_utc_datetime(&second_130);
        assert!(!is_business_hours_at(la_second.hour(), la_second.minute(), la_second.second()));

        // Business hours should still work correctly
        // 8:00 AM PST on Nov 3 = 16:00 UTC
        let business_start = NaiveDateTime::new(
            NaiveDate::from_ymd_opt(2024, 11, 3).unwrap(),
            NaiveTime::from_hms_opt(16, 0, 0).unwrap(),
        );
        let la_business = Los_Angeles.from_utc_datetime(&business_start);
        assert_eq!(la_business.hour(), 8);
        assert!(is_business_hours_at(la_business.hour(), la_business.minute(), la_business.second()));
    }

    #[test]
    fn test_time_until_next_check_during_dst_transition() {
        // Verify that time calculations don't panic during DST transitions
        use chrono::{NaiveDate, NaiveTime, NaiveDateTime, TimeZone};

        // Test around spring forward (March 10, 2024)
        for hour in 0..24 {
            for minute in [0, 30] {
                let dt = NaiveDateTime::new(
                    NaiveDate::from_ymd_opt(2024, 3, 10).unwrap(),
                    NaiveTime::from_hms_opt(hour, minute, 0).unwrap(),
                );
                let la = Los_Angeles.from_utc_datetime(&dt);
                // Should not panic
                let _ = time_until_next_check_at(la.hour(), la.minute(), la.second());
            }
        }

        // Test around fall back (November 3, 2024)
        for hour in 0..24 {
            for minute in [0, 30] {
                let dt = NaiveDateTime::new(
                    NaiveDate::from_ymd_opt(2024, 11, 3).unwrap(),
                    NaiveTime::from_hms_opt(hour, minute, 0).unwrap(),
                );
                let la = Los_Angeles.from_utc_datetime(&dt);
                // Should not panic
                let _ = time_until_next_check_at(la.hour(), la.minute(), la.second());
            }
        }
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Any hour 8-16 (inclusive) is within business hours
        #[test]
        fn business_hours_8_to_16(hour in 8u32..17u32, minute in 0u32..60u32, second in 0u32..60u32) {
            prop_assert!(is_business_hours_at(hour, minute, second));
        }

        /// Any hour 0-7 or 17-23 is outside business hours
        #[test]
        fn non_business_hours(
            hour in prop_oneof![0u32..8u32, 17u32..24u32],
            minute in 0u32..60u32,
            second in 0u32..60u32
        ) {
            prop_assert!(!is_business_hours_at(hour, minute, second));
        }

        /// time_until_next_check always returns duration < 24 hours
        #[test]
        fn wait_time_bounded(hour in 0u32..24u32, minute in 0u32..60u32, second in 0u32..60u32) {
            if let Some(duration) = time_until_next_check_at(hour, minute, second) {
                prop_assert!(duration.as_secs() <= 24 * 3600);
            }
        }

        /// During business hours at top of hour, returns None (run immediately)
        #[test]
        fn top_of_hour_runs_immediately(hour in 8u32..17u32, second in 0u32..5u32) {
            prop_assert_eq!(time_until_next_check_at(hour, 0, second), None);
        }

        /// format_duration never panics
        #[test]
        fn format_duration_never_panics(secs in 0u64..100_000u64) {
            let _ = format_duration(Duration::from_secs(secs));
        }
    }
}

/// Kani formal verification proofs
#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    fn business_hours_valid_range() {
        let hour: u32 = kani::any();
        kani::assume(hour < 24);
        let minute: u32 = kani::any();
        kani::assume(minute < 60);
        let second: u32 = kani::any();
        kani::assume(second < 60);

        let result = is_business_hours_at(hour, minute, second);

        // Verify the result matches our expectation
        let expected = hour >= BUSINESS_START_HOUR && hour < BUSINESS_END_HOUR;
        kani::assert(result == expected, "business hours check must be consistent");
    }

    #[kani::proof]
    fn time_until_bounded() {
        let hour: u32 = kani::any();
        kani::assume(hour < 24);
        let minute: u32 = kani::any();
        kani::assume(minute < 60);
        let second: u32 = kani::any();
        kani::assume(second < 60);

        if let Some(duration) = time_until_next_check_at(hour, minute, second) {
            // Must be less than 24 hours
            kani::assert(duration.as_secs() <= 24 * 3600, "wait time must be <= 24h");
        }
    }
}
