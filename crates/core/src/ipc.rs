//! Local IPC between the service and its clients (CLI, tray) over a message-mode named pipe.
//!
//! Requests are single lines: `status` or `set <key>=<value>`. Responses start with `ok` or
//! `error: <message>`; a status response continues with one `key=value` per line.

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

use crate::config::Config;
use crate::win::{Handle, wide};

pub const PIPE_NAME: &str = r"\\.\pipe\deepcool-native";

const BUFFER_SIZE: u32 = 4096;
const CLIENT_TIMEOUT_MS: u32 = 1000;

/// Full control for SYSTEM and administrators, read/write for interactive users.
const PIPE_SDDL: &str = "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)";

/// Sends one request to the running service and returns its response.
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

/// Serves requests on a background thread, one client at a time.
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

/// Snapshot of what the service is doing, as reported by `status`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    pub devices: Vec<String>,
    pub cpu_temp: Option<f32>,
    pub cpu_usage: Option<f32>,
    /// Why the temperature is unavailable, if it is.
    pub temp_error: Option<String>,
    pub config: Config,
}

impl Status {
    pub fn encode(&self) -> String {
        let mut out = String::new();
        let mut line = |key: &str, value: &str| {
            out.push_str(key);
            out.push('=');
            out.push_str(&value.replace(['\r', '\n'], " "));
            out.push('\n');
        };
        line("devices", &self.devices.join(";"));
        line(
            "cpu_temp",
            &self.cpu_temp.map_or(String::new(), |t| format!("{t:.1}")),
        );
        line(
            "cpu_usage",
            &self.cpu_usage.map_or(String::new(), |u| format!("{u:.1}")),
        );
        line("temp_error", self.temp_error.as_deref().unwrap_or(""));
        for (key, value) in self.config.entries() {
            line(key, &value);
        }
        out
    }

    pub fn decode(text: &str) -> Result<Self, String> {
        let mut status = Status::default();
        for entry in text.lines().filter(|l| !l.is_empty()) {
            let (key, value) = entry
                .split_once('=')
                .ok_or_else(|| format!("bad line '{entry}'"))?;
            let number = || value.parse::<f32>().ok();
            match key {
                "devices" => {
                    status.devices = value
                        .split(';')
                        .filter(|d| !d.is_empty())
                        .map(Into::into)
                        .collect()
                }
                "cpu_temp" => status.cpu_temp = number(),
                "cpu_usage" => status.cpu_usage = number(),
                "temp_error" => {
                    status.temp_error = Some(value.to_owned()).filter(|e| !e.is_empty())
                }
                // Ignore settings added by a newer service.
                _ => {
                    let _ = status.config.set(key, value);
                }
            }
        }
        Ok(status)
    }
}

/// Asks the service for its status.
pub fn query_status() -> io::Result<Status> {
    let response = request("status")?;
    let body = response
        .strip_prefix("ok\n")
        .ok_or_else(|| io::Error::other(response.trim().to_owned()))?;
    Status::decode(body).map_err(io::Error::other)
}

/// Changes a setting on the running service.
pub fn set(key: &str, value: &str) -> io::Result<()> {
    let response = request(&format!("set {key}={value}"))?;
    match response.trim() {
        "ok" => Ok(()),
        other => Err(io::Error::other(
            other.trim_start_matches("error: ").to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Mode, Unit};

    #[test]
    fn status_round_trip() {
        let status = Status {
            devices: vec!["AK400 DIGITAL".into(), "AK620 DIGITAL".into()],
            cpu_temp: Some(41.5),
            cpu_usage: Some(12.0),
            temp_error: None,
            config: Config {
                mode: Mode::Auto,
                unit: Unit::Fahrenheit,
                ..Config::default()
            },
        };
        assert_eq!(Status::decode(&status.encode()).unwrap(), status);

        let empty = Status {
            temp_error: Some("PawnIO is not installed".into()),
            ..Status::default()
        };
        assert_eq!(Status::decode(&empty.encode()).unwrap(), empty);
    }
}
