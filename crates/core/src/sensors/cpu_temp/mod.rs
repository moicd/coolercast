//! CPU package temperature in °C: PawnIO on Windows x64, hwmon on Linux.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::CpuTemp;

#[cfg(all(windows, target_arch = "x86_64"))]
mod windows;
#[cfg(all(windows, target_arch = "x86_64"))]
pub use windows::CpuTemp;

#[cfg(all(windows, not(target_arch = "x86_64")))]
pub use unsupported::CpuTemp;

/// Windows on ARM: no known way to read the CPU temperature without a vendor driver yet.
#[cfg(all(windows, not(target_arch = "x86_64")))]
mod unsupported {
    use std::io;

    pub struct CpuTemp(());

    impl CpuTemp {
        pub fn open() -> io::Result<Self> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "reading the CPU temperature is not supported on Windows on ARM yet",
            ))
        }

        pub fn read(&mut self) -> io::Result<f32> {
            Self::open().map(|_| 0.0)
        }

        pub fn source(&self) -> &str {
            ""
        }
    }
}
