//! Time helpers. Timestamps are stored as integers.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Current time in unix milliseconds.
pub fn now_ms() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_millis(),
    )
    .unwrap_or(i64::MAX)
}

/// A file time in unix nanoseconds; negative for times before 1970.
pub fn system_time_to_ns(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
        Err(e) => i64::try_from(e.duration().as_nanos())
            .map(|n| -n)
            .unwrap_or(i64::MIN),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_epoch_is_negative() {
        assert_eq!(
            system_time_to_ns(UNIX_EPOCH - Duration::from_secs(1)),
            -1_000_000_000
        );
        assert_eq!(system_time_to_ns(UNIX_EPOCH + Duration::from_nanos(5)), 5);
        assert!(now_ms() > 0);
    }
}
