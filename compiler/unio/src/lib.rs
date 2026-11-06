//! The Unio compiler front end: lexer, parser, syntax tree, diagnostics and their catalog
//! (requirements C-1, C-2, C-6 to C-10, R-12), the formatter (T-1 to T-3), the outline
//! (C-12), and name and type checks (docs/semantics.md, sections 1–4). Errors, resources,
//! and code generation are later stages of the plan.

pub mod ast;
pub mod catalog;
pub mod check;
pub mod diag;
pub mod format;
pub mod lexer;
pub mod outline;
pub mod parser;
pub mod program;
pub mod sexpr;
pub mod types;

pub use diag::{explanation, Diagnostic, Lang, Msg, Severity, Span};

/// The result of checking one source file.
pub struct Analysis {
    pub path: String,
    pub source: String,
    /// Lexical diagnostics first, in the order found, then syntax diagnostics by position.
    pub diagnostics: Vec<Diagnostic>,
    pub comments: Vec<lexer::Comment>,
    pub file: ast::File,
}

impl Analysis {
    pub fn render_text(&self, lang: Lang) -> String {
        diag::render_text(&self.diagnostics, &self.path, &self.source, lang)
    }

    pub fn render_json(&self, lang: Lang) -> String {
        diag::render_json(&self.diagnostics, &self.path, lang)
    }

    /// The tree as an S-expression without spans; None if the file has errors.
    pub fn ast_sexpr(&self) -> Option<String> {
        if self.diagnostics.is_empty() {
            Some(sexpr::file(&self.file))
        } else {
            None
        }
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }
}

/// Checks the files and the modules they import: syntax, then names and types. Returns one
/// analysis for every given file and every imported module of the same packages; the
/// standard library is checked but reported only when it is given.
pub fn check_files(files: &[std::path::PathBuf]) -> Vec<Analysis> {
    let files = files.to_vec();
    on_front_end_stack(move || {
        let mut program = program::Program::load(&files);
        let syntax_ok: Vec<bool> = program
            .modules
            .iter()
            .map(|m| !m.analysis.has_errors())
            .collect();
        if syntax_ok.iter().all(|ok| *ok) {
            check::check_program(&mut program);
        } else {
            // Names and types are checked only when every module parses; otherwise the
            // syntax errors come first.
            program.diagnostics.clear();
        }
        let semantic = std::mem::take(&mut program.diagnostics);
        let mut modules = std::mem::take(&mut program.modules);
        for (module, diagnostic) in semantic {
            let list = &mut modules[module].analysis.diagnostics;
            if !list.contains(&diagnostic) {
                list.push(diagnostic);
            }
        }
        modules
            .into_iter()
            .filter(|m| m.report)
            .map(|mut m| {
                let lexical_or_syntax = m.analysis.has_errors();
                let _ = lexical_or_syntax;
                m.analysis
                    .diagnostics
                    .sort_by_key(|d| (d.span.start.line, d.span.start.column));
                m.analysis
            })
            .collect()
    })
}

/// The stack of the thread that runs the front end. Parsing recurses once per nesting
/// level, which E0013 bounds; the stack is fixed here so that the bound holds whatever
/// thread calls the compiler (R-1).
const STACK_SIZE: usize = 64 * 1024 * 1024;

/// Lexes and parses a source file and collects its diagnostics.
pub fn check_source(path: &str, source: &str) -> Analysis {
    let (path, source) = (path.to_string(), source.to_string());
    on_front_end_stack(move || analyse(&path, &source))
}

/// Runs the work on a thread with the stack of the front end.
pub(crate) fn on_front_end_stack<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> T {
    std::thread::Builder::new()
        .name("unio-front-end".to_string())
        .stack_size(STACK_SIZE)
        .spawn(work)
        .expect("the front-end thread starts")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

pub(crate) fn analyse(path: &str, source: &str) -> Analysis {
    let lexed = lexer::lex(source);
    let mut parser = parser::Parser::new(&lexed.tokens);
    let file = parser.file();
    let mut syntax = std::mem::take(&mut parser.diagnostics);
    syntax.sort_by_key(|d| d.span.start.offset);
    // A lexical error usually causes syntax errors next to it; report only the first.
    let lexical = lexed.diagnostics;
    let swallowed_after = lexical
        .iter()
        .filter(|d| d.code == "E0009" || d.code == "E0005")
        .map(|d| d.span.start.offset)
        .min();
    let mut seen_lines = Vec::new();
    syntax.retain(|d| {
        let line = d.line();
        let near_lexical = lexical
            .iter()
            .any(|l| line + 1 >= l.line() && line <= l.line() + 1);
        let after_swallow = swallowed_after.is_some_and(|offset| d.span.start.offset >= offset);
        let repeated = seen_lines.contains(&line);
        seen_lines.push(line);
        !near_lexical && !after_swallow && !repeated
    });
    let mut diagnostics = lexical;
    diagnostics.extend(syntax);
    Analysis {
        path: path.to_string(),
        source: source.to_string(),
        diagnostics,
        comments: lexed.comments,
        file,
    }
}
