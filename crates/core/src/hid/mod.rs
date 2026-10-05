//! HID transport: enumerates the collections of a vendor and writes output reports.
//!
//! Both platforms take the same buffer: the report ID (or 0 when the device has none) followed by
//! the payload, padded to `output_report_len`.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{HidDevice, enumerate};
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{HidDevice, enumerate};

/// A HID top-level collection found during enumeration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub path: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub product: String,
    pub serial: String,
    pub usage_page: u16,
    pub usage: u16,
    /// Size of an output report including the report ID byte (0 when unused).
    pub output_report_len: usize,
    /// Report ID of the output report, or 0 if the device does not use report IDs.
    pub report_id: u8,
}
