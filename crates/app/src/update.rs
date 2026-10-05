//! "Check for updates": asks GitHub for the latest release. Runs only when the user asks, on a
//! background thread, and sends nothing but the request itself.

use std::{io, ptr};

use coolercast_core::win::wide;
use windows_sys::Win32::Networking::WinHttp::{
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect,
    WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
    WinHttpSendRequest, WinHttpSetTimeouts,
};

/// Where people download new versions.
pub const RELEASES_URL: &str = "https://github.com/moicd/coolercast/releases/latest";

const API_HOST: &str = "api.github.com";
const API_PATH: &str = "/repos/moicd/coolercast/releases/latest";
const TIMEOUT_MS: i32 = 10_000;
/// The release JSON is a few kilobytes; stop reading well before anything unreasonable.
const MAX_RESPONSE: usize = 1 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Check {
    UpToDate,
    Available { version: String, url: String },
    Failed(String),
}

/// Looks up the latest release. Blocks for up to a few seconds.
pub fn check() -> Check {
    match fetch_latest_release() {
        Ok(json) => evaluate(env!("CARGO_PKG_VERSION"), &json),
        Err(e) => Check::Failed(e.to_string()),
    }
}

/// Compares the running version with a GitHub release description.
pub fn evaluate(current: &str, release_json: &str) -> Check {
    let Some(tag) = json_string(release_json, "tag_name") else {
        return Check::Failed("unexpected answer from GitHub".into());
    };
    match (parse_version(&tag), parse_version(current)) {
        (Some(latest), Some(running)) if latest > running => Check::Available {
            version: tag.trim_start_matches('v').to_owned(),
            url: json_string(release_json, "html_url").unwrap_or_else(|| RELEASES_URL.into()),
        },
        (Some(_), Some(_)) => Check::UpToDate,
        _ => Check::Failed(format!("unexpected version '{tag}'")),
    }
}

/// `"v1.2.3"` or `"1.2.3"` (a pre-release suffix is ignored) as comparable numbers.
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((
        parts.next()??,
        parts.next()??,
        parts.next().unwrap_or(Some(0))?,
    ))
}

/// The value of the first `"key": "value"` pair in a JSON text. Enough for the two plain
/// ASCII fields CoolerCast reads, without a JSON parser.
fn json_string(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let after_key = &json[json.find(&needle)? + needle.len()..];
    let after_colon = after_key.trim_start().strip_prefix(':')?.trim_start();
    let body = after_colon.strip_prefix('"')?;
    let mut value = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(value),
            '\\' => value.push(chars.next()?),
            c => value.push(c),
        }
    }
    None
}

struct Internet(*mut core::ffi::c_void);

impl Internet {
    fn new(raw: *mut core::ffi::c_void) -> io::Result<Self> {
        if raw.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(raw))
        }
    }
}

impl Drop for Internet {
    fn drop(&mut self) {
        unsafe { WinHttpCloseHandle(self.0) };
    }
}

fn check_ok(ok: windows_sys::core::BOOL) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn fetch_latest_release() -> io::Result<String> {
    let agent = wide(format!("CoolerCast/{}", env!("CARGO_PKG_VERSION")));
    let session = Internet::new(unsafe {
        WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            ptr::null(),
            ptr::null(),
            0,
        )
    })?;
    check_ok(unsafe {
        WinHttpSetTimeouts(session.0, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS, TIMEOUT_MS)
    })?;
    let host = wide(API_HOST);
    let connection = Internet::new(unsafe {
        WinHttpConnect(session.0, host.as_ptr(), INTERNET_DEFAULT_HTTPS_PORT, 0)
    })?;
    let (verb, path) = (wide("GET"), wide(API_PATH));
    let request = Internet::new(unsafe {
        WinHttpOpenRequest(
            connection.0,
            verb.as_ptr(),
            path.as_ptr(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            WINHTTP_FLAG_SECURE,
        )
    })?;
    let headers = wide("Accept: application/vnd.github+json\r\n");
    check_ok(unsafe {
        WinHttpSendRequest(
            request.0,
            headers.as_ptr(),
            u32::MAX, // null-terminated
            ptr::null(),
            0,
            0,
            0,
        )
    })?;
    check_ok(unsafe { WinHttpReceiveResponse(request.0, ptr::null_mut()) })?;

    let mut status = 0u32;
    let mut size = size_of::<u32>() as u32;
    check_ok(unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            ptr::null(),
            (&mut status as *mut u32).cast(),
            &mut size,
            ptr::null_mut(),
        )
    })?;
    if status != 200 {
        return Err(io::Error::other(format!(
            "GitHub answered with HTTP {status}"
        )));
    }

    let mut body = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let mut read = 0u32;
        check_ok(unsafe {
            WinHttpReadData(
                request.0,
                chunk.as_mut_ptr().cast(),
                chunk.len() as u32,
                &mut read,
            )
        })?;
        if read == 0 || body.len() > MAX_RESPONSE {
            break;
        }
        body.extend_from_slice(&chunk[..read as usize]);
    }
    String::from_utf8(body).map_err(|_| io::Error::other("invalid answer from GitHub"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE: &str = r#"{"url":"https://api.github.com/x","html_url":
        "https://github.com/moicd/coolercast/releases/tag/v0.3.0","id":1,
        "tag_name" : "v0.3.0","name":"CoolerCast \"0.3.0\""}"#;

    #[test]
    fn newer_release_is_available() {
        assert_eq!(
            evaluate("0.2.0", RELEASE),
            Check::Available {
                version: "0.3.0".into(),
                url: "https://github.com/moicd/coolercast/releases/tag/v0.3.0".into(),
            }
        );
    }

    #[test]
    fn same_or_older_release_is_up_to_date() {
        assert_eq!(evaluate("0.3.0", RELEASE), Check::UpToDate);
        assert_eq!(evaluate("0.10.0", RELEASE), Check::UpToDate);
    }

    #[test]
    fn unexpected_answers_fail() {
        assert!(matches!(evaluate("0.2.0", "{}"), Check::Failed(_)));
        assert!(matches!(
            evaluate("0.2.0", r#"{"tag_name":"nightly"}"#),
            Check::Failed(_)
        ));
    }

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.10.0-beta.1"), Some((0, 10, 0)));
        assert_eq!(parse_version("2.1"), Some((2, 1, 0)));
        assert!(parse_version("0.10.0") > parse_version("0.9.9"));
        assert_eq!(parse_version("x.y"), None);
    }

    #[test]
    fn json_strings_with_escapes() {
        assert_eq!(
            json_string(RELEASE, "name").as_deref(),
            Some("CoolerCast \"0.3.0\"")
        );
        assert_eq!(json_string(RELEASE, "missing"), None);
        assert_eq!(json_string(r#"{"id": 3}"#, "id"), None);
    }
}
