//! The index of a package in a registry (SB-12): one JSON document per package at
//! `<registry>/index/<owner>/<name>.json`, which a static file server can serve:
//!
//! ```json
//! {
//!   "format": "unio-registry-index-1",
//!   "package": "acme/json",
//!   "versions": [
//!     {
//!       "version": "1.2.0",
//!       "archive": "archives/acme/json/1.2.0.tar",
//!       "sha384": "<96 hexadecimal digits of the archive>",
//!       "signature": "archives/acme/json/1.2.0.tar.sig",
//!       "dependencies": [{"alias": "text", "package": "acme/text", "version": "^1.0"}]
//!     }
//!   ]
//! }
//! ```
//!
//! Paths are relative to the registry and stay below it; dependencies name packages of the
//! same registry. The download worker parses the document and gives the tool the normalized
//! lines of [`Index::encode`]; the dependencies are checked again against the manifest in the
//! archive when it is unpacked.

use crate::lsp::json::Json;

use super::manifest::{valid_alias, valid_registry_package, valid_sha384};
use super::semver::{Req, Version};

pub const FORMAT: &str = "unio-registry-index-1";
pub const MAX_VERSIONS: usize = 10_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexDep {
    pub alias: String,
    pub package: String,
    pub req: Req,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexVersion {
    pub version: Version,
    pub archive: String,
    pub sha384: String,
    pub signature: Option<String>,
    pub dependencies: Vec<IndexDep>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Index {
    pub package: String,
    pub versions: Vec<IndexVersion>,
}

fn relative(text: &str) -> Result<String, String> {
    // The same rule as a path below the registry URL.
    super::http::Url::parse("http://registry/")?.join(text)?;
    Ok(text.to_string())
}

fn no_space(text: &str, what: &str) -> Result<(), String> {
    if text.is_empty() || text.chars().any(char::is_whitespace) {
        return Err(format!("{what} {text:?} is empty or has spaces"));
    }
    Ok(())
}

impl Index {
    pub fn parse(text: &str, expected: &str) -> Result<Index, String> {
        let json = Json::parse(text).map_err(|e| format!("the index is not JSON: {e}"))?;
        if json.get("format").and_then(Json::as_str) != Some(FORMAT) {
            return Err(format!("the index is not of the format {FORMAT}"));
        }
        let package = json
            .get("package")
            .and_then(Json::as_str)
            .ok_or("the index names no package")?;
        if package != expected {
            return Err(format!(
                "the index is of the package {package}, not of {expected}"
            ));
        }
        let versions = json
            .get("versions")
            .and_then(Json::as_array)
            .ok_or("the index has no list of versions")?;
        if versions.len() > MAX_VERSIONS {
            return Err(format!("the index has more than {MAX_VERSIONS} versions"));
        }
        let mut out = Vec::new();
        for v in versions {
            let field = |name: &str| -> Result<&str, String> {
                v.get(name)
                    .and_then(Json::as_str)
                    .ok_or_else(|| format!("a version of the index has no {name}"))
            };
            let version = Version::parse(field("version")?)?;
            let sha384 = field("sha384")?.to_string();
            if !valid_sha384(&sha384) {
                return Err(format!(
                    "the sha384 of {version} is not 96 lowercase hex digits"
                ));
            }
            let signature = match v.get("signature") {
                None | Some(Json::Null) => None,
                Some(s) => Some(relative(s.as_str().ok_or("a signature is not a text")?)?),
            };
            let mut dependencies = Vec::new();
            if let Some(deps) = v.get("dependencies") {
                for d in deps.as_array().ok_or("dependencies is not a list")? {
                    let alias = d.get("alias").and_then(Json::as_str).unwrap_or("");
                    let package = d.get("package").and_then(Json::as_str).unwrap_or("");
                    let req = d.get("version").and_then(Json::as_str).unwrap_or("");
                    if !valid_alias(alias) || !valid_registry_package(package) {
                        return Err(format!("a dependency of {version} is malformed"));
                    }
                    let req = Req::parse(&req.replace(' ', ""))?;
                    dependencies.push(IndexDep {
                        alias: alias.into(),
                        package: package.into(),
                        req,
                    });
                }
            }
            dependencies.sort_by(|a, b| a.alias.cmp(&b.alias));
            out.push(IndexVersion {
                version,
                archive: relative(field("archive")?)?,
                sha384,
                signature,
                dependencies,
            });
        }
        out.sort_by(|a, b| a.version.cmp(&b.version));
        if out.windows(2).any(|w| w[0].version == w[1].version) {
            return Err("the index lists a version twice".into());
        }
        Ok(Index {
            package: package.to_string(),
            versions: out,
        })
    }

    /// Lines for the tool: `package P`, then per version `version V SHA ARCHIVE SIGNATURE|-`
    /// followed by its `dep ALIAS PACKAGE REQ` lines.
    pub fn encode(&self) -> String {
        let mut out = format!("package {}\n", self.package);
        for v in &self.versions {
            out.push_str(&format!(
                "version {} {} {} {}\n",
                v.version,
                v.sha384,
                v.archive,
                v.signature.as_deref().unwrap_or("-")
            ));
            for d in &v.dependencies {
                out.push_str(&format!("dep {} {} {}\n", d.alias, d.package, d.req));
            }
        }
        out
    }

    pub fn decode(text: &str) -> Result<Index, String> {
        let bad = |line: &str| format!("a malformed line of the index: {line}");
        let mut index = Index {
            package: String::new(),
            versions: Vec::new(),
        };
        for line in text.lines() {
            let parts: Vec<&str> = line.split(' ').collect();
            match parts.as_slice() {
                ["package", p] => index.package = p.to_string(),
                ["version", v, sha, archive, signature] => {
                    no_space(archive, "archive")?;
                    index.versions.push(IndexVersion {
                        version: Version::parse(v)?,
                        sha384: sha.to_string(),
                        archive: relative(archive)?,
                        signature: (*signature != "-")
                            .then(|| relative(signature))
                            .transpose()?,
                        dependencies: Vec::new(),
                    })
                }
                ["dep", alias, package, req] => index
                    .versions
                    .last_mut()
                    .ok_or_else(|| bad(line))?
                    .dependencies
                    .push(IndexDep {
                        alias: alias.to_string(),
                        package: package.to_string(),
                        req: Req::parse(req)?,
                    }),
                _ => return Err(bad(line)),
            }
        }
        Ok(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_index_parses_and_round_trips() {
        let h = "a".repeat(96);
        let text = format!(
            r#"{{"format": "unio-registry-index-1", "package": "acme/json", "versions": [
              {{"version": "1.2.0", "archive": "archives/acme/json/1.2.0.tar", "sha384": "{h}",
                "dependencies": [{{"alias": "text", "package": "acme/text", "version": ">= 1.0, < 2"}}]}},
              {{"version": "1.0.0", "archive": "archives/acme/json/1.0.0.tar", "sha384": "{h}",
                "signature": "archives/acme/json/1.0.0.tar.sig"}}]}}"#
        );
        let index = Index::parse(&text, "acme/json").unwrap();
        assert_eq!(index.versions[0].version.to_string(), "1.0.0");
        assert_eq!(Index::decode(&index.encode()).unwrap(), index);
        assert!(Index::parse(&text, "acme/other").is_err());
        let evil = text.replace("archives/acme/json/1.2.0.tar", "../../x");
        assert!(Index::parse(&evil, "acme/json").is_err());
        let evil = text.replace("archives/acme/json/1.2.0.tar", "http://evil/x.tar");
        assert!(Index::parse(&evil, "acme/json").is_err());
    }
}
