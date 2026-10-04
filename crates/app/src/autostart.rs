//! "Start with Windows" through the current user's `Run` key.

use std::{env, ptr};

use coolercast_core::win::wide;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "CoolerCast";

pub fn enabled() -> bool {
    let (key, name) = (wide(RUN_KEY), wide(VALUE_NAME));
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        ) == ERROR_SUCCESS
    }
}

pub fn set(enable: bool) -> bool {
    let (key, name) = (wide(RUN_KEY), wide(VALUE_NAME));
    let status = if enable {
        let Ok(exe) = env::current_exe() else {
            return false;
        };
        let command = wide(format!("\"{}\"", exe.display()));
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                command.as_ptr().cast(),
                (command.len() * 2) as u32,
            )
        }
    } else {
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) }
    };
    status == ERROR_SUCCESS
}
