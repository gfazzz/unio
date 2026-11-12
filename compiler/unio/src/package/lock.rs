//! `unio.lock` (T-7, T-14): the exact versions and content hashes of every dependency of a
//! package, direct and transitive, and the edges between them. `unio add`, `remove`, and
//! `update` write it; `unio install` and every build read it and never change it. The text is a
//! function of its content alone: packages are sorted by identity and every list is sorted, so
//! two resolutions to the same packages give the same bytes.
//!
//! ```toml
//! lock_version = 1
//!
//! [root]
//! dependencies = ["util||path+../util"]
//!
//! [package.p1]
//! id = "path+../util"
//! name = "util"
//! version = "1.0.0"
//! content = "sha384:..."
//! dependencies = []
//! ```
//!
//! An edge is `alias|requirement|identity`; the requirement is empty where the manifest gives
//! none. `content` is the hash of the unpacked files ([`super::tree`]); `archive` the SHA-384 of
//! the archive bytes, for packages that come as an archive; `fetch` where the archive is
//! downloaded from; `index` the hash of the registry index the version was chosen from;
//! `unsafe_modules` and `permissions` what the review of T-9 showed.

use std::collections::BTreeMap;

use unio_runtime::sandbox::toml::{self, TomlValue};

use super::manifest::valid_alias;

pub const FILE: &str = "unio.lock";
const LOCK_VERSION: i64 = 1;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Edge {
    pub alias: String,
    /// The requirement as the manifest wrote it, without spaces; empty for none.
    pub req: String,
    pub id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub version: String,
    /// `sha384:<hex>` of the files of the package.
    pub content: String,
    /// `sha384:<hex>` of the archive, for a package that comes as one.
    pub archive: Option<String>,
    /// The URL of the archive, for one that is downloaded.
    pub fetch: Option<String>,
    /// The URL of the detached signature, for a package of a registry.
    pub signature: Option<String>,
    /// `sha384:<hex>` of the registry index the version was chosen from.
    pub index: Option<String>,
    pub unsafe_modules: Vec<String>,
    pub permissions: Vec<String>,
    pub dependencies: Vec<Edge>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lock {
    pub root: Vec<Edge>,
    pub packages: Vec<Entry>,
}

/// The hexadecimal digest of a `sha384:<hex>` text, when it is one.
pub fn digest_hex(text: &str) -> Option<&str> {
    text.strip_prefix("sha384:")
        .filter(|h| super::manifest::valid_sha384(h))
}

fn edge_text(e: &Edge) -> String {
    format!("{}|{}|{}", e.alias, e.req, e.id)
}

fn parse_edge(text: &str) -> Result<Edge, String> {
    let mut parts = text.splitn(3, '|');
    let (Some(alias), Some(req), Some(id)) = (parts.next(), parts.next(), parts.next()) else {
        return Err(format!(
            "the edge '{text}' is not alias|requirement|identity"
        ));
    };
    if !valid_alias(alias) || id.is_empty() {
        return Err(format!(
            "the edge '{text}' is not alias|requirement|identity"
        ));
    }
    if !req.is_empty() {
        super::semver::Req::parse(req)?;
    }
    Ok(Edge {
        alias: alias.into(),
        req: req.into(),
        id: id.into(),
    })
}

fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn list(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|i| quote(i)).collect();
    format!("[{}]", quoted.join(", "))
}

impl Lock {
    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.packages.iter().find(|e| e.id == id)
    }

    /// Sorts everything, so that the text depends only on the content.
    pub fn normalize(&mut self) {
        self.root.sort();
        self.root.dedup();
        for e in &mut self.packages {
            e.dependencies.sort();
            e.dependencies.dedup();
            e.unsafe_modules.sort();
            e.unsafe_modules.dedup();
            e.permissions.sort();
            e.permissions.dedup();
        }
        self.packages.sort_by(|a, b| a.id.cmp(&b.id));
    }

    pub fn write(&self) -> String {
        let mut lock = self.clone();
        lock.normalize();
        let mut out = String::from(
            "# unio.lock: the exact dependencies of this package (T-7). Written by unio add, \
             remove and\n# update; unio install and builds only read it. Do not edit by hand.\n",
        );
        out.push_str(&format!("lock_version = {LOCK_VERSION}\n\n[root]\n"));
        let root: Vec<String> = lock.root.iter().map(edge_text).collect();
        out.push_str(&format!("dependencies = {}\n", list(&root)));
        for (i, e) in lock.packages.iter().enumerate() {
            out.push_str(&format!("\n[package.p{}]\n", i + 1));
            out.push_str(&format!("id = {}\n", quote(&e.id)));
            out.push_str(&format!("name = {}\n", quote(&e.name)));
            out.push_str(&format!("version = {}\n", quote(&e.version)));
            out.push_str(&format!("content = {}\n", quote(&e.content)));
            for (key, value) in [
                ("archive", &e.archive),
                ("fetch", &e.fetch),
                ("signature", &e.signature),
                ("index", &e.index),
            ] {
                if let Some(v) = value {
                    out.push_str(&format!("{key} = {}\n", quote(v)));
                }
            }
            out.push_str(&format!("unsafe_modules = {}\n", list(&e.unsafe_modules)));
            out.push_str(&format!("permissions = {}\n", list(&e.permissions)));
            let deps: Vec<String> = e.dependencies.iter().map(edge_text).collect();
            out.push_str(&format!("dependencies = {}\n", list(&deps)));
        }
        out
    }

    pub fn parse(source: &str) -> Result<Lock, String> {
        let table = toml::parse(source)?;
        match table.get("lock_version") {
            Some(TomlValue::Int(LOCK_VERSION)) => {}
            Some(TomlValue::Int(n)) => {
                return Err(format!(
                    "lock_version {n} is not supported; this unio reads version {LOCK_VERSION}"
                ))
            }
            _ => return Err("lock_version is missing".into()),
        }
        let texts = |key: &str| -> Result<Vec<String>, String> {
            match table.get(key) {
                None => Ok(Vec::new()),
                Some(TomlValue::Texts(t)) => Ok(t.clone()),
                Some(_) => Err(format!("{key} must be a list of texts")),
            }
        };
        let mut lock = Lock {
            root: texts("root.dependencies")?
                .iter()
                .map(|t| parse_edge(t))
                .collect::<Result<_, _>>()?,
            packages: Vec::new(),
        };
        let mut entries: BTreeMap<String, BTreeMap<String, TomlValue>> = BTreeMap::new();
        for (key, value) in &table {
            if key == "lock_version" || key == "root.dependencies" {
                continue;
            }
            let rest = key
                .strip_prefix("package.")
                .ok_or_else(|| format!("unknown key {key} in {FILE}"))?;
            let (n, field) = rest
                .split_once('.')
                .ok_or_else(|| format!("unknown key {key} in {FILE}"))?;
            entries
                .entry(n.to_string())
                .or_default()
                .insert(field.to_string(), value.clone());
        }
        for (n, fields) in entries {
            let text = |f: &str| -> Result<Option<String>, String> {
                match fields.get(f) {
                    None => Ok(None),
                    Some(TomlValue::Text(t)) => Ok(Some(t.clone())),
                    Some(_) => Err(format!("package.{n}.{f} must be a text")),
                }
            };
            let need = |f: &str| -> Result<String, String> {
                text(f)?.ok_or_else(|| format!("package.{n}.{f} is missing in {FILE}"))
            };
            let texts = |f: &str| -> Result<Vec<String>, String> {
                match fields.get(f) {
                    None => Ok(Vec::new()),
                    Some(TomlValue::Texts(t)) => Ok(t.clone()),
                    Some(_) => Err(format!("package.{n}.{f} must be a list of texts")),
                }
            };
            for f in fields.keys() {
                if ![
                    "id",
                    "name",
                    "version",
                    "content",
                    "archive",
                    "fetch",
                    "signature",
                    "index",
                    "unsafe_modules",
                    "permissions",
                    "dependencies",
                ]
                .contains(&f.as_str())
                {
                    return Err(format!("unknown key package.{n}.{f} in {FILE}"));
                }
            }
            let entry = Entry {
                id: need("id")?,
                name: need("name")?,
                version: need("version")?,
                content: need("content")?,
                archive: text("archive")?,
                fetch: text("fetch")?,
                signature: text("signature")?,
                index: text("index")?,
                unsafe_modules: texts("unsafe_modules")?,
                permissions: texts("permissions")?,
                dependencies: texts("dependencies")?
                    .iter()
                    .map(|t| parse_edge(t))
                    .collect::<Result<_, _>>()?,
            };
            // The content hash names the cache entry, so it must be exactly a digest: a lock
            // cannot name another directory through it.
            if digest_hex(&entry.content).is_none() {
                return Err(format!(
                    "package.{n}.content of {FILE} is not sha384:<96 hexadecimal digits>"
                ));
            }
            if let Some(a) = &entry.archive {
                if digest_hex(a).is_none() {
                    return Err(format!(
                        "package.{n}.archive of {FILE} is not sha384:<96 hexadecimal digits>"
                    ));
                }
            }
            super::semver::Version::parse(&entry.version)
                .map_err(|e| format!("package.{n}.version of {FILE}: {e}"))?;
            if lock.entry(&entry.id).is_some() {
                return Err(format!("{} is locked twice in {FILE}", entry.id));
            }
            lock.packages.push(entry);
        }
        // Every edge leads to a locked package.
        for edge in lock
            .root
            .iter()
            .chain(lock.packages.iter().flat_map(|e| e.dependencies.iter()))
        {
            if lock.entry(&edge.id).is_none() {
                return Err(format!(
                    "{FILE} names {} without a package entry for it",
                    edge.id
                ));
            }
        }
        lock.normalize();
        Ok(lock)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(c: char) -> String {
        format!("sha384:{}", c.to_string().repeat(96))
    }

    fn sample() -> Lock {
        Lock {
            root: vec![Edge {
                alias: "util".into(),
                req: String::new(),
                id: "path+../util".into(),
            }],
            packages: vec![
                Entry {
                    id: "path+../util".into(),
                    name: "util".into(),
                    version: "1.0.0".into(),
                    content: hash('a'),
                    dependencies: vec![Edge {
                        alias: "text".into(),
                        req: "^1.0".into(),
                        id: "registry+http://r.example/x#acme/text".into(),
                    }],
                    ..Entry::default()
                },
                Entry {
                    id: "registry+http://r.example/x#acme/text".into(),
                    name: "text".into(),
                    version: "1.2.0".into(),
                    content: hash('b'),
                    archive: Some(hash('c')),
                    fetch: Some("http://r.example/x/archives/acme/text/1.2.0.tar".into()),
                    unsafe_modules: vec!["raw".into()],
                    permissions: vec!["env HOME".into()],
                    ..Entry::default()
                },
            ],
        }
    }

    #[test]
    fn the_text_round_trips_and_is_deterministic() {
        let lock = sample();
        let text = lock.write();
        let parsed = Lock::parse(&text).unwrap();
        assert_eq!(parsed.write(), text);
        let mut reversed = lock.clone();
        reversed.packages.reverse();
        assert_eq!(reversed.write(), text);
    }

    #[test]
    fn a_lock_cannot_name_another_directory_or_a_missing_package() {
        let text = sample().write().replace(&hash('a'), "sha384:../../etc");
        assert!(Lock::parse(&text).is_err());
        let mut lock = sample();
        lock.packages.remove(1);
        assert!(Lock::parse(&lock.write()).is_err());
        assert!(Lock::parse("lock_version = 2\n").is_err());
        assert!(Lock::parse("lock_version = 1\nextra = 1\n").is_err());
    }
}
