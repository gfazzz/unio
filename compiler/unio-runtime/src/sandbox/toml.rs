//! The subset of TOML that manifests (`unio.toml`), lock files (`unio.lock`), and policies use:
//! tables, dotted keys, inline tables on one line (`util = { path = "../util" }`, whose members
//! become dotted keys), and values that are texts, integers, booleans, or lists of texts.
//! Anything else is an error, so that a mistyped policy never silently grants or leaves out a
//! restriction.

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum TomlValue {
    Text(String),
    Int(i64),
    Bool(bool),
    Texts(Vec<String>),
}

/// Keys are written in full, for example `permissions.files.read`.
pub type Table = BTreeMap<String, TomlValue>;

pub fn parse(source: &str) -> Result<Table, String> {
    let mut table = Table::new();
    let mut prefix = String::new();
    let mut lines = source.lines().enumerate().peekable();
    while let Some((index, raw)) = lines.next() {
        let number = index + 1;
        let mut line = strip_comment(raw).trim().to_string();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            if !line.ends_with(']') || line.starts_with("[[") {
                return Err(format!("line {number}: invalid table header"));
            }
            let name = line[1..line.len() - 1].trim();
            check_key(name, number)?;
            prefix = format!("{name}.");
            continue;
        }
        // A list may continue over several lines until its closing bracket.
        while line.contains('[') && !balanced(&line) {
            match lines.next() {
                Some((_, more)) => {
                    line.push(' ');
                    line.push_str(strip_comment(more).trim());
                }
                None => return Err(format!("line {number}: the list is not closed")),
            }
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {number}: expected key = value"))?;
        let key = key.trim();
        check_key(key, number)?;
        insert(&mut table, format!("{prefix}{key}"), value.trim(), number)?;
    }
    Ok(table)
}

/// Inserts a value under its full key; the members of an inline table under the key of the
/// table and their own.
fn insert(table: &mut Table, full: String, value: &str, number: usize) -> Result<(), String> {
    if let Some(inner) = value.strip_prefix('{') {
        let inner = inner
            .strip_suffix('}')
            .ok_or_else(|| format!("line {number}: the inline table is not closed"))?;
        let mut empty = true;
        for member in split_members(inner) {
            let member = member.trim();
            if member.is_empty() {
                continue;
            }
            empty = false;
            let (key, value) = member.split_once('=').ok_or_else(|| {
                format!("line {number}: expected key = value in the inline table")
            })?;
            let key = key.trim();
            check_key(key, number)?;
            insert(table, format!("{full}.{key}"), value.trim(), number)?;
        }
        if empty {
            return Err(format!("line {number}: the inline table {full} is empty"));
        }
        return Ok(());
    }
    let parsed = parse_value(value).map_err(|e| format!("line {number}: {e}"))?;
    if table.insert(full.clone(), parsed).is_some() {
        return Err(format!("line {number}: {full} is given twice"));
    }
    Ok(())
}

/// The members of an inline table: split at the commas outside texts, lists, and tables.
fn split_members(inner: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut start = 0;
    let mut depth = 0i32;
    let mut in_text = false;
    let mut escaped = false;
    for (i, c) in inner.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if in_text => escaped = true,
            '"' => in_text = !in_text,
            '[' | '{' if !in_text => depth += 1,
            ']' | '}' if !in_text => depth -= 1,
            ',' if !in_text && depth == 0 => {
                items.push(&inner[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    items.push(&inner[start..]);
    items
}

fn strip_comment(line: &str) -> &str {
    let mut in_text = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_text = !in_text,
            '#' if !in_text => return &line[..i],
            _ => {}
        }
    }
    line
}

fn balanced(line: &str) -> bool {
    let mut depth = 0i32;
    let mut in_text = false;
    for c in line.chars() {
        match c {
            '"' => in_text = !in_text,
            '[' if !in_text => depth += 1,
            ']' if !in_text => depth -= 1,
            _ => {}
        }
    }
    depth <= 0
}

fn check_key(key: &str, number: usize) -> Result<(), String> {
    let ok = !key.is_empty()
        && key.split('.').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        });
    if ok {
        Ok(())
    } else {
        Err(format!("line {number}: invalid key '{key}'"))
    }
}

fn parse_text(value: &str) -> Result<String, String> {
    let inner = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .ok_or_else(|| format!("expected a text in double quotes, found {value}"))?;
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                other => return Err(format!("unknown escape \\{}", other.unwrap_or(' '))),
            },
            '"' => return Err("a quote inside a text must be escaped".into()),
            c => out.push(c),
        }
    }
    Ok(out)
}

fn parse_value(value: &str) -> Result<TomlValue, String> {
    if value == "true" {
        return Ok(TomlValue::Bool(true));
    }
    if value == "false" {
        return Ok(TomlValue::Bool(false));
    }
    if value.starts_with('"') {
        return parse_text(value).map(TomlValue::Text);
    }
    if let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
        let mut items = Vec::new();
        for item in split_items(inner) {
            let item = item.trim();
            if item.is_empty() {
                continue;
            }
            items.push(parse_text(item)?);
        }
        return Ok(TomlValue::Texts(items));
    }
    let digits: String = value.chars().filter(|c| *c != '_').collect();
    digits
        .parse::<i64>()
        .map(TomlValue::Int)
        .map_err(|_| format!("unsupported value {value}"))
}

fn split_items(inner: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut start = 0;
    let mut in_text = false;
    let mut escaped = false;
    for (i, c) in inner.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if in_text => escaped = true,
            '"' => in_text = !in_text,
            ',' if !in_text => {
                items.push(&inner[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    items.push(&inner[start..]);
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_dotted_keys_and_values() {
        let table = parse(
            "# comment\n[package]\nname = \"tools\"\n\n[permissions]\nfiles.read = [\"$CWD/**\", \"a\"] # trailing\nnetwork = false\n[limits]\nheap = 1_000\n",
        )
        .unwrap();
        assert_eq!(table["package.name"], TomlValue::Text("tools".into()));
        assert_eq!(
            table["permissions.files.read"],
            TomlValue::Texts(vec!["$CWD/**".into(), "a".into()])
        );
        assert_eq!(table["permissions.network"], TomlValue::Bool(false));
        assert_eq!(table["limits.heap"], TomlValue::Int(1000));
    }

    #[test]
    fn lists_over_several_lines() {
        let table = parse("env = [\n  \"HOME\",\n  \"TODO_FILE\", # why\n]\n").unwrap();
        assert_eq!(
            table["env"],
            TomlValue::Texts(vec!["HOME".into(), "TODO_FILE".into()])
        );
    }

    #[test]
    fn inline_tables_become_dotted_keys() {
        let table = parse(
            "[dependencies]\nutil = { path = \"../u, v\", version = \"^1\" }\njson = { registry = \"main\", package = \"acme/json\", nested = { list = [\"a\", \"b\"] } }\n",
        )
        .unwrap();
        assert_eq!(
            table["dependencies.util.path"],
            TomlValue::Text("../u, v".into())
        );
        assert_eq!(
            table["dependencies.util.version"],
            TomlValue::Text("^1".into())
        );
        assert_eq!(
            table["dependencies.json.nested.list"],
            TomlValue::Texts(vec!["a".into(), "b".into()])
        );
        assert!(parse("a = { b = 1\n").is_err());
        assert!(parse("a = { }\n").is_err());
        assert!(parse("a = { b = 1, b = 2 }\n").is_err());
        assert!(parse("a = { b }\n").is_err());
    }

    #[test]
    fn mistakes_are_errors() {
        assert!(parse("[permissions\n").is_err());
        assert!(parse("files.read = [1, 2]\n").is_err());
        assert!(parse("a = 1\na = 2\n").is_err());
        assert!(parse("just words\n").is_err());
        assert!(parse("env = [\"HOME\"\n").is_err());
        assert!(parse("key with space = 1\n").is_err());
    }
}
