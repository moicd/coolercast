//! Named-pipe transport for Windows.

use std::io;
use std::ptr;
use std::thread::{self, JoinHandle};

use windows_sys::Win32::Foundation::{ERROR_PIPE_CONNECTED, GetLastError, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FlushFileBuffers, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows_sys::Win32::System::Pipes::{
    CallNamedPipeW, ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_MESSAGE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_WAIT,
};

use crate::win::{Handle, wide};

const PIPE_NAME: &str = r"\\.\pipe\coolercast";

const BUFFER_SIZE: u32 = 4096;
const CLIENT_TIMEOUT_MS: u32 = 1000;

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
    // Create the first instance up front so a name conflict is reported to the caller.
    let first = create_pipe(&security)?;
    Ok(thread::spawn(move || {
        let mut pipe = Some(first);
        loop {
            let current = match pipe.take().map_or_else(|| create_pipe(&security), Ok) {
                Ok(p) => p,
                Err(e) => {
                    crate::error!("IPC: cannot create pipe: {e}");
                    return;
                }
            };
            handle_client(&current, &handler);
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
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            BUFFER_SIZE,
            BUFFER_SIZE,
            0,
            &attributes,
        )
    })
}

fn handle_client(pipe: &Handle, handler: &impl Fn(&str) -> String) {
    let h = pipe.raw();
    let connected = unsafe { ConnectNamedPipe(h, ptr::null_mut()) } != 0
        || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
    if connected {
        let mut buf = vec![0u8; BUFFER_SIZE as usize];
        let mut read = 0u32;
        let ok = unsafe {
            ReadFile(
                h,
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut read,
                ptr::null_mut(),
            )
        };
        if ok != 0 {
            let request = String::from_utf8_lossy(&buf[..read as usize]);
            let response = handler(request.trim());
            let mut written = 0u32;
            unsafe {
                WriteFile(
                    h,
                    response.as_ptr(),
                    response.len() as u32,
                    &mut written,
                    ptr::null_mut(),
                );
                FlushFileBuffers(h);
            }
        }
    }
    unsafe { DisconnectNamedPipe(h) };
}

struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

// The descriptor is immutable after creation.
unsafe impl Send for SecurityDescriptor {}

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
