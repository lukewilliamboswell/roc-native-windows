use super::{MAX_TIMEOUT, Result, put_string, tagged};
use crate::wire::Reader;
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::io;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use windows_sys::Win32::Networking::WinHttp::*;

const MAX_BODY: usize = 16 << 20;
const MAX_HEADERS: usize = 256 << 10;

pub(super) struct Request {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: String,
    pub(super) timeout: u32,
    cache: u8,
    credentials: u8,
    progress: Duration,
}

impl Request {
    pub(super) fn read(reader: &mut Reader<'_>) -> Result<Self> {
        let method = reader.string()?.to_owned();
        let url = reader.string()?.to_owned();
        let mut headers = Vec::new();
        for _ in 0..reader.u32()? {
            headers.push((reader.string()?.to_owned(), reader.string()?.to_owned()));
        }
        let body = reader.string()?.to_owned();
        let timeout = reader.u32()?.min(MAX_TIMEOUT);
        let cache = reader.u8()?;
        let credentials = reader.u8()?;
        let progress_ms = reader.u32()?;
        if !(1..=5).contains(&cache) || !(1..=2).contains(&credentials) {
            return Err("invalid HTTP cache or credentials policy".into());
        }
        Ok(Self {
            method,
            url,
            headers,
            body,
            timeout,
            cache,
            credentials,
            progress: Duration::from_millis(u64::from(progress_ms)),
        })
    }

    pub(super) fn spec_request(&self) -> crate::spec_effects::Request {
        crate::spec_effects::Request {
            method: self.method.clone(),
            url: self.url.clone(),
            body: self.body.clone(),
            timeout: self.timeout,
        }
    }

    pub(super) fn run(self, stop: &AtomicBool, report: &dyn Fn(Vec<u8>)) -> Vec<u8> {
        match self.fetch(stop, report) {
            Ok(payload) => payload,
            Err(error) => failure(&error.to_string()),
        }
    }

    fn fetch(&self, stop: &AtomicBool, report: &dyn Fn(Vec<u8>)) -> io::Result<Vec<u8>> {
        let url = wide(&self.url)?;
        let method = wide(&self.method)?;
        let mut parts = URL_COMPONENTS {
            dwStructSize: std::mem::size_of::<URL_COMPONENTS>() as u32,
            dwHostNameLength: u32::MAX,
            dwUrlPathLength: u32::MAX,
            dwExtraInfoLength: u32::MAX,
            dwUserNameLength: u32::MAX,
            dwPasswordLength: u32::MAX,
            ..Default::default()
        };
        // WinHTTP points the components into `url` when given lengths and no storage.
        unsafe {
            check(WinHttpCrackUrl(url.as_ptr(), 0, 0, &mut parts))?;
        }
        let secure = match parts.nScheme {
            WINHTTP_INTERNET_SCHEME_HTTP => 0,
            WINHTTP_INTERNET_SCHEME_HTTPS => WINHTTP_FLAG_SECURE,
            _ => return Err(invalid("expected an absolute HTTP or HTTPS URL")),
        };
        if parts.dwHostNameLength == 0 || parts.dwUserNameLength != 0 || parts.dwPasswordLength != 0
        {
            return Err(invalid("HTTP URL has no host or contains credentials"));
        }
        let mut hostname = component(parts.lpszHostName, parts.dwHostNameLength).to_vec();
        hostname.push(0);
        let mut path = component(parts.lpszUrlPath, parts.dwUrlPathLength).to_vec();
        if path.is_empty() {
            path.push(u16::from(b'/'));
        }
        path.extend(component(parts.lpszExtraInfo, parts.dwExtraInfoLength));
        if let Some(fragment) = path.iter().position(|&unit| unit == u16::from(b'#')) {
            path.truncate(fragment);
        }
        path.push(0);

        // Each synchronous handle is used by this thread only. Drop closes it.
        unsafe {
            let session = Handle::new(WinHttpOpen(
                wide("roc-native-windows")?.as_ptr(),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                null(),
                null(),
                0,
            ))?;
            let timeout = if self.timeout == 0 {
                MAX_TIMEOUT
            } else {
                self.timeout
            } as i32;
            check(WinHttpSetTimeouts(
                session.0, timeout, timeout, timeout, timeout,
            ))?;
            let connection =
                Handle::new(WinHttpConnect(session.0, hostname.as_ptr(), parts.nPort, 0))?;
            let refresh = if (2..=4).contains(&self.cache) {
                WINHTTP_FLAG_REFRESH
            } else {
                0
            };
            let request = Handle::new(WinHttpOpenRequest(
                connection.0,
                method.as_ptr(),
                path.as_ptr(),
                null(),
                null(),
                null(),
                secure | refresh,
            ))?;
            if self.credentials == 1 {
                set_option(
                    &request,
                    WINHTTP_OPTION_DISABLE_FEATURE,
                    WINHTTP_DISABLE_COOKIES,
                )?;
            }
            set_option(&request, WINHTTP_OPTION_MAX_HTTP_AUTOMATIC_REDIRECTS, 20)?;
            let decompress = match set_option(
                &request,
                WINHTTP_OPTION_DECOMPRESSION,
                WINHTTP_DECOMPRESSION_FLAG_GZIP | WINHTTP_DECOMPRESSION_FLAG_DEFLATE,
            ) {
                Ok(()) => true,
                // A WinHTTP implementation without this option can still
                // deliver responses when we request the identity encoding.
                Err(error) if error.raw_os_error() == Some(ERROR_WINHTTP_INVALID_OPTION as i32) => {
                    false
                }
                Err(error) => return Err(error),
            };
            let mut text_headers = String::new();
            for (name, value) in &self.headers {
                if name.contains(['\r', '\n', ':']) || value.contains(['\r', '\n']) {
                    return Err(invalid("invalid HTTP header"));
                }
                if !decompress && name.eq_ignore_ascii_case("accept-encoding") {
                    continue;
                }
                text_headers.push_str(name);
                text_headers.push_str(": ");
                text_headers.push_str(value);
                text_headers.push_str("\r\n");
            }
            if !decompress {
                text_headers.push_str("Accept-Encoding: identity\r\n");
            }
            let headers = wide(&text_headers)?;
            let body = if matches!(self.method.as_str(), "GET" | "HEAD") {
                &[]
            } else {
                self.body.as_bytes()
            };
            let size =
                u32::try_from(body.len()).map_err(|_| invalid("HTTP request body is too large"))?;
            let data = if body.is_empty() {
                null()
            } else {
                body.as_ptr().cast()
            };
            check(WinHttpSendRequest(
                request.0,
                headers.as_ptr(),
                (headers.len() - 1) as u32,
                data,
                size,
                size,
                0,
            ))?;
            if !self.progress.is_zero() && size != 0 {
                report(encode_progress(1, u64::from(size), Some(u64::from(size))));
            }
            check(WinHttpReceiveResponse(request.0, null_mut()))?;
            let mut status = 0u32;
            let mut status_len = std::mem::size_of::<u32>() as u32;
            check(WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                null(),
                (&mut status as *mut u32).cast(),
                &mut status_len,
                null_mut(),
            ))?;
            let headers = response_headers(&request)?;
            if !decompress
                && headers.lines().skip(1).any(|line| {
                    line.split_once(':').is_some_and(|(name, value)| {
                        name.eq_ignore_ascii_case("content-encoding")
                            && !value.trim().eq_ignore_ascii_case("identity")
                    })
                })
            {
                return Err(invalid(
                    "server sent an unsupported compressed HTTP response",
                ));
            }
            let mut last_report = Instant::now();
            let mut body = Vec::new();
            let mut buffer = [0u8; 16 << 10];
            loop {
                if stop.load(Ordering::Relaxed) {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "HTTP request cancelled",
                    ));
                }
                let mut count = 0u32;
                check(WinHttpReadData(
                    request.0,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                    &mut count,
                ))?;
                if count == 0 {
                    break;
                }
                if body.len() + count as usize > MAX_BODY {
                    return Err(invalid("HTTP response exceeds 16 MiB"));
                }
                body.extend(&buffer[..count as usize]);
                if !self.progress.is_zero() && last_report.elapsed() >= self.progress {
                    report(encode_progress(2, body.len() as u64, None));
                    last_report = Instant::now();
                }
            }
            if !self.progress.is_zero() {
                report(encode_progress(
                    2,
                    body.len() as u64,
                    Some(body.len() as u64),
                ));
            }
            Ok(encode_response(
                status as u16,
                headers
                    .lines()
                    .skip(1)
                    .filter_map(|line| line.split_once(':')),
                &body,
            ))
        }
    }
}

fn encode_progress(direction: u8, received: u64, total: Option<u64>) -> Vec<u8> {
    let mut payload = vec![26, direction];
    payload.extend(received.to_le_bytes());
    payload.push(u8::from(total.is_some()));
    payload.extend(total.unwrap_or_default().to_le_bytes());
    payload
}

struct Handle(*mut c_void);
impl Handle {
    fn new(handle: *mut c_void) -> io::Result<Self> {
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            WinHttpCloseHandle(self.0);
        }
    }
}

fn response_headers(request: &Handle) -> io::Result<String> {
    let mut bytes = 0;
    // Query the required buffer size, then fill a UTF-16 buffer of that size.
    unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_RAW_HEADERS_CRLF,
            null(),
            null_mut(),
            &mut bytes,
            null_mut(),
        );
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        if bytes as usize > MAX_HEADERS {
            return Err(invalid("HTTP response headers exceed 256 KiB"));
        }
        let mut units = vec![0u16; (bytes as usize).div_ceil(2)];
        check(WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_RAW_HEADERS_CRLF,
            null(),
            units.as_mut_ptr().cast(),
            &mut bytes,
            null_mut(),
        ))?;
        Ok(String::from_utf16_lossy(&units[..bytes as usize / 2]))
    }
}

fn set_option(request: &Handle, option: u32, value: u32) -> io::Result<()> {
    unsafe {
        check(WinHttpSetOption(
            request.0,
            option,
            (&value as *const u32).cast(),
            4,
        ))
    }
}

fn component<'a>(ptr: *const u16, length: u32) -> &'a [u16] {
    if length == 0 {
        return &[];
    }
    // The caller keeps the URL from WinHttpCrackUrl alive while copying this slice.
    unsafe { std::slice::from_raw_parts(ptr, length as usize) }
}

fn check(result: i32) -> io::Result<()> {
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn wide(text: &str) -> io::Result<Vec<u16>> {
    if text.contains('\0') {
        return Err(invalid("unexpected NUL in HTTP request"));
    }
    Ok(text.encode_utf16().chain([0]).collect())
}
pub(super) fn failure(message: &str) -> Vec<u8> {
    tagged(2, message.as_bytes())
}

pub(super) fn encode_response<'a>(
    status: u16,
    headers: impl IntoIterator<Item = (&'a str, &'a str)>,
    body: &[u8],
) -> Vec<u8> {
    let mut fields = BTreeMap::<String, String>::new();
    for (name, value) in headers {
        let value = value.trim();
        fields
            .entry(name.to_ascii_lowercase())
            .and_modify(|current| {
                current.push_str(", ");
                current.push_str(value);
            })
            .or_insert_with(|| value.to_owned());
    }
    let mut payload = vec![1];
    payload.extend(status.to_le_bytes());
    payload.extend((fields.len() as u32).to_le_bytes());
    for (name, value) in fields {
        put_string(&mut payload, &name);
        put_string(&mut payload, &value);
    }
    payload.extend(
        String::from_utf8_lossy(body.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(body)).as_bytes(),
    );
    payload
}
