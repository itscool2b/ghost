//! Wall-clock time as stored in the database and in JSON files.

use std::time::{SystemTime, UNIX_EPOCH};

/// Current Unix time in milliseconds.
///
/// A clock set before 1970 yields 0 instead of panicking: these timestamps order records and label
/// backups, and a wrong-but-valid value is far better than refusing to start.
pub fn now_millis() -> i64 {
    millis_since_epoch(SystemTime::now())
}

fn millis_since_epoch(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(elapsed) => i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn now_is_a_plausible_unix_millis_value() {
        let now = now_millis();
        // 2024-01-01 and 2200-01-01.
        assert!(now > 1_704_067_200_000, "{now}");
        assert!(now < 7_258_118_400_000, "{now}");
    }

    #[test]
    fn a_clock_before_the_epoch_yields_zero() {
        if let Some(before) = UNIX_EPOCH.checked_sub(Duration::from_secs(86_400)) {
            assert_eq!(millis_since_epoch(before), 0);
        }
        assert_eq!(millis_since_epoch(UNIX_EPOCH), 0);
        assert_eq!(
            millis_since_epoch(UNIX_EPOCH + Duration::from_millis(1500)),
            1500
        );
    }

    #[test]
    fn times_beyond_i64_millis_saturate() {
        // Representable on unix (i64 seconds), not on Windows (FILETIME); skip where it isn't.
        if let Some(far) = UNIX_EPOCH.checked_add(Duration::from_secs(u64::MAX / 4)) {
            assert_eq!(millis_since_epoch(far), i64::MAX);
        }
    }
}
