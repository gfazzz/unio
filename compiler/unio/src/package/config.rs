//! The configuration of the operator for package operations (SB-12): the registries the
//! download worker may reach and the limits of archives. It is a file of the operator, never of
//! a project: `UNIO_PACKAGES_CONFIG`, or `packages.toml` in the configuration directory of
//! Unio (`$XDG_CONFIG_HOME/unio` or `~/.config/unio` on Linux, `~/Library/Application
//! Support/unio` on macOS, `%APPDATA%\unio` on Windows). Without it no registry is configured,
//! and only path and local archive dependencies can be installed.
//!
//! ```toml
//! [registry.main]
//! url = "http://registry.example.org/unio"
//! # The trust anchor of the registry: a public key file (`unio key new`) that the operator
//! # obtained independently of the registry, relative to this file (T-8a).
//! anchor = "keys/main-anchor.pub"
//!
//! [limits]
//! archive_bytes = 67108864
//! unpacked_bytes = 67108864
//! file_bytes = 16777216
//! entries = 10000
//! depth = 32
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

use unio_runtime::sandbox::toml::{self, TomlValue};

use super::archive::Limits;
use super::http::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registry {
    pub name: String,
    pub url: Url,
    /// The public key file of the trust anchor, absolute once loaded.
    pub anchor: PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub registries: BTreeMap<String, Registry>,
    pub limits: Limits,
    /// The file it was read from, for messages.
    pub file: Option<PathBuf>,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// The configuration directory of Unio for the operator.
pub fn config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("unio"));
    }
    if cfg!(target_os = "macos") {
        return home().map(|h| h.join("Library/Application Support/unio"));
    }
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".config")))
        .map(|d| d.join("unio"))
}

/// The cache of packages: `UNIO_CACHE`, or the cache directory of the system
/// (`$XDG_CACHE_HOME/unio` or `~/.cache/unio` on Linux, `~/Library/Caches/unio` on macOS,
/// `%LOCALAPPDATA%\unio\cache` on Windows).
pub fn default_cache() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("UNIO_CACHE").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    if cfg!(windows) {
        return std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("unio/cache"));
    }
    if cfg!(target_os = "macos") {
        return home().map(|h| h.join("Library/Caches/unio"));
    }
    std::env::var_os("XDG_CACHE_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".cache")))
        .map(|d| d.join("unio"))
}

impl Config {
    /// The configuration of the operator, or none.
    pub fn load() -> Result<Config, String> {
        let file = match std::env::var_os("UNIO_PACKAGES_CONFIG").filter(|f| !f.is_empty()) {
            Some(file) => PathBuf::from(file),
            None => match config_dir() {
                Some(dir) if dir.join("packages.toml").is_file() => dir.join("packages.toml"),
                _ => return Ok(Config::default()),
            },
        };
        let text = std::fs::read_to_string(&file)
            .map_err(|e| format!("the packages configuration {}: {e}", file.display()))?;
        let mut config = Config::parse(&text)
            .map_err(|e| format!("the packages configuration {}: {e}", file.display()))?;
        let dir = std::path::absolute(&file)
            .ok()
            .and_then(|f| f.parent().map(PathBuf::from))
            .unwrap_or_default();
        for registry in config.registries.values_mut() {
            registry.anchor = dir.join(&registry.anchor);
        }
        config.file = Some(file);
        Ok(config)
    }

    pub fn parse(text: &str) -> Result<Config, String> {
        let table = toml::parse(text)?;
        let mut config = Config::default();
        let mut registries: BTreeMap<String, (Option<String>, Option<String>)> = BTreeMap::new();
        for (key, value) in &table {
            if let Some(rest) = key.strip_prefix("registry.") {
                let (name, field) = rest
                    .split_once('.')
                    .ok_or_else(|| format!("{key} must be a table with url"))?;
                if !super::manifest::valid_name(name) {
                    return Err(format!("the registry name {name:?} is not a valid name"));
                }
                let entry = registries.entry(name.to_string()).or_default();
                match (field, value) {
                    ("url", TomlValue::Text(url)) => entry.0 = Some(url.clone()),
                    ("anchor", TomlValue::Text(path)) if !path.is_empty() => {
                        entry.1 = Some(path.clone())
                    }
                    ("allow_unsigned", _) => {
                        return Err(format!(
                            "{key} is no longer accepted: a registry package is installed only \
                             with a valid ML-DSA-65 signature (T-8, docs/cryptography.md); give \
                             the registry its trust anchor with anchor = \"FILE.pub\", or use an \
                             archive dependency whose sha384 the manifest names"
                        ))
                    }
                    _ => return Err(format!("unknown or mistyped key {key}")),
                }
                continue;
            }
            let n = match value {
                TomlValue::Int(n) if *n > 0 => *n as u64,
                _ => return Err(format!("{key} must be a positive whole number")),
            };
            let l = &mut config.limits;
            match key.as_str() {
                "limits.archive_bytes" => l.archive_bytes = n,
                "limits.unpacked_bytes" => l.unpacked_bytes = n,
                "limits.file_bytes" => l.file_bytes = n,
                "limits.entries" => l.entries = n as usize,
                "limits.depth" => l.depth = n as usize,
                _ => return Err(format!("unknown key {key}")),
            }
        }
        for (name, (url, anchor)) in registries {
            let url = url.ok_or_else(|| format!("registry.{name} has no url"))?;
            let anchor = anchor.ok_or_else(|| {
                format!(
                    "registry.{name} has no anchor: the operator provisions the public key file \
                     of the registry's trust anchor, anchor = \"FILE.pub\" (T-8a)"
                )
            })?;
            let url = Url::parse(&url).map_err(|e| format!("registry.{name}.url: {e}"))?;
            config.registries.insert(
                name.clone(),
                Registry {
                    name,
                    url,
                    anchor: PathBuf::from(anchor),
                },
            );
        }
        Ok(config)
    }

    /// The registry whose canonical URL this is.
    pub fn registry_at(&self, canonical: &str) -> Option<&Registry> {
        self.registries
            .values()
            .find(|r| r.url.canonical() == canonical)
    }

    /// Whether the download worker may connect to the origin of this URL: only the hosts of
    /// the configured registries (SB-12).
    pub fn allows(&self, url: &Url) -> bool {
        self.registries
            .values()
            .any(|r| r.url.host == url.host && r.url.port == url.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registries_and_limits() {
        let c = Config::parse(
            "[registry.main]\nurl = \"http://127.0.0.1:9/reg/\"\nanchor = \"a.pub\"\n[limits]\nentries = 5\n",
        )
        .unwrap();
        let main = &c.registries["main"];
        assert_eq!(main.url.canonical(), "http://127.0.0.1:9/reg");
        assert_eq!(main.anchor, PathBuf::from("a.pub"));
        assert!(Config::parse("[registry.main]\nurl = \"http://x/\"\n").is_err());
        assert!(Config::parse(
            "[registry.main]\nurl = \"http://x/\"\nanchor = \"a.pub\"\nallow_unsigned = true\n"
        )
        .unwrap_err()
        .contains("no longer accepted"));
        assert_eq!(c.limits.entries, 5);
        assert!(c.allows(&Url::parse("http://127.0.0.1:9/other").unwrap()));
        assert!(!c.allows(&Url::parse("http://127.0.0.1:10/reg").unwrap()));
        assert!(Config::parse("[registry.main]\nurl = \"https://x/\"\n").is_err());
        assert!(Config::parse("[registry.main]\nanchor = \"a.pub\"\n").is_err());
        assert!(Config::parse("[limits]\nentries = 0\n").is_err());
        assert!(Config::parse("colour = 1\n").is_err());
    }
}
