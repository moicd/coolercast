//! Package energy counter on Windows x64, through PawnIO.
//!
//! - Intel: `MSR_RAPL_POWER_UNIT` (0x606) and `MSR_PKG_ENERGY_STATUS` (0x611).
//! - AMD Zen: `MSR_PWR_UNIT` (0xC0010299) and `MSR_PKG_ENERGY_STAT` (0xC001029B).
//!
//! Both counters are 32 bits wide and count in units of 1/2^ESU joules, where ESU is bits 12:8
//! of the unit register.

use std::io;

use crate::sensors::pawnio::{PawnIo, read_msr};
use crate::sensors::{Vendor, cpu_vendor};

const MSR_RAPL_POWER_UNIT: u64 = 0x606;
const MSR_PKG_ENERGY_STATUS: u64 = 0x611;
const MSR_AMD_PWR_UNIT: u64 = 0xC001_0299;
const MSR_AMD_PKG_ENERGY_STAT: u64 = 0xC001_029B;

pub struct Counter {
    pawn: PawnIo,
    msr: u64,
    joules_per_count: f64,
    source: &'static str,
}

impl Counter {
    pub fn open() -> io::Result<Self> {
        let (module, unit_msr, msr, source) = match cpu_vendor() {
            Vendor::Intel => (
                "IntelMSR.bin",
                MSR_RAPL_POWER_UNIT,
                MSR_PKG_ENERGY_STATUS,
                "Intel RAPL package energy (MSR 0x611)",
            ),
            Vendor::Amd => (
                "AMDFamily17.bin",
                MSR_AMD_PWR_UNIT,
                MSR_AMD_PKG_ENERGY_STAT,
                "AMD RAPL package energy (MSR 0xC001029B)",
            ),
            Vendor::Other => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "unsupported CPU vendor",
                ));
            }
        };
        let pawn = PawnIo::load_module(module)?;
        let joules_per_count = joules_per_count(read_msr(&pawn, unit_msr)?);
        Ok(Self {
            pawn,
            msr,
            joules_per_count,
            source,
        })
    }

    pub fn read(&self) -> io::Result<u64> {
        Ok(read_msr(&self.pawn, self.msr)? & 0xFFFF_FFFF)
    }

    pub fn range(&self) -> u64 {
        1 << 32
    }

    pub fn joules_per_count(&self) -> f64 {
        self.joules_per_count
    }

    pub fn source(&self) -> &str {
        self.source
    }
}

/// Energy of one counter step from the energy status units (bits 12:8) of a RAPL unit register.
pub fn joules_per_count(unit_register: u64) -> f64 {
    let esu = (unit_register >> 8) & 0x1F;
    1.0 / (1u64 << esu) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn energy_units() {
        // Typical Intel value 0x000A0E03: ESU = 14 → 61 µJ.
        assert_eq!(joules_per_count(0x000A_0E03), 1.0 / 16384.0);
        // Typical AMD Zen value 0x000A1003: ESU = 16 → 15.3 µJ.
        assert_eq!(joules_per_count(0x000A_1003), 1.0 / 65536.0);
    }
}
