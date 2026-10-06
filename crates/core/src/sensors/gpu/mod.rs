//! GPU sensors: temperature, usage, power and core clock of the main graphics card, for NVIDIA,
//! AMD and Intel. Only opened while a connected display shows GPU values.
//!
//! - Windows: NVML (`nvml.dll`, shipped with the NVIDIA driver) for NVIDIA cards; for the others
//!   the vendor-neutral D3DKMT performance data Task Manager uses, plus the `GPU Engine` counters.
//! - Linux: the `amdgpu`, `i915`/`xe` and `nouveau` sysfs files, and `nvidia-smi` for the
//!   proprietary NVIDIA driver.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::Gpu;

#[cfg(windows)]
mod nvml;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::Gpu;

pub use super::Values;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vendor {
    Nvidia,
    Amd,
    Intel,
    Other,
}

impl Vendor {
    pub fn from_pci_id(id: u32) -> Self {
        match id {
            0x10DE => Vendor::Nvidia,
            0x1002 => Vendor::Amd,
            0x8086 => Vendor::Intel,
            _ => Vendor::Other,
        }
    }

    /// Lower is preferred when there are several GPUs: the gaming card is usually a discrete
    /// NVIDIA or AMD one, while Intel is usually the integrated GPU.
    pub fn rank(self) -> u8 {
        match self {
            Vendor::Nvidia => 0,
            Vendor::Amd => 1,
            Vendor::Intel => 2,
            Vendor::Other => 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendors() {
        assert_eq!(Vendor::from_pci_id(0x10DE), Vendor::Nvidia);
        assert_eq!(Vendor::from_pci_id(0x1002), Vendor::Amd);
        assert_eq!(Vendor::from_pci_id(0x8086), Vendor::Intel);
        assert_eq!(Vendor::from_pci_id(0x1414), Vendor::Other);
        assert!(Vendor::Amd.rank() < Vendor::Intel.rank());
    }
}
