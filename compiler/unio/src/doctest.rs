//! Documentation tests (T-13, docs/language-contracts.md section 5): the examples of the
//! documentation comments of a module run as tests under `unio test`.
//!
//! An example is a block of `##` lines indented by four spaces after `## `, below a line
//! `## Example:` or `## Examples:` of the same documentation comment; blocks separated by an
//! empty `##` line or a line of text are separate examples. Each example becomes a `test`
//! block appended to the module, named after the declaration the comment documents, so it is
//! checked with the module (a statement of an example sees what a test block of the module
//! sees) and runs in the sandbox of the package like any other test. Example:
//!
//! ```text
//! ## Returns the sum.
//! ##
//! ## Example:
//! ##     assert add(2, 3) == 5
//! public function add(a as Int, b as Int) returns Int:
//! ```

/// One example of a documentation comment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Example {
    /// The declaration the comment documents, or `module` for the comment of the module.
    pub declaration: String,
    /// The line of the first statement of the example, from 1.
    pub line: usize,
    /// The statements, without the comment marker and the four spaces.
    pub code: Vec<String>,
}

/// The text after `##` of a documentation comment line, or `None` for another line.
fn doc_text(line: &str) -> Option<&str> {
    let rest = line.trim_start().strip_prefix("##")?;
    if rest.starts_with('#') {
        return None;
    }
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

/// The name a declaration line declares.
fn declared_name(line: &str) -> Option<String> {
    let mut words = line
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty());
    for word in words.by_ref() {
        match word {
            "public" | "external" | "function" | "struct" | "enum" | "error" | "interface"
            | "unsafe" => continue,
            "extend" => return words.next().map(|w| format!("extend {w}")),
            "test" | "import" | "from" | "module" => return None,
            name => return Some(name.to_string()),
        }
    }
    None
}

/// The examples of the documentation comments of a source.
pub fn examples(source: &str) -> Vec<Example> {
    let lines: Vec<&str> = source.lines().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if doc_text(lines[i]).is_none() {
            i += 1;
            continue;
        }
        let start = i;
        while i < lines.len() && doc_text(lines[i]).is_some() {
            i += 1;
        }
        // The documented declaration: the next line that is not blank.
        let declaration = lines[i..]
            .iter()
            .find(|l| !l.trim().is_empty())
            .and_then(|l| declared_name(l))
            .unwrap_or_else(|| "module".to_string());
        let mut in_examples = false;
        let mut current: Option<Example> = None;
        for (k, line) in lines.iter().enumerate().take(i).skip(start) {
            let text = doc_text(line).unwrap_or("");
            let heading = text.trim();
            if heading.eq_ignore_ascii_case("example:") || heading.eq_ignore_ascii_case("examples:")
            {
                in_examples = true;
                found.extend(current.take());
                continue;
            }
            match text.strip_prefix("    ") {
                Some(code) if in_examples => {
                    let example = current.get_or_insert_with(|| Example {
                        declaration: declaration.clone(),
                        line: k + 1,
                        code: Vec::new(),
                    });
                    example.code.push(code.to_string());
                }
                _ => found.extend(current.take()),
            }
        }
        found.extend(current.take());
    }
    found
}

fn escape(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if matches!(c, '"' | '\\' | '{' | '}') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The name of the test of an example.
pub fn test_name(example: &Example) -> String {
    format!("doc: {} (line {})", example.declaration, example.line)
}

/// The source with the examples appended as `test` blocks, and for every appended line its
/// line in the original source; `None` when the source has no example.
pub fn augment(source: &str) -> Option<(String, Vec<(usize, usize)>)> {
    let found = examples(source);
    if found.is_empty() {
        return None;
    }
    let mut text = source.to_string();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    let mut line = text.lines().count();
    let mut map = Vec::new();
    for example in &found {
        text.push('\n');
        line += 1;
        text.push_str(&format!("test \"{}\":\n", escape(&test_name(example))));
        line += 1;
        map.push((line, example.line));
        for (k, code) in example.code.iter().enumerate() {
            text.push_str("    ");
            text.push_str(code);
            text.push('\n');
            line += 1;
            map.push((line, example.line + k));
        }
    }
    Some((text, map))
}

/// The line of the original source for a line of the augmented one.
pub fn original_line(map: &[(usize, usize)], line: usize) -> usize {
    map.iter()
        .find(|(augmented, _)| *augmented == line)
        .map_or(line, |(_, original)| *original)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "## Adds.
##
## Examples:
##     assert add(2, 3) == 5
##     assert add(0, 0) == 0
##
##     let x = add(1, 1)
##     assert x == 2
public function add(a as Int, b as Int) returns Int:
    return a + b

## Usage:
##     add A B
function other():
    pass
";

    #[test]
    fn examples_are_the_indented_blocks_below_the_heading() {
        let found = examples(SOURCE);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].declaration, "add");
        assert_eq!(found[0].line, 4);
        assert_eq!(
            found[0].code,
            vec!["assert add(2, 3) == 5", "assert add(0, 0) == 0"]
        );
        assert_eq!(found[1].line, 7);
        assert_eq!(test_name(&found[1]), "doc: add (line 7)");
    }

    #[test]
    fn the_augmented_source_keeps_its_lines_and_maps_the_new_ones() {
        let (text, map) = augment(SOURCE).unwrap();
        assert!(text.starts_with(SOURCE));
        let lines: Vec<&str> = text.lines().collect();
        let first = map[1].0;
        assert_eq!(lines[first - 1], "    assert add(2, 3) == 5");
        assert_eq!(original_line(&map, first), 4);
        assert_eq!(original_line(&map, 3), 3);
        assert!(augment("function f():\n    pass\n").is_none());
    }
}
