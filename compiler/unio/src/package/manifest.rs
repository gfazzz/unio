//! The package part of `unio.toml` (T-6, T-14): `[package]` with `name`, `version`, and
//! `unsafe_modules`, and `[dependencies]`, one entry per alias:
//!
//! ```toml
//! [dependencies]
//! util = { path = "../util" }
//! json = { registry = "main", package = "acme/json", version = "^1.2" }
//! vendored = { archive = "vendor/vendored-1.0.0.tar", sha384 = "<96 hex digits>" }
//! ```
//!
//! `[permissions]` is read by the sandbox (`unio_runtime::sandbox`); this module reads it only as
//! written, for the review of T-9.

use std::collections::BTreeMap;

use unio_runtime::sandbox::toml::{self, TomlValue};
use unio_runtime::sandbox::Declared;

use super::semver::{Req, Version};

/// Where a dependency comes from, as the manifest writes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A directory, relative to the directory of the manifest that names it.
    Path(String),
    /// A registry by the name the operator gave it (the packages configuration), or `None`
    /// in a package of a registry for the registry of that package; and the package
    /// `owner/name` there.
    Registry {
        registry: Option<String>,
        package: String,
    },
    /// An archive file (relative to the manifest) or an `http://` URL, with the SHA-384 of its
    /// bytes in lowercase hexadecimal.
    Archive { location: String, sha384: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dependency {
    pub alias: String,
    pub source: Source,
    pub req: Option<Req>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub version: Option<Version>,
    pub unsafe_modules: Vec<String>,
    /// By alias.
    pub dependencies: Vec<Dependency>,
    pub permissions: Declared,
}

/// An alias is one segment of a module path: `deps.<alias>.<module>` (T-14).
pub fn valid_alias(alias: &str) -> bool {
    let mut chars = alias.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && alias != "std"
        && alias != "deps"
}

/// A package name: lowercase letters, digits, `-` and `_`, starting with a letter.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 64
        && matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// A package of a registry: `owner/name`, both valid names.
pub fn valid_registry_package(package: &str) -> bool {
    match package.split_once('/') {
        Some((owner, name)) => valid_name(owner) && valid_name(name),
        None => false,
    }
}

pub fn valid_sha384(hex: &str) -> bool {
    hex.len() == 96
        && hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn text(table: &toml::Table, key: &str) -> Result<Option<String>, String> {
    match table.get(key) {
        None => Ok(None),
        Some(TomlValue::Text(t)) => Ok(Some(t.clone())),
        Some(_) => Err(format!("{key} must be a text")),
    }
}

impl Manifest {
    pub fn parse(source: &str) -> Result<Manifest, String> {
        let table = toml::parse(source)?;
        let permissions = unio_runtime::sandbox::declared_in_manifest(source)?;
        let mut manifest = Manifest {
            permissions,
            ..Manifest::default()
        };
        if let Some(name) = text(&table, "package.name")? {
            if !valid_name(&name) {
                return Err(format!(
                    "package.name '{name}' must be lowercase letters, digits, '-' and '_', \
                     starting with a letter"
                ));
            }
            manifest.name = name;
        }
        if let Some(version) = text(&table, "package.version")? {
            manifest.version =
                Some(Version::parse(&version).map_err(|e| format!("package.version: {e}"))?);
        }
        match table.get("package.unsafe_modules") {
            None => {}
            Some(TomlValue::Texts(list)) => manifest.unsafe_modules = list.clone(),
            Some(_) => return Err("package.unsafe_modules must be a list of module names".into()),
        }
        let mut fields: BTreeMap<String, BTreeMap<String, TomlValue>> = BTreeMap::new();
        for (key, value) in &table {
            let Some(rest) = key.strip_prefix("dependencies.") else {
                continue;
            };
            let Some((alias, field)) = rest.split_once('.') else {
                return Err(format!(
                    "the dependency '{rest}' must be a table, for example \
                     {rest} = {{ path = \"../{rest}\" }}"
                ));
            };
            fields
                .entry(alias.to_string())
                .or_default()
                .insert(field.to_string(), value.clone());
        }
        for (alias, fields) in fields {
            manifest.dependencies.push(dependency(&alias, &fields)?);
        }
        Ok(manifest)
    }

    pub fn dependency(&self, alias: &str) -> Option<&Dependency> {
        self.dependencies.iter().find(|d| d.alias == alias)
    }
}

fn dependency(alias: &str, fields: &BTreeMap<String, TomlValue>) -> Result<Dependency, String> {
    if !valid_alias(alias) {
        return Err(format!(
            "the alias '{alias}' of a dependency must be lowercase letters, digits and '_', \
             starting with a letter or '_', and not std or deps: it is a segment of the module \
             path deps.{alias}.<module>"
        ));
    }
    let get = |field: &str| -> Result<Option<String>, String> {
        match fields.get(field) {
            None => Ok(None),
            Some(TomlValue::Text(t)) => Ok(Some(t.clone())),
            Some(_) => Err(format!("dependencies.{alias}.{field} must be a text")),
        }
    };
    for field in fields.keys() {
        if ![
            "path", "registry", "package", "version", "archive", "sha384",
        ]
        .contains(&field.as_str())
        {
            return Err(format!(
                "unknown key dependencies.{alias}.{field}; use path, registry and package, or \
                 archive and sha384, with version"
            ));
        }
    }
    let req = match get("version")? {
        Some(v) => Some(Req::parse(&v).map_err(|e| format!("dependencies.{alias}.version: {e}"))?),
        None => None,
    };
    let (path, registry, package, archive, sha) = (
        get("path")?,
        get("registry")?,
        get("package")?,
        get("archive")?,
        get("sha384")?,
    );
    let source = match (path, registry.is_some() || package.is_some(), archive) {
        (Some(path), false, None) if sha.is_none() => {
            if path.is_empty() {
                return Err(format!("dependencies.{alias}.path is empty"));
            }
            Source::Path(path)
        }
        (None, true, None) if sha.is_none() => {
            let package = package.ok_or_else(|| {
                format!("dependencies.{alias} names a registry but no package = \"owner/name\"")
            })?;
            if !valid_registry_package(&package) {
                return Err(format!(
                    "dependencies.{alias}.package '{package}' must be owner/name"
                ));
            }
            if req.is_none() {
                return Err(format!(
                    "dependencies.{alias} from a registry needs a version requirement"
                ));
            }
            Source::Registry { registry, package }
        }
        (None, false, Some(location)) => {
            let sha384 = sha.ok_or_else(|| {
                format!("dependencies.{alias}.archive needs sha384, the hash of the archive")
            })?;
            if !valid_sha384(&sha384) {
                return Err(format!(
                    "dependencies.{alias}.sha384 must be 96 lowercase hexadecimal digits"
                ));
            }
            Source::Archive { location, sha384 }
        }
        _ => {
            return Err(format!(
                "dependencies.{alias} needs exactly one source: path, registry with package, or \
                 archive with sha384"
            ))
        }
    };
    Ok(Dependency {
        alias: alias.to_string(),
        source,
        req,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn dependencies_of_every_source() {
        let m = Manifest::parse(&format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nunsafe_modules = [\"raw\"]\n\n\
             [dependencies]\nutil = {{ path = \"../util\" }}\n\
             json = {{ registry = \"main\", package = \"acme/json\", version = \"^1.2\" }}\n\
             [dependencies.vendored]\narchive = \"v.tar\"\nsha384 = \"{HASH}\"\n"
        ))
        .unwrap();
        assert_eq!(m.name, "app");
        assert_eq!(m.unsafe_modules, vec!["raw".to_string()]);
        let aliases: Vec<&str> = m.dependencies.iter().map(|d| d.alias.as_str()).collect();
        assert_eq!(aliases, ["json", "util", "vendored"]);
        assert_eq!(
            m.dependency("util").unwrap().source,
            Source::Path("../util".into())
        );
        assert!(matches!(
            &m.dependency("json").unwrap().source,
            Source::Registry { registry: Some(r), package } if r == "main" && package == "acme/json"
        ));
    }

    #[test]
    fn mistakes_are_errors() {
        for bad in [
            "[dependencies]\nutil = \"1.0\"\n",
            "[dependencies]\nUtil = { path = \"x\" }\n",
            "[dependencies]\nstd = { path = \"x\" }\n",
            "[dependencies]\nu = { path = \"x\", archive = \"y\" }\n",
            "[dependencies]\nu = { registry = \"main\", package = \"acme/u\" }\n",
            "[dependencies]\nu = { registry = \"main\", package = \"u\", version = \"1\" }\n",
            "[dependencies]\nu = { archive = \"x.tar\" }\n",
            "[dependencies]\nu = { archive = \"x.tar\", sha384 = \"abc\" }\n",
            "[dependencies]\nu = { path = \"x\", colour = \"red\" }\n",
            "[dependencies]\nu = { path = \"x\", version = \"one\" }\n",
            "[package]\nname = \"Bad Name\"\n",
        ] {
            assert!(Manifest::parse(bad).is_err(), "{bad}");
        }
    }
}
