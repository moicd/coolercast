//! CPU package temperature in °C: PawnIO on Windows, hwmon on Linux.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::CpuTemp;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::CpuTemp;
