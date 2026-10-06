//! Average CPU frequency in MHz: PDH performance counters on Windows, cpufreq on Linux.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::CpuFreq;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::CpuFreq;
