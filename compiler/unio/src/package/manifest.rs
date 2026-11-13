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
    /// `package.unio_version`: the versions of the Unio toolchain, compiler and runtime, the
    /// package needs (T-14); `None` for any.
    pub unio_version: Option<Req>,
    /// `package.targets`: the targets the package runs on (T-14); `None` for every target.
    pub targets: Option<Vec<String>>,
}

/// The targets a manifest may name in `package.targets` (T-14, docs/packages.md section 1):
/// the targets of `unio build` (docs/runtime.md 8.4), by their Rust target triples.
pub const TARGETS: [&str; 4] = [
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
];

/// The target of this machine in the names of [`TARGETS`], if it is one of them.
pub fn host_target() -> Option<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(TARGETS[0])
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some(TARGETS[1])
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some(TARGETS[2])
    } else if cfg!(all(windows, target_arch = "x86_64")) {
        Some(TARGETS[3])
    } else {
        None
    }
}

/// The version of this toolchain that `package.unio_version` is matched against: the release
/// it belongs to, without a prerelease tag, so that a development build of 0.1.0 counts as
/// 0.1.0 (T-14).
pub fn toolchain_version() -> Version {
    let mut version =
        Version::parse(env!("CARGO_PKG_VERSION")).expect("the version of the compiler crate");
    version.pre.clear();
    version.build.clear();
    version
}

/// The problem of a target name in `package.targets`, if it is not one of [`TARGETS`].
fn target_problem(name: &str) -> Option<Mistake> {
    (!TARGETS.contains(&name)).then(|| Mistake::Target(name.to_string()))
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
        if let Some(req) =
            text(&table, "package.unio_version").map_err(|e| format!("E0088: package.{e}"))?
        {
            manifest.unio_version = Some(
                Req::parse(&req)
                    .map_err(|e| format!("E0088: package.unio_version '{req}': {e}"))?,
            );
        }
        match table.get("package.targets") {
            None => {}
            Some(TomlValue::Texts(list)) => {
                if let Some(bad) = list.iter().find(|t| target_problem(t).is_some()) {
                    return Err(format!(
                        "E0088: package.targets names '{bad}', which is not a target; use {}",
                        TARGETS.join(", ")
                    ));
                }
                manifest.targets = Some(list.clone());
            }
            Some(_) => {
                return Err(format!(
                    "E0088: package.targets must be a list of targets, for example [\"{}\"]",
                    TARGETS[0]
                ))
            }
        }
        let mut fields: BTreeMap<String, BTreeMap<String, TomlValue>> = BTreeMap::new();
        for (key, value) in &table {
            let Some(rest) = key.strip_prefix("dependencies.") else {
                continue;
            };
            let Some((alias, field)) = rest.split_once('.') else {
                return Err(DependencyMistake::NotTable.text(rest));
            };
            fields
                .entry(alias.to_string())
                .or_default()
                .insert(field.to_string(), value.clone());
        }
        for (alias, fields) in fields {
            manifest
                .dependencies
                .push(dependency(&alias, &fields).map_err(|m| m.text(&alias))?);
        }
        Ok(manifest)
    }

    pub fn dependency(&self, alias: &str) -> Option<&Dependency> {
        self.dependencies.iter().find(|d| d.alias == alias)
    }

    /// Whether this toolchain and this machine meet the target and runtime requirements of the
    /// package (T-14): `package.unio_version` holds for [`toolchain_version`], and
    /// `package.targets`, when given, names the target of this machine. The error says which
    /// does not hold and what the package asks for.
    pub fn requirements_met(&self) -> Result<(), String> {
        requirements_met(self.unio_version.as_ref(), self.targets.as_deref())
    }
}

/// [`Manifest::requirements_met`] for requirements already read from a manifest.
pub fn requirements_met(
    unio_version: Option<&Req>,
    targets: Option<&[String]>,
) -> Result<(), String> {
    {
        if let Some(req) = unio_version {
            let have = toolchain_version();
            if !req.matches(&have) {
                return Err(format!(
                    "it needs Unio {} (package.unio_version), and this is Unio {}",
                    req.text(),
                    env!("CARGO_PKG_VERSION")
                ));
            }
        }
        if let Some(targets) = targets {
            let host = host_target();
            if !host.is_some_and(|h| targets.iter().any(|t| t == h)) {
                return Err(format!(
                    "it runs on {} (package.targets), and this machine is {}",
                    if targets.is_empty() {
                        "no target".to_string()
                    } else {
                        targets.join(", ")
                    },
                    host.unwrap_or("not one of the targets of Unio")
                ));
            }
        }
        Ok(())
    }
}

/// A mistake in one entry of `[dependencies]` (T-6, T-14).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DependencyMistake {
    /// `alias = "1.0"`: an entry that is not a table.
    NotTable,
    Alias,
    NotText(String),
    UnknownField(String),
    /// The version requirement, with the problem.
    Version(String),
    EmptyPath,
    NoPackage,
    RegistryPackage(String),
    NoVersion,
    NoSha,
    BadSha,
    /// No source, or more than one.
    OneSource,
}

impl DependencyMistake {
    /// The English text of `unio add` and the other package commands.
    pub fn text(&self, alias: &str) -> String {
        use DependencyMistake as M;
        match self {
            M::NotTable => format!(
                "the dependency '{alias}' must be a table, for example \
                 {alias} = {{ path = \"../{alias}\" }}"
            ),
            M::Alias => format!(
                "the alias '{alias}' of a dependency must be lowercase letters, digits and '_', \
                 starting with a letter or '_', and not std or deps: it is a segment of the module \
                 path deps.{alias}.<module>"
            ),
            M::NotText(field) => format!("dependencies.{alias}.{field} must be a text"),
            M::UnknownField(field) => format!(
                "unknown key dependencies.{alias}.{field}; use path, registry and package, or \
                 archive and sha384, with version"
            ),
            M::Version(e) => format!("dependencies.{alias}.version: {e}"),
            M::EmptyPath => format!("dependencies.{alias}.path is empty"),
            M::NoPackage => {
                format!("dependencies.{alias} names a registry but no package = \"owner/name\"")
            }
            M::RegistryPackage(package) => {
                format!("dependencies.{alias}.package '{package}' must be owner/name")
            }
            M::NoVersion => {
                format!("dependencies.{alias} from a registry needs a version requirement")
            }
            M::NoSha => {
                format!("dependencies.{alias}.archive needs sha384, the hash of the archive")
            }
            M::BadSha => {
                format!("dependencies.{alias}.sha384 must be 96 lowercase hexadecimal digits")
            }
            M::OneSource => format!(
                "dependencies.{alias} needs exactly one source: path, registry with package, or \
                 archive with sha384"
            ),
        }
    }

    /// The field of the entry the mistake is about, if it is one field.
    fn field(&self) -> Option<&str> {
        use DependencyMistake as M;
        match self {
            M::NotText(field) | M::UnknownField(field) => Some(field),
            M::Version(_) => Some("version"),
            M::EmptyPath => Some("path"),
            M::RegistryPackage(_) => Some("package"),
            M::BadSha => Some("sha384"),
            _ => None,
        }
    }
}

fn dependency(
    alias: &str,
    fields: &BTreeMap<String, TomlValue>,
) -> Result<Dependency, DependencyMistake> {
    use DependencyMistake as M;
    if !valid_alias(alias) {
        return Err(M::Alias);
    }
    let get = |field: &str| -> Result<Option<String>, DependencyMistake> {
        match fields.get(field) {
            None => Ok(None),
            Some(TomlValue::Text(t)) => Ok(Some(t.clone())),
            Some(_) => Err(M::NotText(field.to_string())),
        }
    };
    for field in fields.keys() {
        if ![
            "path", "registry", "package", "version", "archive", "sha384",
        ]
        .contains(&field.as_str())
        {
            return Err(M::UnknownField(field.clone()));
        }
    }
    let req = match get("version")? {
        Some(v) => Some(Req::parse(&v).map_err(M::Version)?),
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
                return Err(M::EmptyPath);
            }
            Source::Path(path)
        }
        (None, true, None) if sha.is_none() => {
            let package = package.ok_or(M::NoPackage)?;
            if !valid_registry_package(&package) {
                return Err(M::RegistryPackage(package));
            }
            if req.is_none() {
                return Err(M::NoVersion);
            }
            Source::Registry { registry, package }
        }
        (None, false, Some(location)) => {
            let sha384 = sha.ok_or(M::NoSha)?;
            if !valid_sha384(&sha384) {
                return Err(M::BadSha);
            }
            Source::Archive { location, sha384 }
        }
        _ => return Err(M::OneSource),
    };
    Ok(Dependency {
        alias: alias.to_string(),
        source,
        req,
    })
}

// ---------------------------------------------------------------- the check of `unio check`

/// What a list of the manifest holds, for the message of a value that is not a list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListOf {
    Paths,
    Variables,
    Programs,
    Modules,
    Targets,
}

/// Why an entry of `files.read` or `files.write` is not a scope (T-6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopeProblem {
    Empty,
    /// `*` other than in the suffix `/**`.
    Star,
    /// A `$NAME` other than `$HOME`, `$CWD` and `$TEMP`.
    Placeholder(String),
}

/// A mistake of a manifest that `unio check` reports as E0088 (T-6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mistake {
    /// The file is not in the subset of TOML that manifests use; the problem as the reader
    /// states it.
    Syntax(String),
    UnknownTable(String),
    /// A key of `[package]` or outside any table that a manifest does not have.
    UnknownKey(String),
    /// `package.name` or `package.version` that is not a text.
    NotText(String),
    Name(String),
    /// `package.version`, with the problem.
    Version(String),
    NotList(String, ListOf),
    Scope {
        key: String,
        entry: String,
        problem: ScopeProblem,
    },
    /// An empty name in `env` or `process`.
    EmptyName(String),
    /// A name in `env` that cannot name a variable.
    Variable(String),
    Network,
    UnknownPermission(String),
    Dependency(String, DependencyMistake),
    /// `package.unio_version` that is not a version requirement, with the problem (T-14).
    UnioVersion(String),
    /// A name in `package.targets` that is not a target (T-14).
    Target(String),
}

/// `package.unsafe_modules` of a manifest and the line to report its comparison with the
/// marked modules at (E0089, T-6, RFC 0004): the line of the key, else of `[package]`, else 1.
/// None when the manifest does not parse or the key is not a list, which is E0088 alone.
pub fn unsafe_modules(source: &str) -> Option<(Vec<String>, usize)> {
    let (table, lines) = toml::parse_with_lines(source).ok()?;
    let line = |key: &str| lines.get(key).copied();
    match table.get("package.unsafe_modules") {
        None => Some((
            Vec::new(),
            line("package")
                .or_else(|| line("package.name"))
                .unwrap_or(1),
        )),
        Some(TomlValue::Texts(list)) => Some((list.clone(), line("package.unsafe_modules")?)),
        Some(_) => None,
    }
}

/// The mistakes of a manifest with the line, from 1, of each: every one is reported once, in
/// the order of the lines. A manifest that does not parse has only that mistake.
pub fn check(source: &str) -> Vec<(usize, Mistake)> {
    let (table, lines) = match toml::parse_with_lines(source) {
        Ok(parsed) => parsed,
        Err(e) => {
            let (line, problem) = e
                .strip_prefix("line ")
                .and_then(|rest| rest.split_once(": "))
                .and_then(|(n, p)| Some((n.parse().ok()?, p.to_string())))
                .unwrap_or((1, e.clone()));
            return vec![(line, Mistake::Syntax(problem))];
        }
    };
    // The line of a key, or of the nearest table or inline table that holds it.
    let line_of = |key: &str| -> usize {
        let mut key = key;
        loop {
            if let Some(line) = lines.get(key) {
                return *line;
            }
            match key.rsplit_once('.') {
                Some((head, _)) => key = head,
                None => return 1,
            }
        }
    };
    const TABLES: [&str; 3] = ["package", "dependencies", "permissions"];
    let mut found: Vec<(usize, Mistake)> = Vec::new();
    // Every name outside the three tables once, at its first line: a table, or a key written
    // outside any table.
    let mut unknown: BTreeMap<&str, usize> = BTreeMap::new();
    for (name, line) in &lines {
        let top = name.split('.').next().unwrap_or("");
        if !TABLES.contains(&top) {
            let first = unknown.entry(top).or_insert(*line);
            *first = (*first).min(*line);
        }
    }
    for (top, line) in unknown {
        found.push((
            line,
            if table.contains_key(top) {
                Mistake::UnknownKey(top.to_string())
            } else {
                Mistake::UnknownTable(top.to_string())
            },
        ));
    }
    let mut dependencies: BTreeMap<String, BTreeMap<String, TomlValue>> = BTreeMap::new();
    for (key, value) in &table {
        let line = line_of(key);
        let Some((top, rest)) = key.split_once('.') else {
            continue; // reported above
        };
        match top {
            "package" => match rest {
                "name" => match value {
                    TomlValue::Text(name) if valid_name(name) => {}
                    TomlValue::Text(name) => found.push((line, Mistake::Name(name.clone()))),
                    _ => found.push((line, Mistake::NotText(key.clone()))),
                },
                "version" => match value {
                    TomlValue::Text(v) => {
                        if let Err(e) = Version::parse(v) {
                            found.push((line, Mistake::Version(e)));
                        }
                    }
                    _ => found.push((line, Mistake::NotText(key.clone()))),
                },
                "unsafe_modules" => {
                    if !matches!(value, TomlValue::Texts(_)) {
                        found.push((line, Mistake::NotList(key.clone(), ListOf::Modules)));
                    }
                }
                "unio_version" => match value {
                    TomlValue::Text(r) => {
                        if let Err(e) = Req::parse(r) {
                            found.push((line, Mistake::UnioVersion(e)));
                        }
                    }
                    _ => found.push((line, Mistake::NotText(key.clone()))),
                },
                "targets" => match value {
                    TomlValue::Texts(list) => {
                        found.extend(
                            list.iter()
                                .filter_map(|t| target_problem(t))
                                .map(|m| (line, m)),
                        );
                    }
                    _ => found.push((line, Mistake::NotList(key.clone(), ListOf::Targets))),
                },
                _ => found.push((line, Mistake::UnknownKey(key.clone()))),
            },
            "permissions" => {
                if let Some(mistake) = permission(key, rest, value) {
                    found.push((line, mistake));
                }
            }
            "dependencies" => match rest.split_once('.') {
                Some((alias, field)) => {
                    dependencies
                        .entry(alias.to_string())
                        .or_default()
                        .insert(field.to_string(), value.clone());
                }
                None => found.push((
                    line,
                    Mistake::Dependency(rest.to_string(), DependencyMistake::NotTable),
                )),
            },
            // A key of an unknown table is reported with its table.
            _ => {}
        }
    }
    for (alias, fields) in &dependencies {
        if let Err(mistake) = dependency(alias, fields) {
            let at = match mistake.field() {
                Some(field) => format!("dependencies.{alias}.{field}"),
                None => format!("dependencies.{alias}"),
            };
            found.push((line_of(&at), Mistake::Dependency(alias.clone(), mistake)));
        }
    }
    found.sort_by_key(|(line, _)| *line);
    found
}

/// The mistake of one key of `[permissions]`, if it has one.
fn permission(key: &str, name: &str, value: &TomlValue) -> Option<Mistake> {
    let list = |of: ListOf| match value {
        TomlValue::Texts(items) => Ok(items),
        _ => Err(Mistake::NotList(key.to_string(), of)),
    };
    let result = match name {
        "files.read" | "files.write" => list(ListOf::Paths).and_then(|items| {
            for entry in items {
                if let Some(problem) = scope_problem(entry) {
                    return Err(Mistake::Scope {
                        key: key.to_string(),
                        entry: entry.clone(),
                        problem,
                    });
                }
            }
            Ok(items)
        }),
        "env" => list(ListOf::Variables).and_then(|items| {
            for entry in items {
                if entry.is_empty() {
                    return Err(Mistake::EmptyName(key.to_string()));
                }
                if entry.contains(['=', '\0']) {
                    return Err(Mistake::Variable(entry.clone()));
                }
            }
            Ok(items)
        }),
        "process" => list(ListOf::Programs).and_then(|items| {
            if items.iter().any(String::is_empty) {
                return Err(Mistake::EmptyName(key.to_string()));
            }
            Ok(items)
        }),
        "network" if *value == TomlValue::Bool(false) => return None,
        "network" => return Some(Mistake::Network),
        _ => return Some(Mistake::UnknownPermission(key.to_string())),
    };
    result.err()
}

/// The problem of an entry of `files.read` or `files.write`, as the sandbox finds it when it
/// expands the entry (`unio_runtime::sandbox::Scope::expand`).
pub fn scope_problem(entry: &str) -> Option<ScopeProblem> {
    let body = entry.strip_suffix("/**").unwrap_or(entry);
    if body.contains('*') {
        return Some(ScopeProblem::Star);
    }
    if let Some(after) = body.strip_prefix('$') {
        let name = after.split_once('/').map_or(after, |(name, _)| name);
        if !["HOME", "CWD", "TEMP"].contains(&name) {
            return Some(ScopeProblem::Placeholder(name.to_string()));
        }
        return None;
    }
    body.is_empty().then_some(ScopeProblem::Empty)
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

    /// `unio check` finds a problem in exactly the entries the sandbox refuses to expand.
    #[test]
    fn scope_problems_agree_with_the_sandbox() {
        use unio_runtime::sandbox::{Places, Scope};
        let places = Places::current(&std::env::temp_dir());
        for entry in [
            "$HOME/data/**",
            "$CWD",
            "$TEMP/x",
            "data/in.txt",
            "/etc/hosts",
            "**",
            "/**",
            "$HOEM/x",
            "$/x",
            "$HOME/*.txt",
            "a/**/b",
            "",
            "x/***",
        ] {
            assert_eq!(
                scope_problem(entry).is_some(),
                Scope::expand(entry, &places).is_err(),
                "{entry:?}"
            );
        }
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
            "[package]\nunio_version = \"zero\"\n",
            "[package]\ntargets = \"x86_64-unknown-linux-gnu\"\n",
            "[package]\ntargets = [\"x86_64-linux\"]\n",
        ] {
            assert!(Manifest::parse(bad).is_err(), "{bad}");
            assert!(!check(bad).is_empty(), "{bad}");
        }
    }

    /// The target and runtime requirements of T-14: met by this toolchain and machine, or not.
    #[test]
    fn target_and_runtime_requirements() {
        let with =
            |body: &str| Manifest::parse(&format!("[package]\nname = \"p\"\n{body}")).unwrap();
        assert!(with("").requirements_met().is_ok());
        let this = toolchain_version();
        let same = format!(
            "unio_version = \"={}.{}.{}\"\n",
            this.major, this.minor, this.patch
        );
        assert!(with(&same).requirements_met().is_ok());
        assert!(check(&format!("[package]\nname = \"p\"\n{same}")).is_empty());
        let later = format!("unio_version = \">={}.0\"\n", this.major + 1);
        let problem = with(&later).requirements_met().unwrap_err();
        assert!(problem.contains("package.unio_version"), "{problem}");
        if let Some(host) = host_target() {
            let here = format!("targets = [\"{host}\"]\n");
            assert!(with(&here).requirements_met().is_ok());
            let other = TARGETS.iter().find(|t| **t != host).unwrap();
            let problem = with(&format!("targets = [\"{other}\"]\n"))
                .requirements_met()
                .unwrap_err();
            assert!(
                problem.contains(host) && problem.contains(other),
                "{problem}"
            );
        }
        assert!(with("targets = []\n").requirements_met().is_err());
    }
}
