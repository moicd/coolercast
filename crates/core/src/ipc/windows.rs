//! Named-pipe transport for Windows.

use std::io;
use std::mem;
use std::ptr;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_PIPE_CONNECTED, GetLastError, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    CallNamedPipeW, ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_MESSAGE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_WAIT,
};
use windows_sys::core::BOOL;

use crate::win::{Event, Handle, wide};

const PIPE_NAME: &str = r"\\.\pipe\coolercast";

const BUFFER_SIZE: u32 = 4096;
const CLIENT_TIMEOUT_MS: u32 = 1000;
/// How long a connected client may take to send its request, and to read the response and close.
const SERVER_TIMEOUT: Duration = Duration::from_secs(1);

/// Full control for SYSTEM and administrators, read/write for interactive users.
const PIPE_SDDL: &str = "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)";

pub fn request(command: &str) -> io::Result<String> {
    let name = wide(PIPE_NAME);
    let mut response = vec![0u8; BUFFER_SIZE as usize];
    let mut read = 0u32;
    let ok = unsafe {
        CallNamedPipeW(
            name.as_ptr(),
            command.as_ptr().cast(),
            command.len() as u32,
            response.as_mut_ptr().cast(),
            response.len() as u32,
            &mut read,
            CLIENT_TIMEOUT_MS,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    response.truncate(read as usize);
    Ok(String::from_utf8_lossy(&response).into_owned())
}

pub fn serve<F>(handler: F) -> io::Result<JoinHandle<()>>
where
    F: Fn(&str) -> String + Send + 'static,
{
    let security = SecurityDescriptor::from_sddl(PIPE_SDDL)?;
    // One instance for the life of the service, created here so a name conflict is reported to
    // the caller. Closing it between clients would free the name for any local process to take.
    let pipe = create_pipe(&security)?;
    let event = Event::new()?;
    Ok(thread::spawn(move || {
        loop {
            if let Err(e) = handle_client(&pipe, &event, &handler) {
                crate::error!("IPC: cannot wait for clients: {e}");
                return;
            }
        }
    }))
}

fn create_pipe(security: &SecurityDescriptor) -> io::Result<Handle> {
    let name = wide(PIPE_NAME);
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: security.0,
        bInheritHandle: 0,
    };
    Handle::new(unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            BUFFER_SIZE,
            BUFFER_SIZE,
            0,
            &attributes,
        )
    })
}

/// Serves one client, then leaves the instance ready for the next one. Fails only when the pipe
/// cannot wait for clients any more. A client that stalls is dropped after [`SERVER_TIMEOUT`], so
/// it cannot block the others.
fn handle_client(
    pipe: &Handle,
    event: &Event,
    handler: &impl Fn(&str) -> String,
) -> io::Result<()> {
    let h = pipe.raw();
    let connected = match overlapped(pipe, event, None, |ov| unsafe { ConnectNamedPipe(h, ov) }) {
        Ok(_) => true,
        Err(e) if e.raw_os_error() == Some(ERROR_PIPE_CONNECTED as i32) => true,
        // The client connected and left before the call.
        Err(e) if e.raw_os_error() == Some(ERROR_NO_DATA as i32) => false,
        Err(e) => return Err(e),
    };
    if connected {
        let mut buf = vec![0u8; BUFFER_SIZE as usize];
        let read = |buf: &mut [u8]| {
            overlapped(pipe, event, Some(SERVER_TIMEOUT), |ov| unsafe {
                ReadFile(h, buf.as_mut_ptr(), buf.len() as u32, ptr::null_mut(), ov)
            })
        };
        if let Ok(n) = read(&mut buf) {
            let response = handler(String::from_utf8_lossy(&buf[..n as usize]).trim());
            let written = overlapped(pipe, event, Some(SERVER_TIMEOUT), |ov| unsafe {
                WriteFile(
                    h,
                    response.as_ptr(),
                    response.len() as u32,
                    ptr::null_mut(),
                    ov,
                )
            });
            // Disconnecting discards what the client has not read yet, and FlushFileBuffers would
            // wait for it without a limit. The client closes its end once it has the response,
            // which ends this read.
            if written.is_ok() {
                let _ = read(&mut buf);
            }
        }
    }
    unsafe { DisconnectNamedPipe(h) };
    Ok(())
}

/// Starts an overlapped operation on the pipe with `start` and waits for it, cancelling it after
/// `timeout` if one is given. Returns the number of bytes transferred.
fn overlapped(
    pipe: &Handle,
    event: &Event,
    timeout: Option<Duration>,
    start: impl FnOnce(*mut OVERLAPPED) -> BOOL,
) -> io::Result<u32> {
    let h = pipe.raw();
    let mut ov: OVERLAPPED = unsafe { mem::zeroed() };
    ov.hEvent = event.raw();
    if start(&raw mut ov) == 0 {
        let error = unsafe { GetLastError() };
        if error != ERROR_IO_PENDING {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
        if timeout.is_some_and(|t| !event.wait(t)) {
            unsafe { CancelIoEx(h, &raw const ov) };
        }
    }
    // Waits for the operation or its cancellation, so `ov` and the buffer outlive the request.
    let mut transferred = 0u32;
    if unsafe { GetOverlappedResult(h, &raw const ov, &mut transferred, 1) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(transferred)
}

struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

impl SecurityDescriptor {
    fn from_sddl(sddl: &str) -> io::Result<Self> {
        let sddl = wide(sddl);
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(descriptor))
        }
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0) };
    }
}
