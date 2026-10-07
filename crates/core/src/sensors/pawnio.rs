//! Runtime binding to `PawnIOLib.dll`, the user-mode library of the PawnIO driver
//! (<https://pawnio.eu>). Loaded dynamically so the app still runs without PawnIO.

use std::env;
use std::ffi::CStr;
use std::io;
use std::path::PathBuf;
use std::{fs, ptr};

use windows_sys::Win32::Foundation::HANDLE;

use crate::paths;
use crate::win::Library;

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
    handle: HANDLE,
    execute: ExecuteFn,
    close: CloseFn,
    // Last: the functions above live in this library.
    _library: Library,
}

// The executor handle is a file handle; PawnIOLib calls are thread-safe.
unsafe impl Send for PawnIo {}

impl PawnIo {
    /// Opens the driver and loads a compiled module (`*.bin`).
    pub fn load(module: &[u8]) -> io::Result<Self> {
        let library = load_library()?;
        // SAFETY: the types match the documented signatures of these exports.
        let symbols = unsafe {
            (|| {
                Some((
                    library.symbol::<OpenFn>(c"pawnio_open")?,
                    library.symbol::<LoadFn>(c"pawnio_load")?,
                    library.symbol::<ExecuteFn>(c"pawnio_execute")?,
                    library.symbol::<CloseFn>(c"pawnio_close")?,
                ))
            })()
        };
        let (open, load, execute, close) =
            symbols.ok_or_else(|| io::Error::other("PawnIOLib.dll is missing expected exports"))?;

        let mut handle: HANDLE = ptr::null_mut();
        check(unsafe { open(&mut handle) }, "pawnio_open")?;
        // From here on `Drop` closes the handle.
        let pawn = Self {
            handle,
            execute,
            close,
            _library: library,
        };
        check(
            unsafe { load(pawn.handle, module.as_ptr(), module.len()) },
            "pawnio_load",
        )?;
        Ok(pawn)
    }

    /// Loads one of the modules shipped next to the executable (`third_party/pawnio-modules`).
    pub fn load_module(name: &str) -> io::Result<Self> {
        let path = paths::module_file(name).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("PawnIO module {name} not found next to the executable"),
            )
        })?;
        Self::load(&fs::read(path)?)
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
        unsafe { (self.close)(self.handle) };
    }
}

/// Reads a model-specific register through a module that exports `ioctl_read_msr`.
pub fn read_msr(pawn: &PawnIo, msr: u64) -> io::Result<u64> {
    let mut out = [0u64; 1];
    pawn.execute(c"ioctl_read_msr", &[msr], &mut out)?;
    Ok(out[0])
}

fn load_library() -> io::Result<Library> {
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
    Library::at(&path)
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
