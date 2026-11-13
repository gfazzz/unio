//! `unio fmt --migrate` (R-9): within 0.x an incompatible change of the language ships with an
//! automatic migration or a diagnostic with a fix. A migration here is a registered diagnostic
//! whose fix is unambiguous: the migration applies exactly the fixes the checker offers for that
//! code, so the rewrite and the diagnostic can never disagree, and a second run finds nothing to
//! do (the fixed program no longer has the diagnostic). The registry is keyed by the language
//! version (the version of docs/concept.md) that made the change; `--from VERSION` applies the
//! migrations of the later versions only.

use std::path::PathBuf;

use crate::diag::{Diagnostic, Fix};

/// The version of the language these sources are migrated to: the version of docs/concept.md.
pub const LANGUAGE_VERSION: &str = "0.15";

/// One registered migration.
#[derive(Clone, Copy, Debug)]
pub struct Migration {
    /// A stable name, shown in the report.
    pub id: &'static str,
    /// The language version that made the incompatible change.
    pub since: &'static str,
    /// The diagnostic whose fix is the migration.
    pub code: &'static str,
    /// Where the change is decided.
    pub source: &'static str,
    pub summary: &'static str,
}

/// The registry, oldest first. Every entry needs a test that migrates a program written before
/// the change (`tests/migrate.rs`).
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        id: "variant-in-full",
        since: "0.4",
        code: "E0072",
        source: "concept 0.4, section 2.6; L-6a",
        summary:
            "a variant whose short name collides with a visible name is written with its enum name",
    },
    Migration {
        id: "named-only-label",
        since: "0.13",
        code: "E0040",
        source: "RFC 0001; L-28",
        summary: "a positional argument of the one open `named` parameter gets its label",
    },
];

/// Compares two versions written as dot-separated numbers.
pub fn version_key(version: &str) -> Option<Vec<u64>> {
    version
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect()
}

/// The migrations of the versions after `from`, or all of them.
pub fn selected(from: Option<&str>) -> Result<Vec<Migration>, String> {
    let Some(from) = from else {
        return Ok(MIGRATIONS.to_vec());
    };
    let key = version_key(from).ok_or_else(|| format!("'{from}' is not a language version"))?;
    Ok(MIGRATIONS
        .iter()
        .filter(|m| version_key(m.since).is_some_and(|since| since > key))
        .copied()
        .collect())
}

/// One change made in a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    pub id: &'static str,
    pub line: usize,
    pub column: usize,
    pub new_text: String,
}

/// What migrating one file gives: the new text (unformatted), and what was applied.
#[derive(Clone, Debug)]
pub struct Migrated {
    pub path: PathBuf,
    pub source: String,
    pub text: String,
    pub applied: Vec<Applied>,
}

/// The fix of a diagnostic when it is a registered migration with one fix.
fn migration_fix<'a>(d: &'a Diagnostic, migrations: &[Migration]) -> Option<(&'a Fix, Migration)> {
    let migration = migrations.iter().find(|m| m.code == d.code)?;
    match d.fixes.as_slice() {
        [fix] => Some((fix, *migration)),
        _ => None,
    }
}

/// Applies non-overlapping fixes to a text, from the end so that offsets stay valid.
pub fn apply_fixes(source: &str, fixes: &[(usize, usize, String)]) -> String {
    let mut sorted: Vec<&(usize, usize, String)> = fixes.iter().collect();
    sorted.sort_by_key(|(start, end, _)| (*start, *end));
    let mut kept: Vec<&(usize, usize, String)> = Vec::new();
    for fix in sorted {
        if kept
            .last()
            .is_some_and(|last| fix.0 < last.1 || fix.0 == last.0)
        {
            continue;
        }
        kept.push(fix);
    }
    let mut text = source.to_string();
    for (start, end, new_text) in kept.into_iter().rev() {
        if *start <= *end && *end <= text.len() && text.is_char_boundary(*start) {
            text.replace_range(*start..*end, new_text);
        }
    }
    text
}

/// Checks the files as one program and applies the registered migrations to each given file.
pub fn migrate(files: &[PathBuf], migrations: &[Migration]) -> Vec<Migrated> {
    let files = files.to_vec();
    let migrations = migrations.to_vec();
    crate::on_front_end_stack(move || migrate_here(&files, &migrations))
}

fn migrate_here(files: &[PathBuf], migrations: &[Migration]) -> Vec<Migrated> {
    let program = crate::load_checked(files, crate::effects::Profile::Application);
    crate::analyses(program)
        .into_iter()
        .filter(|a| a.path.ends_with(".unio"))
        .map(|analysis| {
            let mut fixes = Vec::new();
            let mut applied = Vec::new();
            let mut seen = Vec::new();
            for d in &analysis.diagnostics {
                if let Some((fix, migration)) = migration_fix(d, migrations) {
                    let key = (fix.span.start.offset, fix.span.end.offset);
                    if seen.contains(&key) {
                        continue;
                    }
                    seen.push(key);
                    fixes.push((key.0, key.1, fix.new_text.clone()));
                    applied.push(Applied {
                        id: migration.id,
                        line: fix.span.start.line,
                        column: fix.span.start.column,
                        new_text: fix.new_text.clone(),
                    });
                }
            }
            let text = apply_fixes(&analysis.source, &fixes);
            Migrated {
                path: PathBuf::from(&analysis.path),
                source: analysis.source,
                text,
                applied,
            }
        })
        .collect()
}

/// The registry as text, for `unio fmt --migrate --list`.
pub fn list() -> String {
    let mut out = format!("language version {LANGUAGE_VERSION}; migrations:\n");
    for m in MIGRATIONS {
        out.push_str(&format!(
            "  {} (since {}, {}): {} [{}]\n",
            m.id, m.since, m.code, m.summary, m.source
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number() {
        assert!(version_key("0.13").unwrap() > version_key("0.4").unwrap());
        assert!(version_key("x").is_none());
        let after = selected(Some("0.4")).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, "named-only-label");
        assert!(selected(Some(LANGUAGE_VERSION)).unwrap().is_empty());
        assert_eq!(selected(None).unwrap().len(), MIGRATIONS.len());
    }

    #[test]
    fn overlapping_fixes_apply_once() {
        let text = apply_fixes(
            "abcdef",
            &[(1, 2, "X".into()), (1, 2, "Y".into()), (4, 4, "!".into())],
        );
        assert_eq!(text, "aXcd!ef");
    }

    #[test]
    fn every_migration_names_a_code_with_an_explanation() {
        for m in MIGRATIONS {
            assert!(crate::catalog::CODES.contains(&m.code), "{}", m.id);
            assert!(version_key(m.since).is_some(), "{}", m.id);
        }
    }
}
