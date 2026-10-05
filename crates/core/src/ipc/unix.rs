//! Unix socket transport for Linux. Clients write one request, shut down their write half and
//! read the response until the service closes the connection.

use std::fs::{self, Permissions};
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::paths;

const TIMEOUT: Duration = Duration::from_secs(1);
const MAX_MESSAGE: u64 = 64 * 1024;

pub fn request(command: &str) -> io::Result<String> {
    let mut stream = UnixStream::connect(paths::socket_file())?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    stream.write_all(command.as_bytes())?;
    stream.shutdown(Shutdown::Write)?;
    let mut response = String::new();
    stream.take(MAX_MESSAGE).read_to_string(&mut response)?;
    Ok(response)
}

pub fn serve<F>(handler: F) -> io::Result<JoinHandle<()>>
where
    F: Fn(&str) -> String + Send + 'static,
{
    let path = paths::socket_file();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    // Only replace a leftover socket when nobody answers on it.
    if UnixStream::connect(&path).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("{} is in use", path.display()),
        ));
    }
    let _ = fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    // Like the Windows pipe: any local user may read the status and change display settings.
    fs::set_permissions(&path, Permissions::from_mode(0o666))?;

    Ok(thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => handle_client(stream, &handler),
                Err(e) => crate::warn!("IPC: accept failed: {e}"),
            }
        }
    }))
}

fn handle_client(mut stream: UnixStream, handler: &impl Fn(&str) -> String) {
    let _ = stream.set_read_timeout(Some(TIMEOUT));
    let _ = stream.set_write_timeout(Some(TIMEOUT));
    let mut request = String::new();
    if (&stream)
        .take(MAX_MESSAGE)
        .read_to_string(&mut request)
        .is_ok()
    {
        let response = handler(request.trim());
        let _ = stream.write_all(response.as_bytes());
    }
}
