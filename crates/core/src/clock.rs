//! Local time of day, for the night schedule. Windows asks the system; Linux has no local time in
//! `std`, so the UTC offset comes from `date +%z`, checked once an hour (DST changes are picked up
//! within the hour).

use crate::config::ClockTime;

#[cfg(target_os = "linux")]
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(target_os = "linux")]
const OFFSET_TTL: Duration = Duration::from_secs(3600);

/// Reads the local time of day.
#[derive(Debug, Default)]
pub struct LocalClock {
    #[cfg(target_os = "linux")]
    offset: Option<(i64, Instant)>,
}

impl LocalClock {
    #[cfg(windows)]
    pub fn now(&mut self) -> Option<ClockTime> {
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;

        let mut time = unsafe { std::mem::zeroed() };
        unsafe { GetLocalTime(&mut time) };
        Some(ClockTime::new(time.wHour, time.wMinute))
    }

    /// `None` if the UTC offset is unknown (`date` missing).
    #[cfg(target_os = "linux")]
    pub fn now(&mut self) -> Option<ClockTime> {
        let stale = self.offset.is_none_or(|(_, at)| at.elapsed() >= OFFSET_TTL);
        if stale {
            let output = std::process::Command::new("date")
                .arg("+%z")
                .output()
                .ok()?;
            let offset = parse_utc_offset(&String::from_utf8_lossy(&output.stdout))?;
            self.offset = Some((offset, Instant::now()));
        }
        let (offset, _) = self.offset?;
        let utc = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        Some(time_of_day(utc as i64 + offset))
    }
}

/// Seconds east of UTC from `date +%z` output such as `+0200` or `-0530`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_utc_offset(text: &str) -> Option<i64> {
    let text = text.trim();
    let (sign, digits) = match text.as_bytes().first()? {
        b'+' => (1, &text[1..]),
        b'-' => (-1, &text[1..]),
        _ => return None,
    };
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hours: i64 = digits[..2].parse().ok()?;
    let minutes: i64 = digits[2..].parse().ok()?;
    Some(sign * (hours * 3600 + minutes * 60))
}

/// The time of day of a local Unix timestamp.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn time_of_day(local_seconds: i64) -> ClockTime {
    let minutes = local_seconds.rem_euclid(24 * 3600) / 60;
    ClockTime::from_minutes(minutes as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_offsets() {
        assert_eq!(parse_utc_offset("+0200\n"), Some(7200));
        assert_eq!(parse_utc_offset("-0530"), Some(-19800));
        assert_eq!(parse_utc_offset("+0000"), Some(0));
        assert_eq!(parse_utc_offset("CEST"), None);
        assert_eq!(parse_utc_offset("+02"), None);
    }

    #[test]
    fn times_of_day() {
        // 2026-10-06 21:30:00 UTC, two hours east.
        let utc = 1_791_322_200;
        assert_eq!(time_of_day(utc + 7200), ClockTime::new(23, 30));
        assert_eq!(time_of_day(-60), ClockTime::new(23, 59));
    }

    #[test]
    fn local_time_is_available() {
        assert!(LocalClock::default().now().is_some());
    }
}
