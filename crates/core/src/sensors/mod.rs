//! CPU and GPU sensors: CPU usage from the kernel's idle counters; CPU temperature, power and
//! frequency from PawnIO and PDH (Windows) or sysfs (Linux); GPU values from the graphics
//! driver. Everything but the CPU usage and temperature is only read while a connected display
//! shows it.

pub mod cpu_freq;
pub mod cpu_power;
pub mod cpu_temp;
pub mod cpu_usage;
pub mod gpu;
#[cfg(windows)]
pub mod pawnio;
#[cfg(windows)]
pub mod pdh;

use std::arch::x86_64::__cpuid;

/// Temperature, usage, power and clock of one component (CPU or GPU); `None` when unknown.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Values {
    /// Temperature in °C.
    pub temp: Option<f32>,
    /// Usage in percent.
    pub usage: Option<f32>,
    /// Power in watts.
    pub power: Option<f32>,
    /// Clock in MHz.
    pub freq: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vendor {
    Intel,
    Amd,
    Other,
}

pub fn cpu_vendor() -> Vendor {
    let leaf = __cpuid(0);
    let mut id = [0u8; 12];
    id[..4].copy_from_slice(&leaf.ebx.to_le_bytes());
    id[4..8].copy_from_slice(&leaf.edx.to_le_bytes());
    id[8..].copy_from_slice(&leaf.ecx.to_le_bytes());
    match &id {
        b"GenuineIntel" => Vendor::Intel,
        b"AuthenticAMD" => Vendor::Amd,
        _ => Vendor::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_is_detected() {
        assert_ne!(cpu_vendor(), Vendor::Other);
    }
}
