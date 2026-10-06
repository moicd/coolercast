//! CPU sensors: usage from the kernel's idle counters; temperature, power and frequency from
//! PawnIO and PDH (Windows) or sysfs (Linux). Power and frequency are only read when a connected
//! display shows them.

pub mod cpu_freq;
pub mod cpu_power;
pub mod cpu_temp;
pub mod cpu_usage;
#[cfg(windows)]
pub mod pawnio;

use std::arch::x86_64::__cpuid;

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
