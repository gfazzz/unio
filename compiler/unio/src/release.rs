//! The signed manifest of a release (R-6, R-6a; docs/cryptography.md, section 4): the files of
//! a release with their SHA-384, signed in an envelope ([`crate::envelope`]) with the purpose
//! `release-manifest`, a subject that names the release (`unio-0.1.0-linux-x86_64`), and a
//! sequence that grows with every release, so that a verifier refuses an older release than
//! one it has accepted. `unio release sign` writes it and `unio release verify` checks it.
//!
//! ```text
//! unio-release-manifest-1
//! file sha384:<96 hexadecimal digits> bin/unio
//! file sha384:<96 hexadecimal digits> lib/unio/libunio.a
//! ```
//!
//! Names are relative to the directory of the manifest, with `/`, sorted, without `..`.

use crate::envelope;

pub const FORMAT: &str = "unio-release-manifest-1";
pub const MAX_MANIFEST: usize = 1 << 20;

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && name.bytes().all(|b| b.is_ascii_graphic() && b != b'\\')
        && !name
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
}

/// The manifest of the files, by their names relative to its directory.
pub fn manifest(files: &[(String, Vec<u8>)]) -> Result<String, String> {
    let mut lines: Vec<(&str, String)> = Vec::new();
    for (name, bytes) in files {
        if !valid_name(name) {
            return Err(format!(
                "{name:?} is not a relative name below the manifest"
            ));
        }
        lines.push((name, format!("file {} {name}\n", envelope::digest(bytes))));
    }
    lines.sort();
    if lines.windows(2).any(|w| w[0].0 == w[1].0) {
        return Err("a file is named twice".into());
    }
    let mut out = format!("{FORMAT}\n");
    for (_, line) in lines {
        out.push_str(&line);
    }
    Ok(out)
}

/// The (name, digest) lines of a manifest, strictly.
pub fn parse(text: &[u8]) -> Result<Vec<(String, String)>, String> {
    if text.len() > MAX_MANIFEST {
        return Err(format!("the manifest is larger than {MAX_MANIFEST} bytes"));
    }
    let text = std::str::from_utf8(text).map_err(|_| "the manifest is not text")?;
    let mut lines = text.lines();
    if lines.next() != Some(FORMAT) || !text.ends_with('\n') {
        return Err(format!("the manifest is not of the format {FORMAT}"));
    }
    let mut out = Vec::new();
    for line in lines {
        let rest = line
            .strip_prefix("file sha384:")
            .ok_or_else(|| format!("a malformed line of the manifest: {line:?}"))?;
        let (hex, name) = rest
            .split_once(' ')
            .filter(|(h, n)| envelope::unhex(h, 48).is_some() && valid_name(n))
            .ok_or_else(|| format!("a malformed line of the manifest: {line:?}"))?;
        out.push((name.to_string(), format!("sha384:{hex}")));
    }
    if out.windows(2).any(|w| w[0].0 >= w[1].0) {
        return Err("the manifest is not sorted by name or names a file twice".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manifest_round_trips_and_refuses_bad_names() {
        let files = vec![
            ("lib/a".to_string(), b"a".to_vec()),
            ("bin/unio".to_string(), b"u".to_vec()),
        ];
        let text = manifest(&files).unwrap();
        let parsed = parse(text.as_bytes()).unwrap();
        assert_eq!(parsed[0].0, "bin/unio");
        assert_eq!(parsed[1].1, envelope::digest(b"a"));
        for bad in ["../x", "/x", "a//b", "a b"] {
            assert!(manifest(&[(bad.to_string(), Vec::new())]).is_err(), "{bad}");
        }
        assert!(parse(text.replace("bin/unio", "zz").as_bytes()).is_err());
    }
}
