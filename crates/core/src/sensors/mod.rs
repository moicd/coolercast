//! CPU sensors: usage from the kernel's idle counters, temperature through PawnIO (Windows) or
//! hwmon (Linux).

pub mod cpu_temp;
pub mod cpu_usage;
#[cfg(all(windows, target_arch = "x86_64"))]
pub mod pawnio;
