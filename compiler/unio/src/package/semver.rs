//! Semantic versions and version requirements of `[dependencies]` (T-6, T-14).
//!
//! A version is `MAJOR.MINOR.PATCH`, optionally `-PRERELEASE` and `+BUILD` (Semantic Versioning
//! 2.0.0; the build is ignored in comparisons). A requirement is one or more comparators
//! separated by commas, all of which must hold: `^1.2.3` (the default for a bare version),
//! `~1.2`, `=1.2.3`, `>1.0`, `>=1.0`, `<2.0`, `<=2.0`, and `*`. A prerelease version satisfies a
//! requirement only when a comparator names a prerelease of the same `MAJOR.MINOR.PATCH`
//! (language-contracts section 6: prereleases require an explicit prerelease constraint).

use std::cmp::Ordering;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Pre {
    Number(u64),
    Text(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Vec<Pre>,
    pub build: String,
}

fn number(text: &str) -> Result<u64, String> {
    if text.is_empty()
        || !text.bytes().all(|b| b.is_ascii_digit())
        || (text.len() > 1 && text.starts_with('0'))
    {
        return Err(format!("'{text}' is not a version number"));
    }
    text.parse()
        .map_err(|_| format!("'{text}' is too large for a version number"))
}

fn identifiers(text: &str, what: &str) -> Result<Vec<String>, String> {
    let parts: Vec<String> = text.split('.').map(str::to_string).collect();
    for p in &parts {
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err(format!("'{text}' is not a valid {what}"));
        }
    }
    Ok(parts)
}

impl Version {
    pub fn parse(text: &str) -> Result<Version, String> {
        let text = text.trim();
        let (rest, build) = match text.split_once('+') {
            Some((rest, build)) => {
                identifiers(build, "build")?;
                (rest, build.to_string())
            }
            None => (text, String::new()),
        };
        let (core, pre) = match rest.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (rest, None),
        };
        let numbers: Vec<&str> = core.split('.').collect();
        if numbers.len() != 3 {
            return Err(format!(
                "'{text}' is not a version of the form MAJOR.MINOR.PATCH"
            ));
        }
        let pre = match pre {
            None => Vec::new(),
            Some(pre) => identifiers(pre, "prerelease")?
                .into_iter()
                .map(|p| {
                    if p.bytes().all(|b| b.is_ascii_digit()) {
                        number(&p).map(Pre::Number)
                    } else {
                        Ok(Pre::Text(p))
                    }
                })
                .collect::<Result<_, _>>()?,
        };
        Ok(Version {
            major: number(numbers[0])?,
            minor: number(numbers[1])?,
            patch: number(numbers[2])?,
            pre,
            build,
        })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }

    fn triple(&self) -> (u64, u64, u64) {
        (self.major, self.minor, self.patch)
    }
}

impl Ord for Pre {
    fn cmp(&self, other: &Pre) -> Ordering {
        match (self, other) {
            (Pre::Number(a), Pre::Number(b)) => a.cmp(b),
            (Pre::Number(_), Pre::Text(_)) => Ordering::Less,
            (Pre::Text(_), Pre::Number(_)) => Ordering::Greater,
            (Pre::Text(a), Pre::Text(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for Pre {
    fn partial_cmp(&self, other: &Pre) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Version) -> Ordering {
        self.triple()
            .cmp(&other.triple())
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            })
            // The build is ignored by precedence; it orders only equal versions, so that the
            // order of candidates is total and deterministic.
            .then_with(|| self.build.cmp(&other.build))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Version) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            let parts: Vec<String> = self
                .pre
                .iter()
                .map(|p| match p {
                    Pre::Number(n) => n.to_string(),
                    Pre::Text(t) => t.clone(),
                })
                .collect();
            write!(f, "-{}", parts.join("."))?;
        }
        if !self.build.is_empty() {
            write!(f, "+{}", self.build)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Caret,
    Tilde,
    Exact,
    Greater,
    GreaterEq,
    Less,
    LessEq,
    Any,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Comparator {
    op: Op,
    major: u64,
    minor: Option<u64>,
    patch: Option<u64>,
    pre: Vec<Pre>,
}

/// A version requirement, kept with the text it was written as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Req {
    text: String,
    comparators: Vec<Comparator>,
}

impl Req {
    pub fn parse(text: &str) -> Result<Req, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("an empty version requirement".into());
        }
        let mut comparators = Vec::new();
        for part in text.split(',') {
            comparators.push(Comparator::parse(part.trim())?);
        }
        Ok(Req {
            text: text.to_string(),
            comparators,
        })
    }

    /// The requirement `=VERSION`.
    pub fn exact(version: &Version) -> Req {
        Req::parse(&format!("={version}")).expect("a version is a valid requirement")
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn matches(&self, version: &Version) -> bool {
        if !self.comparators.iter().all(|c| c.matches(version)) {
            return false;
        }
        if !version.is_prerelease() {
            return true;
        }
        // A prerelease only for a comparator that names a prerelease of the same version.
        self.comparators.iter().any(|c| {
            !c.pre.is_empty()
                && c.major == version.major
                && c.minor == Some(version.minor)
                && c.patch == Some(version.patch)
        })
    }
}

impl fmt::Display for Req {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl Comparator {
    fn parse(text: &str) -> Result<Comparator, String> {
        if text == "*" {
            return Ok(Comparator {
                op: Op::Any,
                major: 0,
                minor: None,
                patch: None,
                pre: Vec::new(),
            });
        }
        let (op, rest) = [
            (">=", Op::GreaterEq),
            ("<=", Op::LessEq),
            (">", Op::Greater),
            ("<", Op::Less),
            ("=", Op::Exact),
            ("^", Op::Caret),
            ("~", Op::Tilde),
        ]
        .iter()
        .find_map(|(prefix, op)| text.strip_prefix(prefix).map(|r| (*op, r.trim())))
        .unwrap_or((Op::Caret, text));
        let (core, pre) = match rest.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (rest, None),
        };
        if rest.contains('+') {
            return Err(format!("a requirement names no build: '{text}'"));
        }
        let numbers: Vec<&str> = core.split('.').collect();
        if numbers.is_empty() || numbers.len() > 3 {
            return Err(format!("'{text}' is not a version requirement"));
        }
        let major =
            number(numbers[0]).map_err(|_| format!("'{text}' is not a version requirement"))?;
        let part = |i: usize| -> Result<Option<u64>, String> {
            match numbers.get(i) {
                None => Ok(None),
                Some(n) => number(n)
                    .map(Some)
                    .map_err(|_| format!("'{text}' is not a version requirement")),
            }
        };
        let (minor, patch) = (part(1)?, part(2)?);
        let pre = match pre {
            None => Vec::new(),
            Some(_) if patch.is_none() => {
                return Err(format!("a prerelease needs a full version: '{text}'"))
            }
            Some(pre) => Version::parse(&format!("0.0.0-{pre}"))?.pre,
        };
        Ok(Comparator {
            op,
            major,
            minor,
            patch,
            pre,
        })
    }

    fn lower(&self) -> Version {
        Version {
            major: self.major,
            minor: self.minor.unwrap_or(0),
            patch: self.patch.unwrap_or(0),
            pre: self.pre.clone(),
            build: String::new(),
        }
    }

    fn matches(&self, v: &Version) -> bool {
        let at_least = |low: &Version| cmp_ignoring_build(v, low) != Ordering::Less;
        let triple = |a: u64, b: u64, c: u64| (v.major, v.minor, v.patch) < (a, b, c);
        match self.op {
            Op::Any => true,
            Op::Exact => match (self.minor, self.patch) {
                (Some(minor), Some(patch)) => {
                    (v.major, v.minor, v.patch) == (self.major, minor, patch) && v.pre == self.pre
                }
                (Some(minor), None) => v.major == self.major && v.minor == minor,
                _ => v.major == self.major,
            },
            Op::Greater => match (self.minor, self.patch) {
                (Some(_), Some(_)) => cmp_ignoring_build(v, &self.lower()) == Ordering::Greater,
                (Some(minor), None) => (v.major, v.minor) > (self.major, minor),
                _ => v.major > self.major,
            },
            Op::GreaterEq => at_least(&self.lower()),
            Op::Less => cmp_ignoring_build(v, &self.lower()) == Ordering::Less,
            Op::LessEq => match (self.minor, self.patch) {
                (Some(_), Some(_)) => cmp_ignoring_build(v, &self.lower()) != Ordering::Greater,
                (Some(minor), None) => (v.major, v.minor) <= (self.major, minor),
                _ => v.major <= self.major,
            },
            Op::Tilde => {
                at_least(&self.lower())
                    && match self.minor {
                        Some(minor) => triple(self.major, minor + 1, 0),
                        None => triple(self.major + 1, 0, 0),
                    }
            }
            Op::Caret => {
                at_least(&self.lower())
                    && match (self.major, self.minor, self.patch) {
                        (0, Some(0), Some(patch)) => triple(0, 0, patch + 1),
                        (0, Some(minor), _) => triple(0, minor + 1, 0),
                        (major, _, _) => triple(major + 1, 0, 0),
                    }
            }
        }
    }
}

fn cmp_ignoring_build(a: &Version, b: &Version) -> Ordering {
    let strip = |v: &Version| Version {
        build: String::new(),
        ..v.clone()
    };
    strip(a).cmp(&strip(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    fn m(req: &str, version: &str) -> bool {
        Req::parse(req).unwrap().matches(&v(version))
    }

    #[test]
    fn versions_parse_and_order() {
        assert!(v("1.0.0-alpha") < v("1.0.0-alpha.1"));
        assert!(v("1.0.0-alpha.1") < v("1.0.0-alpha.beta"));
        assert!(v("1.0.0-beta.11") > v("1.0.0-beta.2"));
        assert!(v("1.0.0-rc.1") < v("1.0.0"));
        assert!(v("1.10.0") > v("1.9.9"));
        assert_eq!(v("1.2.3+build.5").to_string(), "1.2.3+build.5");
        for bad in [
            "1.2",
            "01.2.3",
            "1.2.3-",
            "1.2.3-a..b",
            "x.y.z",
            "1.2.3.4",
            "",
        ] {
            assert!(Version::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn requirements_match() {
        assert!(m("1.2.3", "1.9.0"));
        assert!(!m("1.2.3", "2.0.0"));
        assert!(!m("1.2.3", "1.2.2"));
        assert!(m("^0.2.3", "0.2.9"));
        assert!(!m("^0.2.3", "0.3.0"));
        assert!(m("^0.0.3", "0.0.3"));
        assert!(!m("^0.0.3", "0.0.4"));
        assert!(m("~1.2", "1.2.7"));
        assert!(!m("~1.2", "1.3.0"));
        assert!(m("=1.2.3", "1.2.3"));
        assert!(!m("=1.2.3", "1.2.4"));
        assert!(m(">=1.0, <2.0", "1.5.0"));
        assert!(!m(">=1.0, <2.0", "2.0.0"));
        assert!(m("*", "7.0.0"));
        assert!(m(">1.2", "1.3.0"));
        assert!(!m(">1.2", "1.2.9"));
        assert!(m("<=1.2", "1.2.9"));
    }

    #[test]
    fn prereleases_need_an_explicit_prerelease_constraint() {
        assert!(!m("^1.0.0", "1.1.0-beta.1"));
        assert!(!m("*", "1.0.0-rc.1"));
        assert!(m("^1.1.0-beta.1", "1.1.0-beta.2"));
        assert!(!m("^1.1.0-beta.1", "1.2.0-beta.1"));
        assert!(m("^1.1.0-beta.1", "1.2.0"));
    }

    #[test]
    fn bad_requirements_are_errors() {
        for bad in ["", ">=", "1.x", "^1.2.3+b", "1.2-pre", "a"] {
            assert!(Req::parse(bad).is_err(), "{bad}");
        }
    }
}
