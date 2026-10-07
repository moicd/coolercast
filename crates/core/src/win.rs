//! Small safe wrappers around the Win32 primitives used across the crate.

use std::ffi::{CStr, OsStr};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, FreeLibrary, HANDLE, HMODULE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, INFINITE, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
    ProcessPowerThrottling, SetProcessInformation, WaitForSingleObject,
};

/// Encodes a string as a null-terminated UTF-16 buffer.
pub fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

/// Decodes a UTF-16 buffer up to its first null character.
pub fn from_wide(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// An owned kernel handle, closed on drop.
#[derive(Debug)]
pub struct Handle(HANDLE);

// Kernel handles can be used from any thread.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Handle {
    /// Takes ownership of `raw`, mapping both null and `INVALID_HANDLE_VALUE` to the last OS error.
    pub fn new(raw: HANDLE) -> io::Result<Self> {
        if raw.is_null() || raw == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(raw))
        }
    }

    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// A manual-reset Win32 event object, signaled by overlapped I/O.
#[derive(Debug)]
pub struct Event(Handle);

impl Event {
    /// Creates an unnamed event.
    pub fn new() -> io::Result<Self> {
        let raw = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        Handle::new(raw).map(Self)
    }

    pub fn raw(&self) -> HANDLE {
        self.0.raw()
    }

    /// Waits until the event is signaled or `timeout` elapses. Returns `true` if signaled.
    pub fn wait(&self, timeout: Duration) -> bool {
        unsafe { WaitForSingleObject(self.raw(), millis(timeout)) == WAIT_OBJECT_0 }
    }
}

/// A DLL loaded at runtime, so it costs nothing until it is needed. Freed on drop.
#[derive(Debug)]
pub struct Library(HMODULE);

impl Library {
    /// A DLL from System32.
    pub fn system(name: &str) -> io::Result<Self> {
        Self::load(name.as_ref(), LOAD_LIBRARY_SEARCH_SYSTEM32)
    }

    /// A DLL at an absolute path. Its own dependencies come from System32 and its own folder,
    /// never from the application folder.
    pub fn at(path: &Path) -> io::Result<Self> {
        let flags = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32;
        Self::load(path.as_os_str(), flags)
    }

    fn load(name: &OsStr, flags: u32) -> io::Result<Self> {
        let module = unsafe { LoadLibraryExW(wide(name).as_ptr(), ptr::null_mut(), flags) };
        if module.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(module))
        }
    }

    /// The address of an export as a function pointer.
    ///
    /// # Safety
    ///
    /// `T` must be the function pointer type of the export, and must not be called after the
    /// library is dropped.
    pub unsafe fn symbol<T: Copy>(&self, name: &CStr) -> Option<T> {
        let address = unsafe { GetProcAddress(self.0, name.as_ptr().cast()) }?;
        Some(unsafe {
            std::mem::transmute_copy::<unsafe extern "system" fn() -> isize, T>(&address)
        })
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        unsafe { FreeLibrary(self.0) };
    }
}

fn millis(timeout: Duration) -> u32 {
    timeout.as_millis().min(u128::from(INFINITE - 1)) as u32
}

/// Turns EcoQoS on for this process, or gives the choice back to Windows.
///
/// With EcoQoS, Windows runs the process on efficiency cores and at the most efficient clock
/// speed, even on AC power. Meant for background work that nobody waits for; interactive code
/// should leave it to Windows, which raises the QoS of focused windows on its own. The base
/// priority is left alone on purpose: at idle priority the display would stop updating under
/// full load, when the temperature matters most.
pub fn set_efficiency_mode(on: bool) -> io::Result<()> {
    let mask = if on {
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED
    } else {
        0
    };
    // ControlMask 0 resets to system management; with the flag, StateMask turns it on.
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: mask,
        StateMask: mask,
    };
    let ok = unsafe {
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            (&raw const state).cast(),
            size_of_val(&state) as u32,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Returns `true` if a process with the given executable name (case-insensitive) is running.
pub fn process_running(exe_name: &str) -> bool {
    let Ok(snapshot) = Handle::new(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) })
    else {
        return false;
    };
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut ok = unsafe { Process32FirstW(snapshot.raw(), &mut entry) };
    while ok != 0 {
        if from_wide(&entry.szExeFile).eq_ignore_ascii_case(exe_name) {
            return true;
        }
        ok = unsafe { Process32NextW(snapshot.raw(), &mut entry) };
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_round_trip() {
        let w = wide("AK400");
        assert_eq!(w.last(), Some(&0));
        assert_eq!(from_wide(&w), "AK400");
    }

    #[test]
    fn efficiency_mode_round_trip() {
        use windows_sys::Win32::System::Threading::GetProcessInformation;

        let read = || {
            let mut state = PROCESS_POWER_THROTTLING_STATE {
                Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                ..Default::default()
            };
            let ok = unsafe {
                GetProcessInformation(
                    GetCurrentProcess(),
                    ProcessPowerThrottling,
                    (&raw mut state).cast(),
                    size_of_val(&state) as u32,
                )
            };
            assert_ne!(ok, 0, "{}", io::Error::last_os_error());
            (state.ControlMask, state.StateMask)
        };
        let eco = PROCESS_POWER_THROTTLING_EXECUTION_SPEED;
        set_efficiency_mode(true).unwrap();
        assert_eq!(read(), (eco, eco));
        set_efficiency_mode(false).unwrap();
        assert_eq!(read(), (0, 0));
    }

    #[test]
    fn library_symbols() {
        let kernel32 = Library::system("kernel32.dll").unwrap();
        type GetTickCount = unsafe extern "system" fn() -> u32;
        assert!(unsafe { kernel32.symbol::<GetTickCount>(c"GetTickCount") }.is_some());
        assert!(unsafe { kernel32.symbol::<GetTickCount>(c"NoSuchExport") }.is_none());
        assert!(Library::system("no-such-library.dll").is_err());
    }

    #[test]
    fn events_signal_and_time_out() {
        use windows_sys::Win32::System::Threading::SetEvent;

        let event = Event::new().unwrap();
        assert!(!event.wait(Duration::from_millis(1)));
        assert_ne!(unsafe { SetEvent(event.raw()) }, 0);
        // Manual-reset: it stays signaled until a new overlapped request clears it.
        assert!(event.wait(Duration::ZERO));
        assert!(event.wait(Duration::ZERO));
    }
}
