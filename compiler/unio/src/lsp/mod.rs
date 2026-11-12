//! The language server of `unio lsp` (T-10, SB-13; docs/runtime.md 9.7, item 7).
//!
//! The server process (`server.rs`, part of the `unio` command, since it starts workers) only
//! speaks the protocol and holds the open documents. Every analysis runs in a worker of the
//! tools with the `[compile]` limits: the server sends the worker a [`Request`] with the unsaved
//! buffers of the document's package on its standard input, and the worker answers with the
//! result as JSON on its standard output ([`analysis::answer`]). This module is the part both
//! sides share: the request, positions of LSP, and the paths of `file:` URIs.

pub mod analysis;
pub mod facts;
pub mod json;

use std::path::{Path, PathBuf};

use crate::Lang;

/// What the server asks a worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Diagnostics,
    Hover,
    Definition,
    Completion,
    Formatting,
    Rename,
    /// The types of `let` bindings written without one.
    InlayHints,
}

impl Kind {
    fn code(self) -> u8 {
        match self {
            Kind::Diagnostics => 0,
            Kind::Hover => 1,
            Kind::Definition => 2,
            Kind::Completion => 3,
            Kind::Formatting => 4,
            Kind::Rename => 5,
            Kind::InlayHints => 6,
        }
    }

    fn from_code(code: u8) -> Option<Kind> {
        Some(match code {
            0 => Kind::Diagnostics,
            1 => Kind::Hover,
            2 => Kind::Definition,
            3 => Kind::Completion,
            4 => Kind::Formatting,
            5 => Kind::Rename,
            6 => Kind::InlayHints,
            _ => return None,
        })
    }
}

/// One analysis: the document, the position in it (LSP: line and UTF-16 unit, from 0), the new
/// name of a rename, and the unsaved texts of the open documents of its package, by the paths
/// of their URIs.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub kind: Kind,
    pub path: PathBuf,
    pub line: u32,
    pub character: u32,
    pub new_name: String,
    pub lang: Lang,
    pub buffers: Vec<(PathBuf, String)>,
}

/// The version of the encoding of a request, so that a worker of another build refuses it.
const REQUEST_VERSION: u32 = 1;

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    put_u32(out, bytes.len() as u32);
    out.extend_from_slice(bytes);
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn u32(&mut self) -> Option<u32> {
        let end = self.at.checked_add(4)?;
        let v = u32::from_le_bytes(self.data.get(self.at..end)?.try_into().ok()?);
        self.at = end;
        Some(v)
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.u32()? as usize;
        let end = self.at.checked_add(n)?;
        let v = self.data.get(self.at..end)?;
        self.at = end;
        Some(v)
    }

    fn text(&mut self) -> Option<String> {
        String::from_utf8(self.bytes()?.to_vec()).ok()
    }

    fn path(&mut self) -> Option<PathBuf> {
        // Paths travel as UTF-8: they come from `file:` URIs, which are text.
        self.text().map(PathBuf::from)
    }
}

impl Request {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_u32(&mut out, REQUEST_VERSION);
        put_u32(&mut out, u32::from(self.kind.code()));
        put_bytes(&mut out, self.path.to_string_lossy().as_bytes());
        put_u32(&mut out, self.line);
        put_u32(&mut out, self.character);
        put_bytes(&mut out, self.new_name.as_bytes());
        put_u32(&mut out, u32::from(self.lang == Lang::Ru));
        put_u32(&mut out, self.buffers.len() as u32);
        for (path, text) in &self.buffers {
            put_bytes(&mut out, path.to_string_lossy().as_bytes());
            put_bytes(&mut out, text.as_bytes());
        }
        out
    }

    pub fn decode(data: &[u8]) -> Option<Request> {
        let mut r = Reader { data, at: 0 };
        if r.u32()? != REQUEST_VERSION {
            return None;
        }
        let kind = Kind::from_code(u8::try_from(r.u32()?).ok()?)?;
        let path = r.path()?;
        let line = r.u32()?;
        let character = r.u32()?;
        let new_name = r.text()?;
        let lang = if r.u32()? == 1 { Lang::Ru } else { Lang::En };
        let n = r.u32()?;
        let mut buffers = Vec::new();
        for _ in 0..n {
            buffers.push((r.path()?, r.text()?));
        }
        (r.at == data.len()).then_some(Request {
            kind,
            path,
            line,
            character,
            new_name,
            lang,
            buffers,
        })
    }
}

// ---------------------------------------------------------------- positions

/// The byte offset of an LSP position (line from 0, UTF-16 units from 0) in the text; a
/// position past the end of its line is the end of the line, past the last line the end.
pub fn offset_of(text: &str, line: u32, character: u32) -> usize {
    let mut start = 0;
    for _ in 0..line {
        match text[start..].find('\n') {
            Some(i) => start += i + 1,
            None => return text.len(),
        }
    }
    let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
    let mut units = 0u32;
    for (i, c) in text[start..end].char_indices() {
        if units >= character {
            return start + i;
        }
        units += c.len_utf16() as u32;
    }
    end
}

/// The LSP position (line from 0, UTF-16 units from 0) of a byte offset of the text.
pub fn position_of(text: &str, offset: usize) -> (u32, u32) {
    let offset = offset.min(text.len());
    let mut offset = offset;
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &text[..offset];
    let line = before.matches('\n').count() as u32;
    let start = before.rfind('\n').map_or(0, |i| i + 1);
    let character = text[start..offset]
        .chars()
        .map(|c| c.len_utf16() as u32)
        .sum();
    (line, character)
}

/// An LSP range of the bytes `start..end` of the text.
pub fn range(text: &str, start: usize, end: usize) -> json::Json {
    let (l1, c1) = position_of(text, start);
    let (l2, c2) = position_of(text, end.max(start));
    json::Json::object(vec![
        (
            "start",
            json::Json::object(vec![
                ("line", json::Json::int(l1)),
                ("character", json::Json::int(c1)),
            ]),
        ),
        (
            "end",
            json::Json::object(vec![
                ("line", json::Json::int(l2)),
                ("character", json::Json::int(c2)),
            ]),
        ),
    ])
}

// ---------------------------------------------------------------- URIs

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// The path of a `file:` URI; None for any other scheme (an unsaved `untitled:` document has
/// no package and is not analysed).
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file://host/path`: only the local host.
    let rest = match rest.find('/') {
        Some(0) => rest,
        Some(i) if &rest[..i] == "localhost" => &rest[i..],
        _ => return None,
    };
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let text = String::from_utf8(out).ok()?;
    if cfg!(windows) {
        // `/c:/dir/file.unio`
        let trimmed = text.strip_prefix('/').unwrap_or(&text);
        let has_drive = trimmed.len() >= 2 && trimmed.as_bytes()[1] == b':';
        if has_drive {
            return Some(PathBuf::from(trimmed.replace('/', "\\")));
        }
        return Some(PathBuf::from(text.replace('/', "\\")));
    }
    Some(PathBuf::from(text))
}

/// The `file:` URI of an absolute path.
pub fn path_to_uri(path: &Path) -> String {
    let text = path.to_string_lossy();
    // A canonical path of Windows is verbatim (`\\?\C:\dir`); its URI names the plain path.
    let text = match text.strip_prefix(r"\\?\UNC\") {
        Some(share) => format!(r"\\{share}"),
        None => text.strip_prefix(r"\\?\").unwrap_or(&text).to_string(),
    };
    let text = text.replace('\\', "/");
    let mut out = String::from("file:");
    if text.starts_with("//") {
        // A share: `file://server/share/...`.
    } else if text.starts_with('/') {
        out.push_str("//");
    } else {
        out.push_str("///");
    }
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            // The colon of a drive letter.
            b':' if out.len() == 9 && out.starts_with("file:///") => out.push(':'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_count_utf16_units() {
        let text = "ab\nä😀x\n";
        assert_eq!(offset_of(text, 1, 0), 3);
        assert_eq!(offset_of(text, 1, 1), 5);
        assert_eq!(offset_of(text, 1, 3), 9);
        assert_eq!(offset_of(text, 1, 99), 10);
        assert_eq!(offset_of(text, 9, 0), text.len());
        assert_eq!(position_of(text, 9), (1, 3));
        assert_eq!(position_of(text, 0), (0, 0));
        assert_eq!(position_of(text, 11), (2, 0));
    }

    #[test]
    fn uris_and_paths() {
        #[cfg(unix)]
        {
            let path = Path::new("/tmp/a b/ü.unio");
            let uri = path_to_uri(path);
            assert_eq!(uri, "file:///tmp/a%20b/%C3%BC.unio");
            assert_eq!(uri_to_path(&uri).unwrap(), path);
        }
        assert_eq!(
            path_to_uri(Path::new(r"\\?\C:\dir\a b.unio")),
            "file:///C:/dir/a%20b.unio"
        );
        #[cfg(windows)]
        assert_eq!(
            uri_to_path("file:///c%3A/dir/a.unio").unwrap(),
            PathBuf::from(r"c:\dir\a.unio")
        );
        assert!(uri_to_path("untitled:Untitled-1").is_none());
        assert!(uri_to_path("file://elsewhere/x.unio").is_none());
    }

    #[test]
    fn requests_round_trip() {
        let request = Request {
            kind: Kind::Rename,
            path: PathBuf::from("/p/main.unio"),
            line: 3,
            character: 7,
            new_name: "total".into(),
            lang: Lang::Ru,
            buffers: vec![(PathBuf::from("/p/main.unio"), "текст".into())],
        };
        assert_eq!(Request::decode(&request.encode()), Some(request.clone()));
        let mut bad = request.encode();
        bad.push(0);
        assert_eq!(Request::decode(&bad), None);
    }
}
