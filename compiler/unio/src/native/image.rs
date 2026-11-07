//! The program embedded in an executable made by `unio build`: the path of the main file,
//! the directory of the standard library, every source file the program loads (and the
//! manifests that decide its package), and the manifest whose permissions the executable
//! applies. The encoding is a sequence of length-prefixed byte strings, so that two builds of
//! the same sources give the same bytes (C-4).

use std::path::PathBuf;

pub struct Embedded {
    pub main: PathBuf,
    pub stdlib: Option<PathBuf>,
    pub manifest: Option<(String, PathBuf)>,
    pub files: Vec<(PathBuf, String)>,
}

fn put(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn path_bytes(p: &std::path::Path) -> Vec<u8> {
    p.to_string_lossy().as_bytes().to_vec()
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn bytes(&mut self) -> Option<&[u8]> {
        let len = u64::from_le_bytes(self.data.get(self.at..self.at + 8)?.try_into().ok()?);
        self.at += 8;
        let end = self.at.checked_add(usize::try_from(len).ok()?)?;
        let b = self.data.get(self.at..end)?;
        self.at = end;
        Some(b)
    }

    fn text(&mut self) -> Option<String> {
        String::from_utf8(self.bytes()?.to_vec()).ok()
    }

    fn path(&mut self) -> Option<PathBuf> {
        self.text().map(PathBuf::from)
    }
}

impl Embedded {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put(&mut out, &path_bytes(&self.main));
        match &self.stdlib {
            Some(p) => {
                put(&mut out, b"1");
                put(&mut out, &path_bytes(p));
            }
            None => put(&mut out, b"0"),
        }
        match &self.manifest {
            Some((text, root)) => {
                put(&mut out, b"1");
                put(&mut out, text.as_bytes());
                put(&mut out, &path_bytes(root));
            }
            None => put(&mut out, b"0"),
        }
        put(&mut out, self.files.len().to_string().as_bytes());
        for (path, text) in &self.files {
            put(&mut out, &path_bytes(path));
            put(&mut out, text.as_bytes());
        }
        out
    }

    pub fn decode(data: &[u8]) -> Option<Embedded> {
        let mut r = Reader { data, at: 0 };
        let main = r.path()?;
        let stdlib = match r.bytes()? {
            b"1" => Some(r.path()?),
            _ => None,
        };
        let manifest = match r.bytes()? {
            b"1" => Some((r.text()?, r.path()?)),
            _ => None,
        };
        let count: usize = r.text()?.parse().ok()?;
        let mut files = Vec::with_capacity(count);
        for _ in 0..count {
            files.push((r.path()?, r.text()?));
        }
        Some(Embedded {
            main,
            stdlib,
            manifest,
            files,
        })
    }
}
