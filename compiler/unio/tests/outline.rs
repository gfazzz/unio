//! `unio outline`: the declarations of a file with their signatures (C-12).

use unio::outline::{outline_json, outline_text};

const SOURCE: &str = "import std.text

## A point.
public struct Point:
    public x as Int
    y as Int

enum Shape:
    Circle(radius as Float)
    Empty

error Missing:
    path as Path

interface Display:
    function to_text(self) returns Text

let LIMIT as Int = 10

public function area[T with Shape](shape as T, named scale as Float = 1.0) returns Float fails with Missing:
    return 0.0

extend Point with Display:
    function to_text(self) returns Text:
        return \"p\"

test \"the area\":
    assert true
";

#[test]
fn text_lists_declarations_with_signatures_and_lines() {
    let analysis = unio::check_source("a.unio", SOURCE);
    assert!(analysis.diagnostics.is_empty());
    let text = outline_text(&analysis.file);
    let expected = "\
4: public struct Point
    5: public x as Int
    6: y as Int
8: enum Shape
    9: Circle(radius as Float)
    10: Empty
12: error Missing
    13: path as Path
15: interface Display
    16: function to_text(self) returns Text
18: let LIMIT as Int
20: public function area[T with Shape](shape as T, named scale as Float = 1.0) returns Float fails with Missing
23: extend Point with Display
    24: function to_text(self) returns Text
27: test \"the area\"
";
    assert_eq!(text, expected);
}

#[test]
fn json_has_kind_name_signature_line_and_members() {
    let analysis = unio::check_source("a.unio", SOURCE);
    let json = outline_json(&analysis.file);
    assert!(json.starts_with('['), "{json}");
    assert!(json.contains("\"kind\": \"function\""), "{json}");
    assert!(json.contains("\"name\": \"area\""), "{json}");
    assert!(
        json.contains("\"signature\": \"public function area[T with Shape](shape as T, named scale as Float = 1.0) returns Float fails with Missing\""),
        "{json}"
    );
    assert!(json.contains("\"line\": 20"), "{json}");
    assert!(json.contains("\"members\": ["), "{json}");
}
