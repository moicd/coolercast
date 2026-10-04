//! Runtime binding to `PawnIOLib.dll`, the user-mode library of the PawnIO driver
//! (<https://pawnio.eu>). Loaded dynamically so the app still runs without PawnIO.

use std::env;
use std::ffi::CStr;
use std::io;
use std::path::PathBuf;
use std::ptr;

use windows_sys::Win32::Foundation::{FreeLibrary, HANDLE, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};

use crate::win::wide;

type OpenFn = unsafe extern "system" fn(*mut HANDLE) -> i32;
type LoadFn = unsafe extern "system" fn(HANDLE, *const u8, usize) -> i32;
type ExecuteFn = unsafe extern "system" fn(
    HANDLE,
    *const u8,
    *const u64,
    usize,
    *mut u64,
    usize,
    *mut usize,
) -> i32;
type CloseFn = unsafe extern "system" fn(HANDLE) -> i32;

const E_ACCESSDENIED: u32 = 0x8007_0005;

pub const INSTALL_HINT: &str = "install it with `winget install namazso.PawnIO`";

/// A PawnIO executor with one module loaded.
pub struct PawnIo {
    library: HMODULE,
    handle: HANDLE,
    execute: ExecuteFn,
    close: CloseFn,
}

// The executor handle is a file handle; PawnIOLib calls are thread-safe.
unsafe impl Send for PawnIo {}

impl PawnIo {
    /// Opens the driver and loads a compiled module (`*.bin`).
    pub fn load(module: &[u8]) -> io::Result<Self> {
        let library = load_library()?;
        let symbols = (|| unsafe {
            Some((
                symbol::<OpenFn>(library, c"pawnio_open")?,
                symbol::<LoadFn>(library, c"pawnio_load")?,
                symbol::<ExecuteFn>(library, c"pawnio_execute")?,
                symbol::<CloseFn>(library, c"pawnio_close")?,
            ))
        })();
        let Some((open, load, execute, close)) = symbols else {
            unsafe { FreeLibrary(library) };
            return Err(io::Error::other(
                "PawnIOLib.dll is missing expected exports",
            ));
        };

        let mut handle: HANDLE = ptr::null_mut();
        if let Err(e) = check(unsafe { open(&mut handle) }, "pawnio_open") {
            unsafe { FreeLibrary(library) };
            return Err(e);
        }
        // From here on `Drop` releases both the handle and the library.
        let pawn = Self {
            library,
            handle,
            execute,
            close,
        };
        check(
            unsafe { load(pawn.handle, module.as_ptr(), module.len()) },
            "pawnio_load",
        )?;
        Ok(pawn)
    }

    /// Calls a function exported by the loaded module and returns how many outputs it wrote.
    pub fn execute(&self, function: &CStr, input: &[u64], output: &mut [u64]) -> io::Result<usize> {
        let mut written = 0usize;
        let hr = unsafe {
            (self.execute)(
                self.handle,
                function.as_ptr().cast(),
                input.as_ptr(),
                input.len(),
                output.as_mut_ptr(),
                output.len(),
                &mut written,
            )
        };
        check(hr, "pawnio_execute")?;
        Ok(written)
    }
}

impl Drop for PawnIo {
    fn drop(&mut self) {
        unsafe {
            (self.close)(self.handle);
            FreeLibrary(self.library);
        }
    }
}

fn load_library() -> io::Result<HMODULE> {
    let program_files = env::var_os("ProgramFiles")
        .map_or_else(|| PathBuf::from(r"C:\Program Files"), PathBuf::from);
    let path = program_files.join("PawnIO").join("PawnIOLib.dll");
    if !path.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("PawnIO is not installed; {INSTALL_HINT}"),
        ));
    }
    // Absolute path + restricted search: never pick up a DLL planted next to the executable.
    let library = unsafe {
        LoadLibraryExW(
            wide(&path).as_ptr(),
            ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    if library.is_null() {
        Err(io::Error::last_os_error())
    } else {
        Ok(library)
    }
}

unsafe fn symbol<T>(library: HMODULE, name: &CStr) -> Option<T> {
    let address = unsafe { GetProcAddress(library, name.as_ptr().cast()) }?;
    // SAFETY: `T` is the documented signature of the export.
    Some(unsafe { std::mem::transmute_copy::<unsafe extern "system" fn() -> isize, T>(&address) })
}

fn check(hr: i32, what: &str) -> io::Result<()> {
    if hr >= 0 {
        return Ok(());
    }
    if hr as u32 == E_ACCESSDENIED {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{what}: access denied (PawnIO requires administrator rights)"),
        ));
    }
    Err(io::Error::other(format!(
        "{what} failed (HRESULT 0x{:08X})",
        hr as u32
    )))
}
