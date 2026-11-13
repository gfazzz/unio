//! The Unio compiler front end: lexer, parser, syntax tree, diagnostics and their catalog
//! (requirements C-1, C-2, C-6 to C-10, R-12), the formatter (T-1 to T-3), the outline
//! (C-12), the static checks of docs/semantics.md (names, types, errors, values, resources,
//! layers, profiles and effects), and `unio symbol` with content hashes (C-11 to C-11b).
//! The interpreter of docs/runtime.md runs programs for `unio run` and `unio test`, and
//! `native` compiles them to an executable for `unio build` (docs/runtime.md 8.4).

pub mod api;
pub mod ast;
pub mod catalog;
pub mod check;
pub mod diag;
pub mod doctest;
pub mod effects;
pub mod envelope;
pub mod format;
pub mod interp;
pub mod lexer;
mod lower;
pub mod lsp;
pub mod migrate;
pub mod native;
pub mod outline;
pub mod package;
pub mod parser;
pub mod program;
pub mod release;
pub mod repl;
pub mod sandbox_cli;
pub mod sexpr;
pub mod sha;
pub mod symbol;
pub mod types;
pub mod vfs;

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
    check_files_with(files, effects::Profile::Application)
}

/// Loads and checks a program: syntax, then, when every module parses, the semantic checks,
/// the profile and the permissions of the manifest.
pub fn load_checked(files: &[std::path::PathBuf], profile: effects::Profile) -> program::Program {
    load_with(files, profile, program::TestScope::None)
}

/// Loads and checks a program for `unio test`: the tests of the given files whose description
/// contains the filter run, so their function values count in the effects analysis; the tests
/// of imported modules and of the standard library do not (RFC 0005, tests in the analysis).
pub fn load_for_test(files: &[std::path::PathBuf], filter: Option<&str>) -> program::Program {
    let scope = program::TestScope::Given {
        filter: filter.map(str::to_string),
    };
    load_with(files, effects::Profile::Application, scope)
}

/// Like check_files, with the analysis of the tests that `unio test` runs with this filter.
pub fn check_files_for_test(files: &[std::path::PathBuf], filter: Option<&str>) -> Vec<Analysis> {
    let files = files.to_vec();
    let filter = filter.map(str::to_string);
    on_front_end_stack(move || analyses(load_for_test(&files, filter.as_deref())))
}

fn load_with(
    files: &[std::path::PathBuf],
    profile: effects::Profile,
    tests: program::TestScope,
) -> program::Program {
    let mut program = program::Program::load(files);
    program.tests = tests;
    program.check_manifests();
    let syntax_ok = program.modules.iter().all(|m| !m.analysis.has_errors());
    if syntax_ok {
        check::check_program(&mut program);
        effects::report(&mut program, profile);
    } else {
        // Names and types are checked only when every module parses; otherwise the
        // syntax errors come first.
        program.diagnostics.clear();
    }
    program
}

/// `unio symbol`: the reports of the declarations with the full name, found in the module
/// that a prefix of the name designates, from the package around `start`.
pub fn symbol(query: &str, start: &std::path::Path) -> Vec<symbol::Report> {
    let files = program::symbol_module_files(query, start);
    let query = query.to_string();
    on_front_end_stack(move || {
        let program = load_checked(&files, effects::Profile::Application);
        symbol::lookup(&program, &query)
    })
}

/// Loads and checks a program once for `unio run` and `unio test`: the program when no given
/// file has an error, otherwise the analyses of the given files, as `check_files` reports them.
/// Call it on a thread with a large stack, as the front end needs.
pub fn load_for_run(
    files: &[std::path::PathBuf],
    profile: effects::Profile,
) -> Result<program::Program, Vec<Analysis>> {
    let program = load_checked(files, profile);
    let failed = !program.manifests.is_empty()
        || program.modules.iter().enumerate().any(|(i, m)| {
            m.report
                && (m.analysis.has_errors()
                    || program
                        .diagnostics
                        .iter()
                        .any(|(module, d)| *module == i && d.severity == Severity::Error))
        });
    if failed {
        Err(analyses(program))
    } else {
        Ok(program)
    }
}

/// Like check_files, with the build profile of `unio check --profile` (C-21).
pub fn check_files_with(files: &[std::path::PathBuf], profile: effects::Profile) -> Vec<Analysis> {
    let files = files.to_vec();
    on_front_end_stack(move || analyses(load_checked(&files, profile)))
}

/// The analyses of the given files, with the semantic diagnostics merged in.
pub fn analyses(mut program: program::Program) -> Vec<Analysis> {
    let semantic = std::mem::take(&mut program.diagnostics);
    let manifests = std::mem::take(&mut program.manifests);
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
        .chain(manifests)
        .collect()
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
