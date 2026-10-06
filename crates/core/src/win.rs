//! Small safe wrappers around the Win32 primitives used across the crate.

use std::ffi::{CStr, OsStr};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::ptr;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, FreeLibrary, HANDLE, HMODULE, INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
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

/// A Win32 event object.
#[derive(Debug)]
pub struct Event(Handle);

impl Event {
    /// Creates an unnamed event. A manual-reset event stays signaled once set.
    pub fn new(manual_reset: bool) -> io::Result<Self> {
        let raw = unsafe { CreateEventW(ptr::null(), manual_reset.into(), 0, ptr::null()) };
        Handle::new(raw).map(Self)
    }

    pub fn set(&self) {
        unsafe { SetEvent(self.0.raw()) };
    }

    pub fn raw(&self) -> HANDLE {
        self.0.raw()
    }

    /// Waits until the event is signaled or `timeout` elapses. Returns `true` if signaled.
    pub fn wait(&self, timeout: Duration) -> bool {
        unsafe { WaitForSingleObject(self.raw(), millis(timeout)) == WAIT_OBJECT_0 }
    }

    pub fn is_set(&self) -> bool {
        self.wait(Duration::ZERO)
    }
}

/// A DLL from System32 loaded at runtime, so it costs nothing until it is needed. Freed on drop.
#[derive(Debug)]
pub struct Library(HMODULE);

impl Library {
    pub fn system(name: &str) -> io::Result<Self> {
        let module = unsafe {
            LoadLibraryExW(
                wide(name).as_ptr(),
                ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
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

/// Waits until any of `events` is signaled and returns its index, or `None` on timeout.
pub fn wait_any(events: &[&Event], timeout: Duration) -> Option<usize> {
    let handles: Vec<HANDLE> = events.iter().map(|e| e.raw()).collect();
    let result = unsafe {
        WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, millis(timeout))
    };
    match result {
        WAIT_TIMEOUT | WAIT_FAILED => None,
        r => Some((r - WAIT_OBJECT_0) as usize).filter(|&i| i < handles.len()),
    }
}

fn millis(timeout: Duration) -> u32 {
    timeout.as_millis().min(u128::from(INFINITE - 1)) as u32
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
    fn library_symbols() {
        let kernel32 = Library::system("kernel32.dll").unwrap();
        type GetTickCount = unsafe extern "system" fn() -> u32;
        assert!(unsafe { kernel32.symbol::<GetTickCount>(c"GetTickCount") }.is_some());
        assert!(unsafe { kernel32.symbol::<GetTickCount>(c"NoSuchExport") }.is_none());
        assert!(Library::system("no-such-library.dll").is_err());
    }

    #[test]
    fn events_signal_and_time_out() {
        let a = Event::new(true).unwrap();
        let b = Event::new(false).unwrap();
        assert_eq!(wait_any(&[&a, &b], Duration::from_millis(1)), None);
        b.set();
        assert_eq!(wait_any(&[&a, &b], Duration::ZERO), Some(1));
        // Auto-reset: consumed by the previous wait.
        assert!(!b.is_set());
        a.set();
        assert!(a.is_set());
        assert!(a.is_set());
    }
}
