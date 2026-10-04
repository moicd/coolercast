//! A tiny logger: timestamped lines to a size-capped file and/or stderr.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

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

    let mut t: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    let tag = match level {
        Level::Info => "INFO ",
        Level::Warn => "WARN ",
        Level::Error => "ERROR",
    };
    let line = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} {tag} {args}\n",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
    );

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
