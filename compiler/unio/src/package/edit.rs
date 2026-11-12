//! Changes of `[dependencies]` in the text of `unio.toml` by `unio add` and `unio remove`,
//! keeping every other line, comment, and the order as the author wrote them.

fn header(line: &str) -> Option<String> {
    let line = line.split('#').next().unwrap_or("").trim();
    line.strip_prefix('[')
        .and_then(|l| l.strip_suffix(']'))
        .map(|name| name.trim().to_string())
}

fn key(line: &str) -> Option<String> {
    let line = line.split('#').next().unwrap_or("").trim();
    let (key, _) = line.split_once('=')?;
    Some(key.trim().to_string())
}

/// A text in double quotes, as the manifest parser reads it.
pub fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The text with the line `alias = value` at the end of `[dependencies]`, which is added at
/// the end when there is none.
pub fn add(text: &str, alias: &str, value: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let entry = format!("{alias} = {value}");
    let start = lines
        .iter()
        .position(|l| header(l).as_deref() == Some("dependencies"));
    let Some(start) = start else {
        let mut out = text.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("[dependencies]\n{entry}\n"));
        return out;
    };
    // After the last line of the section that is not empty.
    let mut end = start + 1;
    let mut last = start;
    while end < lines.len() && header(lines[end]).is_none() {
        if !lines[end].trim().is_empty() {
            last = end;
        }
        end += 1;
    }
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    out.insert(last + 1, entry);
    out.join("\n") + "\n"
}

/// The text without the dependency `alias`, written as a line of `[dependencies]` (also as
/// dotted keys) or as its own table `[dependencies.alias]`; `None` when it is not there.
pub fn remove(text: &str, alias: &str) -> Option<String> {
    let mut out = Vec::new();
    let mut section = String::new();
    let mut removed = false;
    for line in text.lines() {
        if let Some(name) = header(line) {
            section = name;
            if section == format!("dependencies.{alias}") {
                removed = true;
                continue;
            }
            out.push(line);
            continue;
        }
        if section == format!("dependencies.{alias}") {
            continue;
        }
        if section == "dependencies" {
            if let Some(k) = key(line) {
                let k = k.trim_matches('"');
                if k == alias || k.starts_with(&format!("{alias}.")) {
                    removed = true;
                    continue;
                }
            }
        }
        out.push(line);
    }
    removed.then(|| out.join("\n") + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_remove_keep_the_rest() {
        let text = "[package]\nname = \"app\" # the app\n\n[dependencies]\nold = { path = \"../old\" }\n\n[permissions]\nenv = []\n";
        let added = add(text, "util", "{ path = \"../util\" }");
        assert_eq!(
            added,
            "[package]\nname = \"app\" # the app\n\n[dependencies]\nold = { path = \"../old\" }\nutil = { path = \"../util\" }\n\n[permissions]\nenv = []\n"
        );
        assert_eq!(remove(&added, "util").unwrap(), text);
        assert!(remove(text, "missing").is_none());
        let fresh = add("[package]\nname = \"app\"", "u", "{ path = \"u\" }");
        assert_eq!(
            fresh,
            "[package]\nname = \"app\"\n\n[dependencies]\nu = { path = \"u\" }\n"
        );
        let table = "[dependencies.u]\npath = \"u\"\n[permissions]\nenv = []\n";
        assert_eq!(remove(table, "u").unwrap(), "[permissions]\nenv = []\n");
        let dotted = "[dependencies]\nu.path = \"u\"\nv.path = \"v\"\n";
        assert_eq!(
            remove(dotted, "u").unwrap(),
            "[dependencies]\nv.path = \"v\"\n"
        );
    }
}
