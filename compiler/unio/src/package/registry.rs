//! The registry side (T-8): a directory tree that any static HTTP server can serve, written by
//! `unio registry init`, `unio key endorse`, `unio key revoke`, `unio publish`, and
//! `unio registry resign`.
//!
//! ```text
//! <registry>/index/<owner>/<name>.json           the index of a package (super::index)
//! <registry>/index/<owner>/<name>.json.sig       its envelope: registry-index, owner/name, sequence
//! <registry>/archives/<owner>/<name>/<v>.tar     the archive of a version (ustar, deterministic)
//! <registry>/archives/<owner>/<name>/<v>.tar.sig its envelope: package-archive, owner/name@v
//! <registry>/trust/keys                          key endorsements (super::trust)
//! <registry>/trust/revocations                   the revocation list, signed by the anchor
//! <registry>/trust/anchor.pub                    the anchor's public key, for reference only
//! ```
//!
//! A published version is never removed or replaced: publishing a version again with other
//! content is refused, and with the same content changes nothing. Every change of an index
//! raises its sequence. Files are written to a temporary name and renamed, the archive and its
//! signature before the index; a client that reads an index between the rename of the document
//! and of its envelope sees a signature that does not match and refuses it until both are in
//! place.

use std::path::{Path, PathBuf};

use crate::envelope::{self, PublicKey, SecretKey};

use super::archive::{self, Item, Limits};
use super::index::{Index, IndexDep, IndexVersion};
use super::manifest::{valid_registry_package, Manifest, Source};
use super::trust::{self, Endorsement, Revocations};

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path.parent().ok_or("a path without a directory")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = dir.join(format!(".{name}.unio-tmp-{}", std::process::id()));
    std::fs::write(&temporary, bytes)
        .and_then(|()| std::fs::rename(&temporary, path))
        .map_err(|e| {
            let _ = std::fs::remove_file(&temporary);
            format!("{}: {e}", path.display())
        })
}

fn read(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn trust_file(root: &Path, name: &str) -> PathBuf {
    root.join("trust").join(name)
}

/// Makes the layout of a new registry, with an empty revocation list (sequence 1) signed by
/// the anchor and no endorsed keys.
pub fn init(root: &Path, anchor: &SecretKey) -> Result<(), String> {
    let revocations = trust_file(root, "revocations");
    if revocations.exists() {
        return Err(format!(
            "{} already exists: the registry is initialized",
            revocations.display()
        ));
    }
    for dir in ["index", "archives", "trust"] {
        std::fs::create_dir_all(root.join(dir))
            .map_err(|e| format!("{}: {e}", root.join(dir).display()))?;
    }
    if !trust_file(root, "keys").exists() {
        write(&trust_file(root, "keys"), b"")?;
    }
    write(
        &trust_file(root, "anchor.pub"),
        anchor.public().to_file().as_bytes(),
    )?;
    let list = Revocations {
        sequence: 1,
        revoked: Default::default(),
    };
    write(&revocations, list.file(anchor)?.as_bytes())
}

/// Adds the endorsement of `key` by `signer` to the registry's file of endorsements,
/// replacing an earlier endorsement of the same key by the same signer.
pub fn endorse(
    root: &Path,
    signer: &SecretKey,
    key: &PublicKey,
    expires: Option<String>,
) -> Result<(), String> {
    let path = trust_file(root, "keys");
    let old = read(&path)?.unwrap_or_default();
    let signer_id = signer.public().id;
    let mut out = String::new();
    for (e, payload, sealed) in trust::parse_keys(&old)? {
        if e.key.id == key.id && sealed.key == signer_id {
            continue;
        }
        out.push_str(std::str::from_utf8(&payload).unwrap_or_default());
        out.push_str(&String::from_utf8_lossy(&sealed.signed));
        out.push_str(&format!("signature {}\n", envelope::hex(&sealed.signature)));
    }
    out.push_str(&trust::endorsement_record(
        signer,
        &Endorsement {
            key: key.clone(),
            expires,
        },
    )?);
    write(&path, out.as_bytes())
}

/// Adds a key to the revocation list, signed by the anchor, with the next sequence.
pub fn revoke(root: &Path, anchor: &SecretKey, id: &str) -> Result<u64, String> {
    let path = trust_file(root, "revocations");
    let old = read(&path)?.ok_or_else(|| {
        format!(
            "{} does not exist: run `unio registry init` first",
            path.display()
        )
    })?;
    let (mut list, _, sealed) = Revocations::parse_file(&old)?;
    let anchor_id = anchor.public().id;
    if sealed.key != anchor_id {
        return Err(format!(
            "the revocation list is signed by the anchor {}, and this key is {anchor_id}: only \
             the anchor revokes",
            sealed.key
        ));
    }
    if id == anchor_id {
        return Err("the anchor cannot revoke itself; the operator provisions a new one".into());
    }
    list.revoked.insert(id.to_string());
    list.sequence += 1;
    write(&path, list.file(anchor)?.as_bytes())?;
    Ok(list.sequence)
}

/// The deterministic archive of a package directory, as `unio publish` makes it, and its
/// manifest: the files `unio add --path` would copy, in sorted order with fixed metadata.
pub fn package_archive(dir: &Path) -> Result<(Manifest, Vec<u8>), String> {
    let entries = super::tree::read_dir(dir, &Limits::default(), true)?;
    for (name, item) in &entries {
        if let Item::File(bytes) = item {
            if SecretKey::looks_like(bytes) {
                return Err(format!(
                    "{name} is a secret key file: keys never go into a package; move it out of \
                     the package directory"
                ));
            }
        }
    }
    let Some(Item::File(text)) = entries.get("unio.toml") else {
        return Err("the package has no unio.toml".into());
    };
    let text = std::str::from_utf8(text).map_err(|_| "unio.toml is not UTF-8")?;
    let manifest = Manifest::parse(text).map_err(|e| format!("unio.toml: {e}"))?;
    Ok((manifest, archive::write(&entries)))
}

/// What [`publish`] did.
#[derive(Debug, PartialEq, Eq)]
pub struct Published {
    pub package: String,
    pub version: String,
    pub sha384: String,
    /// The sequence of the index after publishing.
    pub sequence: u64,
    /// The version was already published with the same content; nothing changed.
    pub already: bool,
}

/// Publishes the package in `dir` as `owner/<its name>`, signed with `key`.
pub fn publish(root: &Path, owner: &str, dir: &Path, key: &SecretKey) -> Result<Published, String> {
    if !trust_file(root, "revocations").is_file() {
        return Err(format!(
            "{} is not a registry: run `unio registry init` first",
            root.display()
        ));
    }
    let (manifest, bytes) = package_archive(dir)?;
    let package = format!("{owner}/{}", manifest.name);
    if !valid_registry_package(&package) {
        return Err(format!(
            "{package} is not a valid package of a registry: owner/name, lowercase"
        ));
    }
    let version = manifest
        .version
        .clone()
        .ok_or("unio.toml has no package.version; a published package needs one")?;
    let mut dependencies = Vec::new();
    for d in &manifest.dependencies {
        match (&d.source, &d.req) {
            (
                Source::Registry {
                    registry: None,
                    package,
                },
                Some(req),
            ) => dependencies.push(IndexDep {
                alias: d.alias.clone(),
                package: package.clone(),
                req: req.clone(),
            }),
            _ => {
                return Err(format!(
                    "the dependency '{}' is not a package of the same registry with a version \
                     requirement (`{{ package = \"owner/name\", version = \"^1.0\" }}`): a \
                     published package cannot depend on a path, an archive, or another registry",
                    d.alias
                ))
            }
        }
    }
    dependencies.sort_by(|a, b| a.alias.cmp(&b.alias));
    let sha = crate::sha::sha384_hex(&bytes);
    let archive_rel = format!("archives/{package}/{version}.tar");
    let signature_rel = format!("{archive_rel}.sig");
    let index_path = root.join(format!("index/{package}.json"));
    let mut index = match read(&index_path)? {
        Some(text) => {
            let text = String::from_utf8(text).map_err(|_| "the index is not UTF-8")?;
            Index::parse(&text, &package).map_err(|e| format!("{}: {e}", index_path.display()))?
        }
        None => Index {
            package: package.clone(),
            sequence: 0,
            versions: Vec::new(),
        },
    };
    if let Some(v) = index.versions.iter().find(|v| v.version == version) {
        if v.sha384 == sha {
            return Ok(Published {
                package,
                version: version.to_string(),
                sha384: sha,
                sequence: index.sequence,
                already: true,
            });
        }
        return Err(format!(
            "{package} {version} is already published with other content (sha384 {}): a \
             published version is never replaced or removed (T-8); publish a new version",
            v.sha384
        ));
    }
    let archive_path = root.join(&archive_rel);
    if let Some(existing) = read(&archive_path)? {
        if existing != bytes {
            return Err(format!(
                "{} exists with other content, though the index does not list it; refused",
                archive_path.display()
            ));
        }
    }
    write(&archive_path, &bytes)?;
    let subject = format!("{package}@{version}");
    let sealed = envelope::seal(key, "package-archive", &subject, 0, &bytes)?;
    write(&root.join(&signature_rel), sealed.as_bytes())?;
    index.versions.push(IndexVersion {
        version: version.clone(),
        archive: archive_rel,
        sha384: sha.clone(),
        signature: signature_rel,
        dependencies,
    });
    index.versions.sort_by(|a, b| a.version.cmp(&b.version));
    index.sequence += 1;
    write_index(root, &index, key)?;
    Ok(Published {
        package,
        version: version.to_string(),
        sha384: sha,
        sequence: index.sequence,
        already: false,
    })
}

fn write_index(root: &Path, index: &Index, key: &SecretKey) -> Result<(), String> {
    let json = index.to_json();
    let sealed = envelope::seal(
        key,
        "registry-index",
        &index.package,
        index.sequence,
        json.as_bytes(),
    )?;
    let path = root.join(format!("index/{}.json", index.package));
    write(&path, json.as_bytes())?;
    write(
        &root.join(format!("index/{}.json.sig", index.package)),
        sealed.as_bytes(),
    )
}

/// Signs every archive and index of the registry again with `key`, each index with the next
/// sequence: the recovery after a signing key is revoked. The archives are checked against
/// their indexes first; nothing is published or removed. Returns the number of indexes.
pub fn resign(root: &Path, key: &SecretKey) -> Result<usize, String> {
    let mut indexes = Vec::new();
    let base = root.join("index");
    for owner in std::fs::read_dir(&base).map_err(|e| format!("{}: {e}", base.display()))? {
        let owner = owner.map_err(|e| e.to_string())?.path();
        if !owner.is_dir() {
            continue;
        }
        for file in std::fs::read_dir(&owner).map_err(|e| e.to_string())? {
            let file = file.map_err(|e| e.to_string())?.path();
            if file.extension().is_some_and(|e| e == "json") {
                indexes.push(file);
            }
        }
    }
    indexes.sort();
    for path in &indexes {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let owner = path.parent().and_then(Path::file_name).unwrap_or_default();
        let name = path.file_stem().unwrap_or_default();
        let package = format!("{}/{}", owner.to_string_lossy(), name.to_string_lossy());
        let mut index =
            Index::parse(&text, &package).map_err(|e| format!("{}: {e}", path.display()))?;
        for v in &index.versions {
            let bytes = std::fs::read(root.join(&v.archive))
                .map_err(|e| format!("{}: {e}", root.join(&v.archive).display()))?;
            if crate::sha::sha384_hex(&bytes) != v.sha384 {
                return Err(format!(
                    "the archive of {package} {} does not have the hash of its index; nothing \
                     more is signed",
                    v.version
                ));
            }
            let sealed = envelope::seal(
                key,
                "package-archive",
                &format!("{package}@{}", v.version),
                0,
                &bytes,
            )?;
            write(&root.join(&v.signature), sealed.as_bytes())?;
        }
        index.sequence += 1;
        write_index(root, &index, key)?;
    }
    Ok(indexes.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishing_is_deterministic_and_never_replaces_a_version() {
        let base = std::env::temp_dir().join(format!("unio-registry-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (reg, pkg) = (base.join("reg"), base.join("pkg"));
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(
            pkg.join("unio.toml"),
            "[package]\nname = \"util\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        std::fs::write(
            pkg.join("a.unio"),
            "public function f() returns Int:\n    return 1\n",
        )
        .unwrap();
        let key = SecretKey::from_seed(&[7; 32]);
        assert!(publish(&reg, "acme", &pkg, &key).is_err());
        init(&reg, &key).unwrap();
        assert!(init(&reg, &key).is_err());
        let first = publish(&reg, "acme", &pkg, &key).unwrap();
        assert_eq!((first.sequence, first.already), (1, false));
        let tar = std::fs::read(reg.join("archives/acme/util/1.0.0.tar")).unwrap();
        assert_eq!(package_archive(&pkg).unwrap().1, tar);
        assert!(publish(&reg, "acme", &pkg, &key).unwrap().already);
        std::fs::write(
            pkg.join("a.unio"),
            "public function f() returns Int:\n    return 2\n",
        )
        .unwrap();
        let again = publish(&reg, "acme", &pkg, &key).unwrap_err();
        assert!(again.contains("never replaced or removed"), "{again}");
        assert_eq!(
            std::fs::read(reg.join("archives/acme/util/1.0.0.tar")).unwrap(),
            tar
        );
        // A secret key in the package is never published.
        std::fs::write(pkg.join("release.key"), key.to_file()).unwrap();
        assert!(package_archive(&pkg).unwrap_err().contains("secret key"));
        let _ = std::fs::remove_dir_all(&base);
    }
}
