//! The HTTP client of the download worker (SB-12): `GET` over a connection the tool opened to
//! a configured registry, with `Connection: close`, no redirects, and a limit on the size of
//! the response. Only `http://` URLs: Unio 0.1 has no TLS (a decision of 2026-11-12; TLS comes
//! with the networking RFC of 0.2, which defines its profiles, docs/cryptography.md). The
//! content of a package is bound by its ML-DSA-65 signature, its hash, and the signed index,
//! never by the transport (docs/cryptography.md, section 4); what is downloaded is not private.

use std::io::{BufRead, BufReader, Read, Write};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    pub host: String,
    pub port: u16,
    /// Starts with `/`.
    pub path: String,
}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
}

fn valid_path(path: &str) -> bool {
    path.starts_with('/')
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-~%+=@:".contains(&b))
        && !path.split('/').any(|p| p == ".." || p == ".")
}

impl Url {
    pub fn parse(text: &str) -> Result<Url, String> {
        if text.starts_with("https://") {
            return Err(format!(
                "{text}: HTTPS is not supported: Unio 0.1 has no TLS (it comes with the \
                 networking of 0.2); use an http:// registry, whose packages are installed only \
                 with a valid ML-DSA-65 signature and hash, whatever the transport"
            ));
        }
        let rest = text
            .strip_prefix("http://")
            .ok_or_else(|| format!("{text}: only http:// URLs are supported"))?;
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (
                host,
                port.parse::<u16>()
                    .map_err(|_| format!("{text}: the port is not a number"))?,
            ),
            None => (authority, 80),
        };
        let host = host.to_ascii_lowercase();
        if !valid_host(&host) {
            return Err(format!("{text}: the host is not a valid name"));
        }
        if !valid_path(path) {
            return Err(format!(
                "{text}: the path has characters this client does not send"
            ));
        }
        Ok(Url {
            host,
            port,
            path: path.to_string(),
        })
    }

    /// `http://host:port`, the origin the configuration allows.
    pub fn origin(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }

    /// The canonical text: origin and path, without a trailing `/` unless it is the path.
    pub fn canonical(&self) -> String {
        let path = if self.path.len() > 1 {
            self.path.trim_end_matches('/')
        } else {
            ""
        };
        format!("{}{path}", self.origin())
    }

    /// A relative path below this URL (a directory): no `..`, no absolute path, no scheme, so
    /// that an index cannot send the download to another host or outside the registry.
    pub fn join(&self, relative: &str) -> Result<Url, String> {
        if relative.starts_with('/') || relative.contains("://") || relative.is_empty() {
            return Err(format!(
                "{relative:?} is not a relative path of the registry"
            ));
        }
        let base = self.path.trim_end_matches('/');
        let path = format!("{base}/{relative}");
        if !valid_path(&path) {
            return Err(format!(
                "{relative:?} is not a relative path of the registry"
            ));
        }
        Ok(Url {
            host: self.host.clone(),
            port: self.port,
            path,
        })
    }
}

/// The largest header block of a response.
const MAX_HEADERS: usize = 64 * 1024;

/// Sends `GET` for the URL on the stream and returns the body of a `200` response, at most
/// `limit` bytes.
pub fn get<S: Read + Write>(stream: S, url: &Url, limit: u64) -> Result<Vec<u8>, String> {
    let mut reader = BufReader::new(stream);
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}:{}\r\nUser-Agent: unio\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        url.path, url.host, url.port
    );
    reader
        .get_mut()
        .write_all(request.as_bytes())
        .and_then(|()| reader.get_mut().flush())
        .map_err(|e| format!("sending the request: {e}"))?;
    let mut head = Vec::new();
    loop {
        let mut line = Vec::new();
        let n = (&mut reader)
            .take((MAX_HEADERS - head.len().min(MAX_HEADERS)) as u64 + 1)
            .read_until(b'\n', &mut line)
            .map_err(|e| format!("reading the response: {e}"))?;
        if n == 0 {
            return Err("the connection closed before the headers ended".into());
        }
        head.extend_from_slice(&line);
        if head.len() > MAX_HEADERS {
            return Err(format!("the headers are longer than {MAX_HEADERS} bytes"));
        }
        if line == b"\r\n" || line == b"\n" {
            break;
        }
    }
    let head = String::from_utf8(head).map_err(|_| "the headers are not text".to_string())?;
    let mut lines = head.lines();
    let status = lines.next().unwrap_or("");
    let mut parts = status.split_whitespace();
    let (Some(version), Some(code)) = (parts.next(), parts.next()) else {
        return Err(format!("not an HTTP response: {status:?}"));
    };
    if !version.starts_with("HTTP/1.") {
        return Err(format!("not an HTTP/1 response: {status:?}"));
    }
    match code {
        "200" => {}
        c if c.starts_with('3') => {
            return Err(format!(
                "the server answered {status:?}; redirects are not followed, so a registry \
                 cannot send a download to another host"
            ))
        }
        _ => return Err(format!("the server answered {status:?}")),
    }
    let mut length: Option<u64> = None;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            length = Some(
                value
                    .parse()
                    .map_err(|_| format!("a bad Content-Length {value:?}"))?,
            );
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            if value.eq_ignore_ascii_case("chunked") {
                chunked = true;
            } else {
                return Err(format!("the transfer encoding {value:?} is not supported"));
            }
        }
    }
    let too_large = || format!("the response is larger than the limit of {limit} bytes");
    let mut body = Vec::new();
    if chunked {
        loop {
            let mut size_line = String::new();
            (&mut reader)
                .take(1024)
                .read_line(&mut size_line)
                .map_err(|e| format!("reading a chunk: {e}"))?;
            let size_text = size_line.trim().split(';').next().unwrap_or("");
            let size = u64::from_str_radix(size_text, 16)
                .map_err(|_| format!("a bad chunk size {size_text:?}"))?;
            if size == 0 {
                break;
            }
            if body.len() as u64 + size > limit {
                return Err(too_large());
            }
            let start = body.len();
            body.resize(start + size as usize, 0);
            reader
                .read_exact(&mut body[start..])
                .map_err(|e| format!("reading a chunk: {e}"))?;
            let mut crlf = [0u8; 2];
            reader
                .read_exact(&mut crlf)
                .map_err(|e| format!("reading a chunk: {e}"))?;
        }
    } else if let Some(length) = length {
        if length > limit {
            return Err(too_large());
        }
        body.resize(length as usize, 0);
        reader
            .read_exact(&mut body)
            .map_err(|e| format!("the body ended early: {e}"))?;
    } else {
        (&mut reader)
            .take(limit + 1)
            .read_to_end(&mut body)
            .map_err(|e| format!("reading the body: {e}"))?;
        if body.len() as u64 > limit {
            return Err(too_large());
        }
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        input: std::io::Cursor<Vec<u8>>,
        sent: Vec<u8>,
    }

    impl Read for Fake {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for Fake {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.sent.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn fake(response: &str) -> Fake {
        Fake {
            input: std::io::Cursor::new(response.as_bytes().to_vec()),
            sent: Vec::new(),
        }
    }

    #[test]
    fn urls_parse_and_join_only_below_the_registry() {
        let url = Url::parse("http://Example.org:8080/reg/").unwrap();
        assert_eq!(url.canonical(), "http://example.org:8080/reg");
        assert_eq!(
            url.join("index/a/b.json").unwrap().path,
            "/reg/index/a/b.json"
        );
        for bad in ["../x", "/etc", "http://evil/x", "a/../../x", "a b"] {
            assert!(url.join(bad).is_err(), "{bad}");
        }
        assert!(Url::parse("https://example.org/")
            .unwrap_err()
            .contains("HTTPS"));
        assert!(Url::parse("ftp://example.org/").is_err());
        assert!(Url::parse("http://exa mple/").is_err());
    }

    #[test]
    fn responses_are_read_within_the_limit() {
        let url = Url::parse("http://h/x").unwrap();
        let body = get(
            fake("HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello"),
            &url,
            10,
        );
        assert_eq!(body.unwrap(), b"hello");
        let body = get(
            fake("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nhel\r\n2\r\nlo\r\n0\r\n\r\n"),
            &url,
            10,
        );
        assert_eq!(body.unwrap(), b"hello");
        let body = get(fake("HTTP/1.0 200 OK\r\n\r\nhello"), &url, 10);
        assert_eq!(body.unwrap(), b"hello");
        assert!(get(
            fake("HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\n"),
            &url,
            10
        )
        .unwrap_err()
        .contains("limit"));
        assert!(get(fake("HTTP/1.0 200 OK\r\n\r\nhello world"), &url, 5).is_err());
        assert!(get(
            fake("HTTP/1.1 302 Found\r\nLocation: http://evil/\r\n\r\n"),
            &url,
            10
        )
        .unwrap_err()
        .contains("redirects"));
        assert!(get(fake("HTTP/1.1 404 Not Found\r\n\r\n"), &url, 10).is_err());
    }
}
