/// Business hours scheduler
/// Runs checks hourly between 8am and 5pm Pacific time, 7 days a week

use chrono::{TimeZone, Timelike};
use chrono_tz::America::Los_Angeles;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, info};

pub const BUSINESS_START_HOUR: u32 = 8; // 8 AM
pub const BUSINESS_END_HOUR: u32 = 17; // 5 PM (17:00)

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

/// Run the scheduler loop
pub async fn run_scheduler<F, Fut>(mut check_fn: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    info!("Scheduler started (Pacific time, 8am-5pm daily)");

    loop {
        // Calculate wait time
        match time_until_next_check() {
            Some(wait_duration) => {
                info!("Next check in {}", format_duration(wait_duration));
                sleep(wait_duration).await;
            }
            None => {
                debug!("Running check immediately");
            }
        }

        // Run the check if we're in business hours
        if is_business_hours() {
            check_fn().await;
        }

        // Small delay to avoid running multiple times in the same minute
        sleep(Duration::from_secs(60)).await;
    }
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
