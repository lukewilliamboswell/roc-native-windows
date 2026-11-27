//! The published executable grants exact HTTP origins. The same parsed
//! target is used for policy decisions and WinHTTP connection parameters.

use crate::app_resource::embedded_app_data;
use std::collections::BTreeSet;
use std::io;
use std::net::Ipv4Addr;
use std::sync::Arc;

const POLICY_RESOURCE: u16 = 503;
const POLICY_VERSION: &str = "roc-windows-capabilities 1";
const MAX_URL_BYTES: usize = 16 * 1024;

#[derive(Clone, Default)]
pub(super) struct Policy(Arc<BTreeSet<Origin>>);

impl Policy {
    pub(super) fn load() -> crate::Result<Self> {
        match embedded_app_data(POLICY_RESOURCE, 64 * 1024)? {
            Some(bytes) => Self::parse(bytes).map_err(|error| error.to_string()),
            None => Ok(Self::default()),
        }
    }

    pub(super) fn parse(bytes: &[u8]) -> io::Result<Self> {
        let text = std::str::from_utf8(bytes).map_err(|_| invalid("invalid capability UTF-8"))?;
        let mut lines = text.split('\n');
        if lines.next() != Some(POLICY_VERSION) {
            return Err(invalid("unsupported embedded capability version"));
        }
        let mut origins = BTreeSet::new();
        for line in lines {
            let target = Target::parse(line)?;
            if line != target.origin.absolute()
                || origins.len() == 64
                || !origins.insert(target.origin)
            {
                return Err(invalid("invalid or duplicate embedded HTTP origin"));
            }
        }
        Ok(Self(Arc::new(origins)))
    }

    pub(super) fn admits(&self, target: &Target) -> bool {
        self.0.contains(&target.origin)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct Origin {
    pub(super) secure: bool,
    pub(super) host: String,
    pub(super) port: u16,
}

impl Origin {
    fn absolute(&self) -> String {
        let scheme = if self.secure { "https" } else { "http" };
        format!("{scheme}://{}:{}", self.host, self.port)
    }
}

pub(super) struct Target {
    pub(super) origin: Origin,
    pub(super) path: String,
}

impl Target {
    pub(super) fn parse(url: &str) -> io::Result<Self> {
        if url.len() > MAX_URL_BYTES
            || url
                .bytes()
                .any(|byte| byte <= b' ' || byte == 127 || byte == b'\\')
        {
            return Err(invalid("invalid HTTP URL"));
        }
        let (scheme, suffix) = url
            .split_once("://")
            .ok_or_else(|| invalid("expected an absolute HTTP or HTTPS URL"))?;
        let (secure, default_port) = if scheme.eq_ignore_ascii_case("https") {
            (true, 443)
        } else if scheme.eq_ignore_ascii_case("http") {
            (false, 80)
        } else {
            return Err(invalid("expected an absolute HTTP or HTTPS URL"));
        };
        let end = suffix.find(['/', '?', '#']).unwrap_or(suffix.len());
        let authority = &suffix[..end];
        let (host, port) = match authority.split_once(':') {
            Some((host, port)) if port.bytes().all(|byte| byte.is_ascii_digit()) => (
                host,
                port.parse::<u16>()
                    .map_err(|_| invalid("invalid HTTP port"))?,
            ),
            Some(_) => return Err(invalid("invalid HTTP authority")),
            None => (authority, default_port),
        };
        let host = host.to_ascii_lowercase();
        if port == 0 || !valid_host(&host) {
            return Err(invalid("expected a valid ASCII DNS name or IPv4 address"));
        }
        let path = suffix[end..].split('#').next().unwrap_or_default();
        let path = if path.starts_with('/') {
            path.to_owned()
        } else {
            format!("/{path}")
        };
        Ok(Self {
            origin: Origin { secure, host, port },
            path,
        })
    }

    pub(super) fn absolute(&self) -> String {
        format!("{}{}", self.origin.absolute(), self.path)
    }
}

fn valid_host(host: &str) -> bool {
    if host
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return host.parse::<Ipv4Addr>().is_ok();
    }
    host.len() <= 253
        && host.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admits_only_the_published_scheme_host_and_effective_port() {
        let policy = Policy::parse(b"roc-windows-capabilities 1\nhttps://example.com:443").unwrap();
        assert!(policy.admits(&Target::parse("HTTPS://EXAMPLE.com/path?query").unwrap()));
        for denied in [
            "https://example.com:444/path",
            "http://example.com/path",
            "https://example.com.example.test",
        ] {
            assert!(!policy.admits(&Target::parse(denied).unwrap()));
        }
        assert!(!Policy::default().admits(&Target::parse("https://example.com").unwrap()));
        for invalid in [
            "https://example.com@attacker.test",
            "https://@example.com",
            "https://example.com\\attacker.test",
            "http://127.1",
        ] {
            assert!(Target::parse(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn embedded_policy_is_versioned_and_canonical() {
        assert!(Policy::parse(b"roc-windows-capabilities 2").is_err());
        assert!(Policy::parse(b"roc-windows-capabilities 1\nhttps://example.com").is_err());
        assert!(
            Policy::parse(
                b"roc-windows-capabilities 1\nhttps://example.com:443\nhttps://example.com:443"
            )
            .is_err()
        );
        let target = Target::parse("https://Example.com?hello#ignored").unwrap();
        assert_eq!(target.absolute(), "https://example.com:443/?hello");
    }
}
