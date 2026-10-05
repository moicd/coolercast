//! A tiny logger: timestamped lines to a size-capped file and/or stderr.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// The log file is rotated to `*.old.log` once it grows past this size.
const MAX_FILE_SIZE: u64 = 512 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

struct Logger {
    path: Option<PathBuf>,
    file: Option<File>,
    size: u64,
    stderr: bool,
}

static LOGGER: Mutex<Option<Logger>> = Mutex::new(None);

/// Starts logging to `file` (if any) and to stderr (if `stderr`).
pub fn init(file: Option<PathBuf>, stderr: bool) {
    let mut logger = Logger {
        path: file,
        file: None,
        size: 0,
        stderr,
    };
    logger.open();
    *LOGGER.lock().unwrap_or_else(|e| e.into_inner()) = Some(logger);
}

pub fn write(level: Level, args: fmt::Arguments<'_>) {
    let mut guard = LOGGER.lock().unwrap_or_else(|e| e.into_inner());
    let Some(logger) = guard.as_mut() else { return };

    let tag = match level {
        Level::Info => "INFO ",
        Level::Warn => "WARN ",
        Level::Error => "ERROR",
    };
    let line = format!("{} {tag} {args}\n", timestamp());

    if logger.stderr {
        eprint!("{line}");
    }
    if logger.size + line.len() as u64 > MAX_FILE_SIZE {
        logger.rotate();
    }
    if let Some(file) = logger.file.as_mut()
        && file.write_all(line.as_bytes()).is_ok()
    {
        logger.size += line.len() as u64;
    }
}

/// Local time on Windows. On Linux the service logs to the journal, which adds its own
/// timestamps, so the clock is only informative there and shown in UTC.
#[cfg(windows)]
fn timestamp() -> String {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;

    let mut t: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
    )
}

#[cfg(not(windows))]
fn timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let (year, month, day) = civil_from_days((secs / 86_400) as i64);
    let s = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{:03}Z",
        s / 3600,
        s / 60 % 60,
        s % 60,
        now.subsec_millis()
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's algorithm).
#[cfg_attr(windows, allow(dead_code))]
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

impl Logger {
    fn open(&mut self) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        self.file = OpenOptions::new().create(true).append(true).open(path).ok();
        self.size = self
            .file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .map_or(0, |m| m.len());
    }

    fn rotate(&mut self) {
        let Some(path) = &self.path else { return };
        self.file = None;
        let _ = fs::rename(path, path.with_extension("old.log"));
        self.open();
    }
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => { $crate::log::write($crate::log::Level::Info, format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => { $crate::log::write($crate::log::Level::Warn, format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => { $crate::log::write($crate::log::Level::Error, format_args!($($arg)*)) };
}

#[cfg(test)]
mod tests {
    use super::civil_from_days;

    #[test]
    fn dates_from_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(20_731), (2026, 10, 5));
    }
}
