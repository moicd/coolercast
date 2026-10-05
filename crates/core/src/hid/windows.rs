//! HID transport for Windows, built directly on SetupAPI and the Win32 HID functions.

use std::time::Duration;
use std::{io, mem, ptr};

use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA,
    SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces,
    SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
};
use windows_sys::Win32::Devices::HumanInterfaceDevice::{
    HIDD_ATTRIBUTES, HIDP_BUTTON_CAPS, HIDP_CAPS, HIDP_STATUS_SUCCESS, HIDP_VALUE_CAPS,
    HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetHidGuid, HidD_GetPreparsedData,
    HidD_GetProductString, HidD_GetSerialNumberString, HidD_SetOutputReport, HidP_GetButtonCaps,
    HidP_GetCaps, HidP_GetValueCaps, HidP_Output, PHIDP_PREPARSED_DATA,
};
use windows_sys::Win32::Foundation::{
    ERROR_INVALID_FUNCTION, ERROR_IO_PENDING, GENERIC_WRITE, GetLastError,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::core::GUID;

use super::DeviceInfo;
use crate::win::{Event, Handle, from_wide, wide};

/// How long a single output report may take before the write is cancelled.
const WRITE_TIMEOUT: Duration = Duration::from_millis(1000);

/// Lists the present HID collections of a vendor.
pub fn enumerate(vendor_id: u16) -> io::Result<Vec<DeviceInfo>> {
    let mut guid: GUID = unsafe { mem::zeroed() };
    unsafe { HidD_GetHidGuid(&mut guid) };

    let set = unsafe {
        SetupDiGetClassDevsW(
            &guid,
            ptr::null(),
            ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if set as isize == -1 {
        return Err(io::Error::last_os_error());
    }

    let mut devices = Vec::new();
    for index in 0.. {
        let mut iface: SP_DEVICE_INTERFACE_DATA = unsafe { mem::zeroed() };
        iface.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
        if unsafe { SetupDiEnumDeviceInterfaces(set, ptr::null(), &guid, index, &mut iface) } == 0 {
            break;
        }
        let Some(path) = interface_path(set, &iface) else {
            continue;
        };
        if let Some(info) = query(&path).filter(|d| d.vendor_id == vendor_id) {
            devices.push(info);
        }
    }

    unsafe { SetupDiDestroyDeviceInfoList(set) };
    Ok(devices)
}

fn interface_path(
    set: windows_sys::Win32::Devices::DeviceAndDriverInstallation::HDEVINFO,
    iface: &SP_DEVICE_INTERFACE_DATA,
) -> Option<String> {
    let mut required = 0u32;
    unsafe {
        SetupDiGetDeviceInterfaceDetailW(
            set,
            iface,
            ptr::null_mut(),
            0,
            &mut required,
            ptr::null_mut(),
        )
    };
    if required == 0 {
        return None;
    }
    // u64 storage keeps the detail struct correctly aligned.
    let mut buf = vec![0u64; (required as usize).div_ceil(8)];
    let detail = buf.as_mut_ptr().cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
    unsafe { (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32 };
    let ok = unsafe {
        SetupDiGetDeviceInterfaceDetailW(
            set,
            iface,
            detail,
            required,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return None;
    }
    let path_offset = mem::offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath);
    let chars = (required as usize - path_offset) / 2;
    let path = unsafe {
        std::slice::from_raw_parts(
            buf.as_ptr().cast::<u8>().add(path_offset).cast::<u16>(),
            chars,
        )
    };
    Some(from_wide(path))
}

/// Reads attributes, strings and report layout of one collection without write access.
fn query(path: &str) -> Option<DeviceInfo> {
    let handle = open(path, 0, 0).ok()?;
    let h = handle.raw();

    let mut attrs: HIDD_ATTRIBUTES = unsafe { mem::zeroed() };
    attrs.Size = size_of::<HIDD_ATTRIBUTES>() as u32;
    if !unsafe { HidD_GetAttributes(h, &mut attrs) } {
        return None;
    }

    let mut text = [0u16; 128];
    let byte_len = size_of_val(&text) as u32;
    let product = if unsafe { HidD_GetProductString(h, text.as_mut_ptr().cast(), byte_len) } {
        from_wide(&text)
    } else {
        String::new()
    };
    text.fill(0);
    let serial = if unsafe { HidD_GetSerialNumberString(h, text.as_mut_ptr().cast(), byte_len) } {
        from_wide(&text)
    } else {
        String::new()
    };

    let mut preparsed: PHIDP_PREPARSED_DATA = unsafe { mem::zeroed() };
    if !unsafe { HidD_GetPreparsedData(h, &mut preparsed) } {
        return None;
    }
    let mut caps: HIDP_CAPS = unsafe { mem::zeroed() };
    let caps_ok = unsafe { HidP_GetCaps(preparsed, &mut caps) } == HIDP_STATUS_SUCCESS;
    let report_id = if caps_ok {
        output_report_id(preparsed, &caps)
    } else {
        0
    };
    unsafe { HidD_FreePreparsedData(preparsed) };
    if !caps_ok {
        return None;
    }

    Some(DeviceInfo {
        path: path.to_owned(),
        vendor_id: attrs.VendorID,
        product_id: attrs.ProductID,
        product,
        serial,
        usage_page: caps.UsagePage,
        usage: caps.Usage,
        output_report_len: caps.OutputReportByteLength as usize,
        report_id,
    })
}

/// Finds the report ID used by the output report, looking at value caps first, then buttons.
fn output_report_id(preparsed: PHIDP_PREPARSED_DATA, caps: &HIDP_CAPS) -> u8 {
    if caps.NumberOutputValueCaps > 0 {
        let mut len = caps.NumberOutputValueCaps;
        let mut values: Vec<HIDP_VALUE_CAPS> = vec![unsafe { mem::zeroed() }; len as usize];
        let status =
            unsafe { HidP_GetValueCaps(HidP_Output, values.as_mut_ptr(), &mut len, preparsed) };
        if status == HIDP_STATUS_SUCCESS && len > 0 {
            return values[0].ReportID;
        }
    }
    if caps.NumberOutputButtonCaps > 0 {
        let mut len = caps.NumberOutputButtonCaps;
        let mut buttons: Vec<HIDP_BUTTON_CAPS> = vec![unsafe { mem::zeroed() }; len as usize];
        let status =
            unsafe { HidP_GetButtonCaps(HidP_Output, buttons.as_mut_ptr(), &mut len, preparsed) };
        if status == HIDP_STATUS_SUCCESS && len > 0 {
            return buttons[0].ReportID;
        }
    }
    0
}

fn open(path: &str, access: u32, flags: u32) -> io::Result<Handle> {
    let path = wide(path);
    Handle::new(unsafe {
        CreateFileW(
            path.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            flags,
            ptr::null_mut(),
        )
    })
}

/// A HID collection opened for writing output reports.
#[derive(Debug)]
pub struct HidDevice {
    handle: Handle,
    event: Event,
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
        Ok(Self {
            handle: open(&info.path, GENERIC_WRITE, FILE_FLAG_OVERLAPPED)?,
            event: Event::new(true)?,
            buf: vec![0; info.output_report_len],
        })
    }

    /// Sends one output report. `report[0]` is the report ID; the report is zero-padded or
    /// truncated to the length the device declares.
    pub fn write(&mut self, report: &[u8]) -> io::Result<()> {
        let n = report.len().min(self.buf.len());
        self.buf.fill(0);
        self.buf[..n].copy_from_slice(&report[..n]);

        let h = self.handle.raw();
        let mut ov: OVERLAPPED = unsafe { mem::zeroed() };
        ov.hEvent = self.event.raw();
        let ok = unsafe {
            WriteFile(
                h,
                self.buf.as_ptr(),
                self.buf.len() as u32,
                ptr::null_mut(),
                &mut ov,
            )
        };
        if ok == 0 {
            match unsafe { GetLastError() } {
                ERROR_IO_PENDING => {}
                // No interrupt OUT endpoint: fall back to a control transfer.
                ERROR_INVALID_FUNCTION => return self.set_output_report(),
                err => return Err(io::Error::from_raw_os_error(err as i32)),
            }
        }

        if !self.event.wait(WRITE_TIMEOUT) {
            let mut written = 0u32;
            unsafe {
                CancelIoEx(h, &ov);
                // Wait for the cancellation so `ov` and the buffer outlive the request.
                GetOverlappedResult(h, &ov, &mut written, 1);
            }
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "HID write timed out",
            ));
        }
        let mut written = 0u32;
        if unsafe { GetOverlappedResult(h, &ov, &mut written, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn set_output_report(&self) -> io::Result<()> {
        let ok = unsafe {
            HidD_SetOutputReport(
                self.handle.raw(),
                self.buf.as_ptr().cast(),
                self.buf.len() as u32,
            )
        };
        if ok {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}
