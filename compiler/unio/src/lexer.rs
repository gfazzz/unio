//! The lexer: section L of spec/grammar.ebnf.
//!
//! It produces NEWLINE, INDENT and DEDENT, ignores line ends inside brackets, keeps comments
//! as trivia, and lexes interpolations of text recursively. Unlike the reference checker it
//! does not stop at the first error: it reports the error and continues, so that one run
//! shows several problems (C-2).

use crate::catalog;
use crate::diag::{Diagnostic, Fix, Msg, Pos, Span};

/// Keywords of section L. Contextual keywords are lexed as names.
pub const KEYWORDS: &[&str] = &[
    "and",
    "as",
    "assert",
    "break",
    "case",
    "continue",
    "else",
    "enum",
    "extend",
    "fail",
    "fails",
    "false",
    "for",
    "from",
    "function",
    "if",
    "import",
    "in",
    "interface",
    "is",
    "let",
    "match",
    "not",
    "on",
    "or",
    "pass",
    "public",
    "return",
    "returns",
    "self",
    "struct",
    "true",
    "try",
    "using",
    "var",
    "while",
    "with",
];

/// Names that are keywords only where a rule uses them.
pub const CONTEXTUAL: &[&str] = &["error", "external", "module", "named", "test", "unsafe"];

/// Words reserved for later releases (E0032).
pub const RESERVED: &[&str] = &["uses"];

/// Punctuation, longest first.
const PUNCTUATION: &[&str] = &[
    "...", "?.", "==", "!=", "<=", ">=", "**", "->", "(", ")", "[", "]", "{", "}", ",", ":", ".",
    "=", "<", ">", "+", "-", "*", "/", "%", "|",
];

/// How deep texts may nest inside interpolations.
const MAX_TEXT_DEPTH: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Name,
    Int,
    Float,
    Text,
    Keyword(&'static str),
    Symbol(&'static str),
    Newline,
    Indent,
    Dedent,
    Eof,
}

/// A piece of a text literal: decoded characters or an interpolated expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextPiece {
    Literal(String),
    /// The tokens of the expression, ending with an EOF token at the closing brace.
    Interpolation(Vec<Token>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: Kind,
    /// The spelling of names and numbers; empty for other tokens.
    pub text: String,
    pub span: Span,
    /// The pieces of a TEXT token.
    pub pieces: Vec<TextPiece>,
}

impl Token {
    fn new(kind: Kind, text: String, span: Span) -> Token {
        Token {
            kind,
            text,
            span,
            pieces: Vec::new(),
        }
    }

    /// What the token is, for messages (E0012).
    pub fn describe(&self) -> Msg {
        match &self.kind {
            Kind::Name => catalog::token_name(&self.text),
            Kind::Int | Kind::Float => catalog::token_number(&self.text),
            Kind::Text => catalog::token_text(),
            Kind::Keyword(k) => catalog::quoted(k),
            Kind::Symbol(s) => catalog::quoted(s),
            Kind::Newline => catalog::token_line_end(),
            Kind::Indent => catalog::token_block_start(),
            Kind::Dedent => catalog::token_block_end(),
            Kind::Eof => catalog::token_file_end(),
        }
    }
}

/// A comment, kept for the formatter and for documentation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub span: Span,
    pub text: String,
    pub doc: bool,
}

pub struct Lexed {
    pub tokens: Vec<Token>,
    pub comments: Vec<Comment>,
    pub diagnostics: Vec<Diagnostic>,
}

struct Lexer {
    chars: Vec<char>,
    /// Byte offset of each character, and of the end.
    offsets: Vec<usize>,
    pos: usize,
    line: usize,
    column: usize,
    comments: Vec<Comment>,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Copy)]
struct Level {
    width: usize,
    /// False for indentation accepted after an E0003 without opening a block.
    real: bool,
}

/// Lexes a whole file.
pub fn lex(source: &str) -> Lexed {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let chars: Vec<char> = source.chars().collect();
    let mut offsets = Vec::with_capacity(chars.len() + 1);
    let mut offset = 0;
    for c in &chars {
        offsets.push(offset);
        offset += c.len_utf8();
    }
    offsets.push(offset);
    let mut lexer = Lexer {
        chars,
        offsets,
        pos: 0,
        line: 1,
        column: 1,
        comments: Vec::new(),
        diagnostics: Vec::new(),
    };
    let tokens = lexer.file();
    Lexed {
        tokens,
        comments: lexer.comments,
        diagnostics: lexer.diagnostics,
    }
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

impl Lexer {
    fn here(&self) -> Pos {
        Pos {
            offset: self.offsets[self.pos.min(self.chars.len())],
            line: self.line,
            column: self.column,
        }
    }

    fn peek(&self, k: usize) -> Option<char> {
        self.chars.get(self.pos + k).copied()
    }

    fn advance(&mut self) {
        if let Some(c) = self.chars.get(self.pos) {
            if *c == '\n' {
                self.line += 1;
                self.column = 1;
            } else {
                self.column += 1;
            }
            self.pos += 1;
        }
    }

    fn error(&mut self, code: &'static str, start: Pos, end: Pos, message: Msg) {
        self.diagnostics
            .push(Diagnostic::error(code, Span::new(start, end), message));
    }

    fn file(&mut self) -> Vec<Token> {
        let mut out: Vec<Token> = Vec::new();
        let mut levels = vec![Level {
            width: 0,
            real: true,
        }];
        // Whether the last logical line ended with ':'.
        let mut ends_with_colon = false;
        while self.pos < self.chars.len() {
            let line_start = self.here();
            let mut width = 0;
            let mut tab_reported = false;
            while let Some(c) = self.peek(0) {
                if c == ' ' {
                    width += 1;
                } else if c == '\t' {
                    if !tab_reported {
                        let start = self.here();
                        let mut end = start;
                        end.offset += 1;
                        end.column += 1;
                        self.diagnostics.push(
                            Diagnostic::error(
                                "E0001",
                                Span::new(start, end),
                                catalog::tab_in_indentation(),
                            )
                            .with_fix(Fix {
                                span: Span::new(start, end),
                                new_text: "    ".to_string(),
                                description: catalog::replace_tab(),
                            }),
                        );
                        tab_reported = true;
                    }
                    width = width - width % 4 + 4;
                } else {
                    break;
                }
                self.advance();
            }
            match self.peek(0) {
                None | Some('\n') | Some('\r') | Some('#') => {
                    self.rest_of_line();
                    continue;
                }
                _ => {}
            }
            let line_pos = Pos {
                offset: line_start.offset,
                line: line_start.line,
                column: 1,
            };
            let first = self.here();
            if width % 4 != 0 {
                self.error(
                    "E0002",
                    line_pos,
                    first,
                    catalog::indentation_not_multiple(),
                );
                width -= width % 4;
            }
            let top = levels.last().map_or(0, |l| l.width);
            if width > top {
                if width != top + 4 {
                    self.error(
                        "E0003",
                        line_pos,
                        first,
                        catalog::indentation_grows_too_much(),
                    );
                    levels.push(Level { width, real: false });
                } else if !ends_with_colon {
                    self.error("E0003", line_pos, first, catalog::unexpected_indentation());
                    levels.push(Level { width, real: false });
                } else {
                    levels.push(Level { width, real: true });
                    out.push(Token::new(
                        Kind::Indent,
                        String::new(),
                        Span::new(line_pos, first),
                    ));
                }
            } else {
                while width < levels.last().map_or(0, |l| l.width) {
                    let level = levels.pop().expect("a level");
                    if level.real {
                        out.push(Token::new(
                            Kind::Dedent,
                            String::new(),
                            Span::new(first, first),
                        ));
                    }
                }
                if width > levels.last().map_or(0, |l| l.width) {
                    // Only after an earlier E0003; keep the line in the enclosing block.
                    levels.push(Level { width, real: false });
                }
            }
            let before = out.len();
            self.logical_line(&mut out);
            ends_with_colon =
                out.len() >= before + 2 && out[out.len() - 2].kind == Kind::Symbol(":");
        }
        let end = self.here();
        while levels.len() > 1 {
            if levels.pop().expect("a level").real {
                out.push(Token::new(Kind::Dedent, String::new(), Span::new(end, end)));
            }
        }
        out.push(Token::new(Kind::Eof, String::new(), Span::new(end, end)));
        out
    }

    /// Skips a blank or comment-only line.
    fn rest_of_line(&mut self) {
        if self.peek(0) == Some('#') {
            self.comment();
        }
        while let Some(c) = self.peek(0) {
            self.advance();
            if c == '\n' {
                break;
            }
        }
    }

    fn comment(&mut self) {
        let start = self.here();
        let from = self.pos;
        while !matches!(self.peek(0), None | Some('\n')) {
            self.advance();
        }
        let text: String = self.chars[from..self.pos]
            .iter()
            .collect::<String>()
            .trim_end_matches('\r')
            .to_string();
        let doc = text.starts_with("##");
        self.comments.push(Comment {
            span: Span::new(start, self.here()),
            text,
            doc,
        });
    }

    fn logical_line(&mut self, out: &mut Vec<Token>) {
        let mut depth: Vec<(char, Pos)> = Vec::new();
        loop {
            match self.peek(0) {
                None => {
                    if let Some((open, pos)) = depth.last().copied() {
                        let mut end = pos;
                        end.offset += 1;
                        end.column += 1;
                        self.error("E0009", pos, end, catalog::unclosed(open));
                    }
                    break;
                }
                Some(' ') | Some('\t') => self.advance(),
                Some('\r') if self.peek(1) == Some('\n') => self.advance(),
                Some('#') => self.comment(),
                Some('\n') => {
                    if depth.is_empty() {
                        let pos = self.here();
                        out.push(Token::new(
                            Kind::Newline,
                            String::new(),
                            Span::new(pos, pos),
                        ));
                        self.advance();
                        return;
                    }
                    self.advance();
                }
                Some(_) => {
                    if let Some(token) = self.token(&mut depth, 0) {
                        out.push(token);
                    }
                }
            }
        }
        let pos = self.here();
        out.push(Token::new(
            Kind::Newline,
            String::new(),
            Span::new(pos, pos),
        ));
    }

    /// One token, or None after an error that dropped the characters.
    fn token(&mut self, depth: &mut Vec<(char, Pos)>, text_depth: usize) -> Option<Token> {
        let start = self.here();
        let c = self.peek(0)?;
        if is_name_start(c) {
            let from = self.pos;
            while self.peek(0).is_some_and(is_name_char) {
                self.advance();
            }
            let word: String = self.chars[from..self.pos].iter().collect();
            let span = Span::new(start, self.here());
            if RESERVED.contains(&word.as_str()) {
                self.diagnostics
                    .push(Diagnostic::error("E0032", span, catalog::reserved(&word)));
            }
            let kind = match KEYWORDS.iter().find(|k| **k == word) {
                Some(k) => Kind::Keyword(k),
                None => Kind::Name,
            };
            return Some(Token::new(kind, word, span));
        }
        if c.is_ascii_digit() {
            return Some(self.number(start));
        }
        if c == '"' {
            return Some(self.text(start, text_depth));
        }
        for p in PUNCTUATION {
            let matches = p
                .chars()
                .enumerate()
                .all(|(k, pc)| self.peek(k) == Some(pc));
            if !matches {
                continue;
            }
            for _ in 0..p.chars().count() {
                self.advance();
            }
            let span = Span::new(start, self.here());
            let opening = p.chars().next().expect("a character");
            match *p {
                "(" | "[" | "{" => depth.push((opening, start)),
                ")" | "]" | "}" => {
                    let expected = match *p {
                        ")" => '(',
                        "]" => '[',
                        _ => '{',
                    };
                    match depth.last().copied() {
                        None => {
                            self.diagnostics.push(Diagnostic::error(
                                "E0010",
                                span,
                                catalog::unmatched(p),
                            ));
                            return None;
                        }
                        Some((open, _)) if open != expected => {
                            self.diagnostics.push(Diagnostic::error(
                                "E0010",
                                span,
                                catalog::mismatched(p, open),
                            ));
                            // `[g(1, 2]`: the bracket closes its own opening bracket, and the
                            // message names the one left open between them, so that the
                            // opening bracket is not reported as unclosed too.
                            match depth.iter().rposition(|(o, _)| *o == expected) {
                                Some(i) => depth.truncate(i),
                                None => {
                                    depth.pop();
                                }
                            }
                        }
                        Some(_) => {
                            depth.pop();
                        }
                    }
                }
                _ => {}
            }
            return Some(Token::new(Kind::Symbol(p), String::new(), span));
        }
        self.advance();
        // A run of one invalid character, such as `&&`, is one mistake.
        while self.peek(0) == Some(c) {
            self.advance();
        }
        let shown = if c.is_control() {
            format!("U+{:04X}", c as u32)
        } else {
            c.to_string()
        };
        self.diagnostics.push(Diagnostic::error(
            "E0007",
            Span::new(start, self.here()),
            catalog::invalid_character(&shown),
        ));
        None
    }

    fn take_while(&mut self, accept: impl Fn(char) -> bool) -> usize {
        let mut n = 0;
        while self.peek(0).is_some_and(&accept) {
            self.advance();
            n += 1;
        }
        n
    }

    fn number(&mut self, start: Pos) -> Token {
        let from = self.pos;
        let mut kind = Kind::Int;
        let prefixed = |l: &Lexer, p: char, digit: fn(char) -> bool| {
            l.peek(0) == Some('0') && l.peek(1) == Some(p) && l.peek(2).is_some_and(digit)
        };
        if prefixed(self, 'x', |c| c.is_ascii_hexdigit()) {
            self.advance();
            self.advance();
            self.take_while(|c| c.is_ascii_hexdigit() || c == '_');
        } else if prefixed(self, 'b', |c| c == '0' || c == '1') {
            self.advance();
            self.advance();
            self.take_while(|c| c == '0' || c == '1' || c == '_');
        } else {
            self.take_while(|c| c.is_ascii_digit() || c == '_');
            if self.peek(0) == Some('.') && self.peek(1).is_some_and(|c| c.is_ascii_digit()) {
                kind = Kind::Float;
                self.advance();
                self.take_while(|c| c.is_ascii_digit() || c == '_');
            }
            let exponent_digit =
                |k: usize, l: &Lexer| l.peek(k).is_some_and(|c| c.is_ascii_digit());
            if matches!(self.peek(0), Some('e') | Some('E'))
                && (exponent_digit(1, self)
                    || (matches!(self.peek(1), Some('+') | Some('-')) && exponent_digit(2, self)))
            {
                kind = Kind::Float;
                self.advance();
                if matches!(self.peek(0), Some('+') | Some('-')) {
                    self.advance();
                }
                self.take_while(|c| c.is_ascii_digit() || c == '_');
            }
        }
        let mut word: String = self.chars[from..self.pos].iter().collect();
        if self.peek(0).is_some_and(is_name_char) {
            self.take_while(is_name_char);
            let bad: String = self.chars[from..self.pos].iter().collect();
            self.diagnostics.push(Diagnostic::error(
                "E0008",
                Span::new(start, self.here()),
                catalog::invalid_number(&bad),
            ));
            word = "0".to_string();
            kind = Kind::Int;
        }
        Token::new(kind, word, Span::new(start, self.here()))
    }

    /// A text literal starting at '"'. On an error the literal ends where the error is.
    fn text(&mut self, start: Pos, text_depth: usize) -> Token {
        self.advance();
        let mut pieces: Vec<TextPiece> = Vec::new();
        let mut literal = String::new();
        if text_depth >= MAX_TEXT_DEPTH {
            self.diagnostics.push(Diagnostic::error(
                "E0013",
                Span::new(start, self.here()),
                catalog::texts_nested_too_deeply(),
            ));
            while !matches!(self.peek(0), None | Some('\n')) {
                self.advance();
            }
            return self.finish_text(start, pieces, literal);
        }
        loop {
            let here = self.here();
            match self.peek(0) {
                None | Some('\n') => {
                    self.error("E0005", start, here, catalog::unterminated_text());
                    return self.finish_text(start, pieces, literal);
                }
                Some('"') => {
                    self.advance();
                    return self.finish_text(start, pieces, literal);
                }
                Some('\\') => {
                    let next = self.peek(1);
                    if next == Some('u') && self.peek(2) == Some('{') {
                        let mut k = 3;
                        let mut value: u32 = 0;
                        while k < 9 && self.peek(k).is_some_and(|c| c.is_ascii_hexdigit()) {
                            value =
                                value * 16 + self.peek(k).and_then(|c| c.to_digit(16)).unwrap_or(0);
                            k += 1;
                        }
                        if k > 3 && self.peek(k) == Some('}') {
                            for _ in 0..=k {
                                self.advance();
                            }
                            literal.push(char::from_u32(value).unwrap_or('\u{fffd}'));
                        } else {
                            self.advance();
                            self.advance();
                            self.error(
                                "E0006",
                                here,
                                self.here(),
                                catalog::invalid_unicode_escape(),
                            );
                        }
                        continue;
                    }
                    let decoded = match next {
                        Some('n') => Some('\n'),
                        Some('t') => Some('\t'),
                        Some('r') => Some('\r'),
                        Some('0') => Some('\0'),
                        Some('\\') => Some('\\'),
                        Some('"') => Some('"'),
                        Some('{') => Some('{'),
                        Some('}') => Some('}'),
                        _ => None,
                    };
                    self.advance();
                    match decoded {
                        Some(c) => {
                            self.advance();
                            literal.push(c);
                        }
                        None => {
                            let shown = next
                                .filter(|c| *c != '\n')
                                .map(String::from)
                                .unwrap_or_default();
                            if next.is_some_and(|c| c != '\n') {
                                self.advance();
                            }
                            self.error("E0006", here, self.here(), catalog::invalid_escape(&shown));
                        }
                    }
                }
                Some('}') => {
                    self.advance();
                    self.error("E0011", here, self.here(), catalog::brace_in_text());
                }
                Some('{') => {
                    if !literal.is_empty() {
                        pieces.push(TextPiece::Literal(std::mem::take(&mut literal)));
                    }
                    match self.interpolation(text_depth) {
                        Ok(Some(tokens)) => pieces.push(TextPiece::Interpolation(tokens)),
                        Ok(None) => {}
                        Err(()) => return self.finish_text(start, pieces, literal),
                    }
                }
                Some(c) => {
                    literal.push(c);
                    self.advance();
                }
            }
        }
    }

    fn finish_text(&mut self, start: Pos, mut pieces: Vec<TextPiece>, literal: String) -> Token {
        if !literal.is_empty() || pieces.is_empty() {
            pieces.push(TextPiece::Literal(literal));
        }
        let mut token = Token::new(Kind::Text, String::new(), Span::new(start, self.here()));
        token.pieces = pieces;
        token
    }

    /// `{ expression }` inside text. Ok(None) for an empty interpolation; Err when the line
    /// ended first, which also ends the text.
    fn interpolation(&mut self, text_depth: usize) -> Result<Option<Vec<Token>>, ()> {
        let open = self.here();
        self.advance();
        let mut depth = vec![('{', open)];
        let mut tokens = Vec::new();
        loop {
            while self.peek(0) == Some(' ') {
                self.advance();
            }
            match self.peek(0) {
                None | Some('\n') => {
                    self.error(
                        "E0011",
                        open,
                        self.here(),
                        catalog::unterminated_interpolation(),
                    );
                    return Err(());
                }
                Some('}') if depth.len() == 1 => {
                    let close = self.here();
                    self.advance();
                    if tokens.is_empty() {
                        self.error("E0011", open, self.here(), catalog::empty_interpolation());
                        return Ok(None);
                    }
                    tokens.push(Token::new(
                        Kind::Eof,
                        String::new(),
                        Span::new(close, close),
                    ));
                    return Ok(Some(tokens));
                }
                Some(_) => {
                    if let Some(token) = self.token(&mut depth, text_depth + 1) {
                        tokens.push(token);
                    }
                }
            }
        }
    }
}
