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

/// Report ID of the display report in the AK, AG, LS, LD, LQ, PRO and CH protocols.
const DISPLAY_REPORT_ID: u8 = 16;

/// Chooses the output report among the IDs a collection declares. Some coolers, like the LD
/// series, declare several output reports, and only report 16 drives the display.
fn pick_report_id(ids: impl IntoIterator<Item = u8>) -> Option<u8> {
    let mut first = None;
    for id in ids {
        if id == DISPLAY_REPORT_ID {
            return Some(id);
        }
        first = first.or(Some(id));
    }
    first
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_the_display_report() {
        assert_eq!(pick_report_id([2, 3, 4, 16]), Some(16));
        assert_eq!(pick_report_id([16]), Some(16));
    }

    #[test]
    fn falls_back_to_the_first_report() {
        assert_eq!(pick_report_id([2, 3]), Some(2));
        assert_eq!(pick_report_id([0]), Some(0));
        assert_eq!(pick_report_id([]), None);
    }
}
