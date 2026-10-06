//! HID transport for Linux: collections are found in sysfs and reports are written to
//! `/dev/hidrawN`. Opening a device needs root or the udev rule shipped with CoolerCast.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use super::DeviceInfo;

const SYSFS_HIDRAW: &str = "/sys/class/hidraw";
const DEV: &str = "/dev";

/// Lists the HID collections of the given vendors.
pub fn enumerate(vendor_ids: &[u16]) -> io::Result<Vec<DeviceInfo>> {
    enumerate_in(Path::new(SYSFS_HIDRAW), Path::new(DEV), vendor_ids)
}

fn enumerate_in(sysfs: &Path, dev: &Path, vendor_ids: &[u16]) -> io::Result<Vec<DeviceInfo>> {
    let entries = match fs::read_dir(sysfs) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut devices = Vec::new();
    for entry in entries.flatten() {
        let device = entry.path().join("device");
        let Ok(uevent) = fs::read_to_string(device.join("uevent")) else {
            continue;
        };
        let Some(ids) = Uevent::parse(&uevent).filter(|ids| vendor_ids.contains(&ids.vendor_id))
        else {
            continue;
        };
        let Ok(descriptor) = fs::read(device.join("report_descriptor")) else {
            continue;
        };
        let layout = Layout::parse(&descriptor);
        devices.push(DeviceInfo {
            path: dev.join(entry.file_name()).to_string_lossy().into_owned(),
            vendor_id: ids.vendor_id,
            product_id: ids.product_id,
            product: ids.name,
            serial: ids.serial,
            usage_page: layout.usage_page,
            usage: layout.usage,
            output_report_len: layout.output_report_len,
            report_id: layout.report_id,
        });
    }
    devices.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(devices)
}

/// The fields of a HID device's `uevent` file that identify it.
#[derive(Debug, PartialEq, Eq)]
struct Uevent {
    vendor_id: u16,
    product_id: u16,
    name: String,
    serial: String,
}

impl Uevent {
    fn parse(text: &str) -> Option<Self> {
        let mut ids = None;
        let mut name = String::new();
        let mut serial = String::new();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key {
                // HID_ID=<bus>:<vendor>:<product>, all hexadecimal.
                "HID_ID" => {
                    let mut parts = value.split(':').skip(1);
                    let vendor = u32::from_str_radix(parts.next()?, 16).ok()?;
                    let product = u32::from_str_radix(parts.next()?, 16).ok()?;
                    ids = Some((u16::try_from(vendor).ok()?, u16::try_from(product).ok()?));
                }
                "HID_NAME" => name = value.to_owned(),
                "HID_UNIQ" => serial = value.to_owned(),
                _ => {}
            }
        }
        let (vendor_id, product_id) = ids?;
        Some(Self {
            vendor_id,
            product_id,
            name,
            serial,
        })
    }
}

/// What CoolerCast needs from a HID report descriptor.
#[derive(Debug, Default, PartialEq, Eq)]
struct Layout {
    /// Usage page and usage of the first top-level collection.
    usage_page: u16,
    usage: u16,
    /// Report ID of the first output report, 0 when the device has no report IDs.
    report_id: u8,
    /// Size of that output report plus the report ID byte, as on Windows; 0 without output.
    output_report_len: usize,
}

impl Layout {
    /// Walks the short items of a report descriptor (HID 1.11, section 6.2.2).
    fn parse(descriptor: &[u8]) -> Self {
        let mut layout = Layout::default();
        let (mut usage_page, mut usage) = (0u32, None);
        let (mut report_size, mut report_count, mut report_id) = (0u32, 0u32, 0u8);
        let mut collection_seen = false;
        let mut output: Option<(u8, u32)> = None;

        let mut i = 0;
        while i < descriptor.len() {
            let prefix = descriptor[i];
            if prefix == 0xFE {
                // Long item: prefix, data size, tag, data.
                i += 3 + usize::from(descriptor.get(i + 1).copied().unwrap_or(0));
                continue;
            }
            let size = match prefix & 0x03 {
                3 => 4,
                n => usize::from(n),
            };
            let Some(bytes) = descriptor.get(i + 1..i + 1 + size) else {
                break;
            };
            let data = bytes
                .iter()
                .rev()
                .fold(0u32, |acc, &b| (acc << 8) | u32::from(b));
            match prefix & 0xFC {
                0x04 => usage_page = data,
                0x08 => usage = usage.or(Some(data)),
                0x74 => report_size = data,
                0x84 => report_id = data as u8,
                0x94 => report_count = data,
                0xA0 if !collection_seen => {
                    collection_seen = true;
                    layout.usage_page = usage_page as u16;
                    layout.usage = usage.unwrap_or(0) as u16;
                }
                // Output item: count the bits of the first output report only.
                0x90 => match &mut output {
                    None => output = Some((report_id, report_size * report_count)),
                    Some((id, bits)) if *id == report_id => *bits += report_size * report_count,
                    Some(_) => {}
                },
                _ => {}
            }
            // Local items (usage) only apply to the next main item.
            if prefix & 0x0C == 0x00 {
                usage = None;
            }
            i += 1 + size;
        }

        if let Some((id, bits)) = output.filter(|&(_, bits)| bits > 0) {
            layout.report_id = id;
            layout.output_report_len = (bits as usize).div_ceil(8) + 1;
        }
        layout
    }
}

/// A hidraw device opened for writing output reports.
#[derive(Debug)]
pub struct HidDevice {
    file: File,
    buf: Vec<u8>,
}

impl HidDevice {
    pub fn open(info: &DeviceInfo) -> io::Result<Self> {
        if info.output_report_len == 0 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "device has no output report",
            ));
        }
        let file = OpenOptions::new()
            .write(true)
            .open(&info.path)
            .map_err(|e| {
                if e.kind() == io::ErrorKind::PermissionDenied {
                    io::Error::new(
                        e.kind(),
                        format!(
                            "{}: permission denied (install the udev rule or run as root)",
                            info.path
                        ),
                    )
                } else {
                    e
                }
            })?;
        Ok(Self {
            file,
            buf: vec![0; info.output_report_len],
        })
    }

    /// Sends one output report. `report[0]` is the report ID, or 0 for devices without report
    /// IDs, which the kernel strips before sending, like Windows does.
    pub fn write(&mut self, report: &[u8]) -> io::Result<()> {
        let n = report.len().min(self.buf.len());
        self.buf.fill(0);
        self.buf[..n].copy_from_slice(&report[..n]);
        self.file.write_all(&self.buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vendor collection with 64-byte input and output reports and no report ID, like the
    /// `A400-DIGITAL`.
    const UNNUMBERED: &[u8] = &[
        0x06, 0x00, 0xFF, // Usage Page (0xFF00)
        0x09, 0x01, // Usage (1)
        0xA1, 0x01, // Collection (Application)
        0x15, 0x00, 0x26, 0xFF, 0x00, // Logical Min 0, Max 255
        0x75, 0x08, 0x95, 0x40, // Report Size 8, Count 64
        0x09, 0x01, 0x81, 0x02, // Usage, Input
        0x75, 0x08, 0x95, 0x40, // Report Size 8, Count 64
        0x09, 0x01, 0x91, 0x02, // Usage, Output
        0xC0, // End Collection
    ];

    #[test]
    fn unnumbered_output_report() {
        assert_eq!(
            Layout::parse(UNNUMBERED),
            Layout {
                usage_page: 0xFF00,
                usage: 1,
                report_id: 0,
                output_report_len: 65,
            }
        );
    }

    #[test]
    fn numbered_output_report() {
        let descriptor = [
            0x06, 0x00, 0xFF, 0x09, 0x01, 0xA1, 0x01, // vendor collection
            0x85, 0x10, // Report ID 16
            0x75, 0x08, 0x95, 0x3F, 0x09, 0x01, 0x91, 0x02, // 63-byte output
            0x85, 0x20, 0x95, 0x10, 0x09, 0x01, 0x91, 0x02, // second output report, ignored
            0xC0,
        ];
        let layout = Layout::parse(&descriptor);
        assert_eq!(layout.report_id, 16);
        assert_eq!(layout.output_report_len, 64);
    }

    #[test]
    fn truncated_or_empty_descriptors() {
        assert_eq!(Layout::parse(&[]), Layout::default());
        assert_eq!(Layout::parse(&UNNUMBERED[..10]).output_report_len, 0);
    }

    #[test]
    fn uevent_fields() {
        let text = "DRIVER=hid-generic\nHID_ID=0003:00003633:00000001\n\
                    HID_NAME=DeepCool A400-DIGITAL\nHID_PHYS=usb-0000:00:14.0-7/input0\n\
                    HID_UNIQ=1FDC24AC05A6\n";
        assert_eq!(
            Uevent::parse(text),
            Some(Uevent {
                vendor_id: 0x3633,
                product_id: 0x0001,
                name: "DeepCool A400-DIGITAL".into(),
                serial: "1FDC24AC05A6".into(),
            })
        );
        assert_eq!(Uevent::parse("HID_NAME=x\n"), None);
    }

    #[test]
    fn enumerates_a_fake_sysfs_tree() {
        let root = std::env::temp_dir().join(format!("coolercast-hid-{}", std::process::id()));
        let add = |name: &str, uevent: &str| {
            let device = root.join(name).join("device");
            fs::create_dir_all(&device).unwrap();
            fs::write(device.join("uevent"), uevent).unwrap();
            fs::write(device.join("report_descriptor"), UNNUMBERED).unwrap();
        };
        add(
            "hidraw3",
            "HID_ID=0003:00003633:00000001\nHID_NAME=A400-DIGITAL\n",
        );
        add("hidraw0", "HID_ID=0003:0000046D:0000C52B\nHID_NAME=Mouse\n");

        let found = enumerate_in(&root, Path::new("/dev"), &[0x3633]).unwrap();
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, "/dev/hidraw3");
        assert_eq!(found[0].product_id, 1);
        assert_eq!(found[0].output_report_len, 65);
    }
}
