//! CPU package temperature through PawnIO.
//!
//! - Intel: `IA32_TEMPERATURE_TARGET` (TjMax) minus the digital readout of
//!   `IA32_PACKAGE_THERM_STATUS` (or `IA32_THERM_STATUS` without package sensors).
//! - AMD Zen (family 17h–1Ah): Tctl from the SMN register `THM_TCON_CUR_TMP`.

use std::arch::x86_64::__cpuid;
use std::time::Duration;
use std::{fs, io};

use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

use super::pawnio::PawnIo;
use crate::paths;
use crate::win::{Handle, wide};

const MSR_IA32_THERM_STATUS: u64 = 0x19C;
const MSR_IA32_TEMPERATURE_TARGET: u64 = 0x1A2;
const MSR_IA32_PACKAGE_THERM_STATUS: u64 = 0x1B1;

const SMN_THM_TCON_CUR_TMP: u64 = 0x0005_9800;

/// Mutex other monitoring tools use to serialize PCI configuration space access.
const PCI_MUTEX_NAME: &str = r"Global\Access_PCI";

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

/// Reads the CPU temperature in °C.
pub struct CpuTemp {
    backend: Backend,
}

enum Backend {
    Intel {
        pawn: PawnIo,
        tjmax: f32,
        msr: u64,
    },
    Amd {
        pawn: PawnIo,
        pci_mutex: Option<Handle>,
    },
}

impl CpuTemp {
    pub fn open() -> io::Result<Self> {
        let backend = match cpu_vendor() {
            Vendor::Intel => {
                let pawn = PawnIo::load(&read_module("IntelMSR.bin")?)?;
                let tjmax = intel_tjmax(read_msr(&pawn, MSR_IA32_TEMPERATURE_TARGET)?);
                // CPUID.06H:EAX[6]: package thermal management.
                let package = __cpuid(6).eax & (1 << 6) != 0;
                let msr = if package {
                    MSR_IA32_PACKAGE_THERM_STATUS
                } else {
                    MSR_IA32_THERM_STATUS
                };
                Backend::Intel { pawn, tjmax, msr }
            }
            Vendor::Amd => {
                let pawn = PawnIo::load(&read_module("AMDFamily17.bin")?)?;
                let name = wide(PCI_MUTEX_NAME);
                let pci_mutex =
                    Handle::new(unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) }).ok();
                Backend::Amd { pawn, pci_mutex }
            }
            Vendor::Other => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "unsupported CPU vendor",
                ));
            }
        };
        let mut sensor = Self { backend };
        sensor.read()?;
        Ok(sensor)
    }

    pub fn read(&mut self) -> io::Result<f32> {
        match &self.backend {
            Backend::Intel { pawn, tjmax, msr } => {
                Ok(intel_temperature(*tjmax, read_msr(pawn, *msr)?))
            }
            Backend::Amd { pawn, pci_mutex } => {
                let _lock = pci_mutex
                    .as_ref()
                    .map(|m| MutexGuard::acquire(m, Duration::from_millis(50)));
                let mut out = [0u64; 1];
                pawn.execute(c"ioctl_read_smn", &[SMN_THM_TCON_CUR_TMP], &mut out)?;
                Ok(amd_tctl(out[0] as u32))
            }
        }
    }

    /// Human-readable description of the sensor in use.
    pub fn source(&self) -> &'static str {
        match self.backend {
            Backend::Intel {
                msr: MSR_IA32_PACKAGE_THERM_STATUS,
                ..
            } => "Intel package (MSR 0x1B1)",
            Backend::Intel { .. } => "Intel core (MSR 0x19C)",
            Backend::Amd { .. } => "AMD Tctl (SMN 0x59800)",
        }
    }
}

fn read_module(name: &str) -> io::Result<Vec<u8>> {
    let path = paths::module_file(name).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("PawnIO module {name} not found next to the executable"),
        )
    })?;
    fs::read(path)
}

fn read_msr(pawn: &PawnIo, msr: u64) -> io::Result<u64> {
    let mut out = [0u64; 1];
    pawn.execute(c"ioctl_read_msr", &[msr], &mut out)?;
    Ok(out[0])
}

/// TjMax from `IA32_TEMPERATURE_TARGET` bits 23:16 (100 °C if the CPU reports 0).
pub fn intel_tjmax(temperature_target: u64) -> f32 {
    match (temperature_target >> 16) & 0xFF {
        0 => 100.0,
        t => t as f32,
    }
}

/// Temperature from a thermal status MSR: TjMax minus the readout in bits 22:16.
pub fn intel_temperature(tjmax: f32, therm_status: u64) -> f32 {
    tjmax - ((therm_status >> 16) & 0x7F) as f32
}

/// Tctl in °C from `THM_TCON_CUR_TMP`: bits 31:21 in 0.125 °C steps, −49 °C when the
/// range select bit (19) is set.
pub fn amd_tctl(reg: u32) -> f32 {
    let celsius = ((reg >> 21) & 0x7FF) as f32 * 0.125;
    if reg & (1 << 19) != 0 {
        celsius - 49.0
    } else {
        celsius
    }
}

struct MutexGuard<'a>(Option<&'a Handle>);

impl<'a> MutexGuard<'a> {
    /// Waits for the mutex; proceeds without it on timeout rather than skipping the reading.
    fn acquire(mutex: &'a Handle, timeout: Duration) -> Self {
        let result = unsafe { WaitForSingleObject(mutex.raw(), timeout.as_millis() as u32) };
        // WAIT_OBJECT_0 (0) or WAIT_ABANDONED (0x80): we own the mutex.
        Self((result == 0 || result == 0x80).then_some(mutex))
    }
}

impl Drop for MutexGuard<'_> {
    fn drop(&mut self) {
        if let Some(mutex) = self.0 {
            unsafe { ReleaseMutex(mutex.raw()) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intel_decoding() {
        // TjMax 100 °C, readout 59 → 41 °C.
        let target = 100 << 16;
        let status = (59 << 16) | 0x8800_0000;
        assert_eq!(intel_tjmax(target), 100.0);
        assert_eq!(intel_temperature(intel_tjmax(target), status), 41.0);
        assert_eq!(intel_tjmax(0), 100.0);
        // TCC offset and other fields around bits 23:16 are ignored.
        assert_eq!(intel_tjmax((5 << 24) | (95 << 16) | 0x1400), 95.0);
    }

    #[test]
    fn amd_decoding() {
        // 45.5 °C = 364 * 0.125
        assert_eq!(amd_tctl(364 << 21), 45.5);
        // Same raw value with the range select bit: 45.5 - 49 = -3.5
        assert_eq!(amd_tctl((364 << 21) | (1 << 19)), -3.5);
        assert_eq!(amd_tctl((744 << 21) | (1 << 19)), 44.0);
    }

    #[test]
    fn vendor_is_detected() {
        assert_ne!(cpu_vendor(), Vendor::Other);
    }
}
