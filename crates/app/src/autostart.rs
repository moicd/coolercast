//! "Start with Windows" through the current user's `Run` key.

use std::{env, ptr};

use coolercast_core::win::{from_wide, wide};
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "CoolerCast";

pub fn enabled() -> bool {
    stored_command().is_some()
}

/// Rewrites an existing entry that points elsewhere or lacks `--tray` (written by version 0.1),
/// so signing in does not open the settings window.
pub fn update() {
    if let (Some(stored), Some(expected)) = (stored_command(), command())
        && stored != expected
    {
        set(true);
    }
}

fn command() -> Option<String> {
    let exe = env::current_exe().ok()?;
    Some(format!("\"{}\" --tray", exe.display()))
}

fn stored_command() -> Option<String> {
    let (key, name) = (wide(RUN_KEY), wide(VALUE_NAME));
    let mut buf = [0u16; 1024];
    let mut size = size_of_val(&buf) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        )
    };
    (status == ERROR_SUCCESS).then(|| from_wide(&buf))
}

pub fn set(enable: bool) -> bool {
    let (key, name) = (wide(RUN_KEY), wide(VALUE_NAME));
    let status = if enable {
        let Some(command) = command() else {
            return false;
        };
        let command = wide(command);
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
