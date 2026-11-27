use super::http_policy::{Policy, Target};
use super::{MAX_TIMEOUT, Result, put_string, tagged};
use crate::wire::Reader;
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::io;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use windows_sys::Win32::Networking::WinHttp::*;
use windows_sys::Win32::UI::Shell::UrlCombineW;

const MAX_BODY: usize = 16 << 20;
const MAX_HEADERS: usize = 256 << 10;
const MAX_REDIRECTS: usize = 20;

enum Hop {
    Response(Vec<u8>),
    Redirect(u16, Target),
    Denied,
}

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

    pub(super) fn target(&self) -> io::Result<Target> {
        for (name, value) in &self.headers {
            if name.is_empty()
                || !name.bytes().all(http_token)
                || name.eq_ignore_ascii_case("host")
                || name.eq_ignore_ascii_case("proxy-authorization")
                || value.contains(['\r', '\n', '\0'])
            {
                return Err(invalid("invalid or restricted HTTP header"));
            }
        }
        Target::parse(&self.url)
    }

    pub(super) fn run(
        self,
        target: Target,
        policy: &Policy,
        stop: &AtomicBool,
        report: &dyn Fn(Vec<u8>),
    ) -> Vec<u8> {
        match self.fetch(target, policy, stop, report) {
            Ok(payload) => payload,
            Err(error)
                if error.kind() == io::ErrorKind::TimedOut
                    || error.raw_os_error() == Some(ERROR_WINHTTP_TIMEOUT as i32) =>
            {
                vec![7]
            }
            Err(error) => failure(&error.to_string()),
        }
    }

    fn fetch(
        &self,
        mut target: Target,
        policy: &Policy,
        stop: &AtomicBool,
        report: &dyn Fn(Vec<u8>),
    ) -> io::Result<Vec<u8>> {
        if !policy.admits(&target) {
            return Ok(policy_denied());
        }
        let timeout = if self.timeout == 0 {
            MAX_TIMEOUT
        } else {
            self.timeout
        };
        let deadline = Instant::now() + Duration::from_millis(u64::from(timeout));
        // SAFETY: The session stays on this worker and owns its native handle.
        let session = unsafe {
            Handle::new(WinHttpOpen(
                wide("roc-native-windows")?.as_ptr(),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                null(),
                null(),
                0,
            ))?
        };
        let mut method = self.method.as_str();
        for redirects in 0..=MAX_REDIRECTS {
            match self.hop(&session, &target, method, deadline, policy, stop, report)? {
                Hop::Response(payload) => return Ok(payload),
                Hop::Denied => return Ok(policy_denied()),
                Hop::Redirect(_, _) if redirects == MAX_REDIRECTS => {
                    return Err(invalid("too many HTTP redirects"));
                }
                Hop::Redirect(status, next) => {
                    method = redirect_method(status, method);
                    target = next;
                }
            }
        }
        unreachable!("the redirect loop stops on its last hop")
    }

    #[allow(clippy::too_many_arguments)] // Each hop shares the parent's deadline and worker.
    fn hop(
        &self,
        session: &Handle,
        target: &Target,
        method: &str,
        deadline: Instant,
        policy: &Policy,
        stop: &AtomicBool,
        report: &dyn Fn(Vec<u8>),
    ) -> io::Result<Hop> {
        let hostname = wide(&target.origin.host)?;
        let path = wide(&target.path)?;
        let wire_method = wide(method)?;
        let secure = if target.origin.secure {
            WINHTTP_FLAG_SECURE
        } else {
            0
        };
        // SAFETY: Handles and UTF-16 buffers belong to this worker and stay live.
        unsafe {
            let timeout = remaining(deadline, stop)?;
            check(WinHttpSetTimeouts(
                session.0, timeout, timeout, timeout, timeout,
            ))?;
            let connection = Handle::new(WinHttpConnect(
                session.0,
                hostname.as_ptr(),
                target.origin.port,
                0,
            ))?;
            let refresh = if (2..=4).contains(&self.cache) {
                WINHTTP_FLAG_REFRESH
            } else {
                0
            };
            let request = Handle::new(WinHttpOpenRequest(
                connection.0,
                wire_method.as_ptr(),
                path.as_ptr(),
                null(),
                null(),
                null(),
                secure | refresh,
            ))?;
            let mut disabled = WINHTTP_DISABLE_REDIRECTS | WINHTTP_DISABLE_AUTHENTICATION;
            if self.credentials == 1 {
                disabled |= WINHTTP_DISABLE_COOKIES;
            }
            set_option(&request, WINHTTP_OPTION_DISABLE_FEATURE, disabled)?;
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
                if !decompress && name.eq_ignore_ascii_case("accept-encoding") {
                    continue;
                }
                if method != self.method && name.eq_ignore_ascii_case("content-length") {
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
            let body = if matches!(method, "GET" | "HEAD") {
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
            remaining(deadline, stop)?;
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
            if let Some(next) = redirect_target(status as u16, target, &headers)? {
                if !policy.admits(&next) {
                    return Ok(Hop::Denied);
                }
                if next.origin == target.origin {
                    return Ok(Hop::Redirect(status as u16, next));
                }
            }
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
                remaining(deadline, stop)?;
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
            Ok(Hop::Response(encode_response(
                status as u16,
                headers
                    .lines()
                    .skip(1)
                    .filter_map(|line| line.split_once(':')),
                &body,
            )))
        }
    }
}

fn redirect_target(status: u16, current: &Target, headers: &str) -> io::Result<Option<Target>> {
    if !matches!(status, 301 | 302 | 303 | 307 | 308) {
        return Ok(None);
    }
    let mut locations = headers.lines().skip(1).filter_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case("location").then(|| value.trim())
    });
    let Some(location) = locations.next() else {
        return Ok(None);
    };
    if location.is_empty()
        || location.len() > 16 * 1024
        || location
            .bytes()
            .any(|byte| byte <= b' ' || byte == 127 || byte == b'\\')
        || locations.next().is_some()
    {
        return Err(invalid("invalid or ambiguous HTTP redirect"));
    }
    let original = wide(&current.absolute())?;
    let location = wide(location)?;
    let mut combined = vec![0_u16; 16 * 1024 + 1];
    let mut len = combined.len() as u32;
    // SAFETY: Both inputs are terminated and the function writes into our bounded buffer.
    let result = unsafe {
        UrlCombineW(
            original.as_ptr(),
            location.as_ptr(),
            combined.as_mut_ptr(),
            &mut len,
            0,
        )
    };
    if result < 0 || len as usize >= combined.len() {
        return Err(invalid("could not resolve HTTP redirect"));
    }
    let url = String::from_utf16(&combined[..len as usize])
        .map_err(|_| invalid("redirect URL is not valid UTF-16"))?;
    Target::parse(&url).map(Some)
}

fn redirect_method(status: u16, method: &str) -> &str {
    match (status, method) {
        (301 | 302, "POST") => "GET",
        (303, method) if method != "HEAD" => "GET",
        _ => method,
    }
}

fn remaining(deadline: Instant, stop: &AtomicBool) -> io::Result<i32> {
    if stop.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "HTTP request cancelled",
        ));
    }
    let now = Instant::now();
    if now >= deadline {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "HTTP request timed out",
        ));
    }
    Ok((deadline - now).as_millis().max(1) as i32)
}

fn http_token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
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

pub(super) fn policy_denied() -> Vec<u8> {
    // TODO return the dedicated HTTP denial when the pinned Signals codec supports it.
    // https://gist.github.com/lukewilliamboswell/92c68a01222f8d36c1ddc893762d481d
    failure("HTTP origin is not allowed by this application's capabilities")
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::thread;

    fn serve(
        listener: TcpListener,
        exchanges: Vec<(&'static str, String)>,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            for (path, response) in exchanges {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut socket = loop {
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(error)
                            if error.kind() == io::ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("waiting for {path}: {error}"),
                    }
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(4)))
                    .unwrap();
                let mut line = String::new();
                let mut request = BufReader::new(&socket);
                request.read_line(&mut line).unwrap();
                assert_eq!(line.trim_end(), format!("GET {path} HTTP/1.1"));
                loop {
                    line.clear();
                    let size = request.read_line(&mut line).unwrap();
                    if size == 0 || line == "\r\n" {
                        break;
                    }
                }
                socket.write_all(response.as_bytes()).unwrap();
            }
        })
    }

    fn redirect(location: &str) -> String {
        format!(
            "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
    }

    fn get(origin: &str, policy: &Policy) -> Vec<u8> {
        let request = Request {
            method: "GET".into(),
            url: format!("{origin}/start"),
            headers: Vec::new(),
            body: String::new(),
            timeout: 4000,
            cache: 1,
            credentials: 1,
            progress: Duration::ZERO,
        };
        let target = request.target().unwrap();
        request.run(target, policy, &AtomicBool::new(false), &|_| {})
    }

    #[test]
    fn follows_relative_redirect_within_the_granted_origin() {
        let host = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let origin = format!("http://{}", host.local_addr().unwrap());
        let policy =
            Policy::parse(format!("roc-windows-capabilities 1\n{origin}").as_bytes()).unwrap();
        let server = serve(
            host,
            vec![
                ("/start", redirect("next?part=1")),
                (
                    "/next?part=1",
                    "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".into(),
                ),
            ],
        );
        let result = get(&origin, &policy);
        assert!(
            result.starts_with(&[1, 200, 0]) && result.ends_with(b"ok"),
            "{result:?}"
        );
        server.join().unwrap();
    }

    #[test]
    fn cross_origin_redirect_never_connects_even_if_the_other_origin_is_granted() {
        let second = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        second.set_nonblocking(true).unwrap();
        let other = format!("http://{}", second.local_addr().unwrap());
        for allowed in [false, true] {
            let first = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let origin = format!("http://{}", first.local_addr().unwrap());
            let mut policy = format!("roc-windows-capabilities 1\n{origin}");
            if allowed {
                policy.push_str(&format!("\n{other}"));
            }
            let policy = Policy::parse(policy.as_bytes()).unwrap();
            let server = serve(first, vec![("/start", redirect(&format!("{other}/other")))]);
            let result = get(&origin, &policy);
            if allowed {
                assert!(result.starts_with(&[1, 46, 1]), "{result:?}");
            } else {
                assert_eq!(result, policy_denied());
            }
            server.join().unwrap();
            assert!(
                matches!(second.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock)
            );
        }
    }
}
