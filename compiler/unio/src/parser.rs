//! The parser: sections F, S, E, P and T of spec/grammar.ebnf.
//!
//! A recursive-descent parser that follows the ordered choices of the grammar. After a
//! syntax error it skips to the end of the logical line, and over the indented block that
//! follows it, so that one file reports several errors (C-2). Constraints the EBNF states
//! only in comments are checked here when they are purely syntactic: E0020, E0023, E0024,
//! E0030 and E0034.

use crate::ast::*;
use crate::catalog;
use crate::diag::{Diagnostic, Fix, Msg, Span};
use crate::lexer::{Kind, TextPiece, Token};

/// How deep expressions, patterns, types and blocks may nest (E0013).
const MAX_DEPTH: usize = 64;

pub struct Failed;

type P<T> = Result<T, Failed>;

pub struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    pub diagnostics: Vec<Diagnostic>,
    speculating: usize,
    depth: usize,
    /// Inline handlers seen in the current simple statement.
    inline_handlers: usize,
    /// The token position right after the last inline handler, and its span.
    handler_end: Option<(usize, Span)>,
}

const COMPARISONS: &[&str] = &["==", "!=", "<", "<=", ">", ">="];

impl<'a> Parser<'a> {
    pub fn new(tokens: &'a [Token]) -> Parser<'a> {
        Parser {
            tokens,
            pos: 0,
            diagnostics: Vec::new(),
            speculating: 0,
            depth: 0,
            inline_handlers: 0,
            handler_end: None,
        }
    }

    // ------------------------------------------------------------ token access

    fn token(&self, k: usize) -> &'a Token {
        let last = self.tokens.len() - 1;
        &self.tokens[(self.pos + k).min(last)]
    }

    fn peek(&self) -> &'a Token {
        self.token(0)
    }

    fn kind(&self) -> &'a Kind {
        &self.peek().kind
    }

    fn at_kw(&self, k: &str) -> bool {
        matches!(self.kind(), Kind::Keyword(w) if *w == k)
    }

    fn at_sym(&self, s: &str) -> bool {
        matches!(self.kind(), Kind::Symbol(w) if *w == s)
    }

    fn sym_at(&self, k: usize, s: &str) -> bool {
        matches!(&self.token(k).kind, Kind::Symbol(w) if *w == s)
    }

    fn kw_at(&self, k: usize, w: &str) -> bool {
        matches!(&self.token(k).kind, Kind::Keyword(x) if *x == w)
    }

    fn at_name(&self) -> bool {
        *self.kind() == Kind::Name
    }

    fn name_at(&self, k: usize) -> bool {
        self.token(k).kind == Kind::Name
    }

    fn at_word(&self, word: &str) -> bool {
        self.at_name() && self.peek().text == word
    }

    fn bump(&mut self) -> &'a Token {
        let token = self.peek();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        token
    }

    fn prev_span(&self) -> Span {
        if self.pos == 0 {
            return self.peek().span;
        }
        self.tokens[self.pos - 1].span
    }

    fn span_from(&self, start: Span) -> Span {
        let end = self.prev_span();
        if end.end.offset < start.start.offset {
            return start;
        }
        start.to(end)
    }

    // ------------------------------------------------------------ errors

    fn report(&mut self, diagnostic: Diagnostic) {
        if self.speculating == 0 {
            self.diagnostics.push(diagnostic);
        }
    }

    fn expected(&mut self, what: Msg) -> Failed {
        let token = self.peek();
        let message = catalog::expected(&what, &token.describe());
        let mut diagnostic = Diagnostic::error("E0012", token.span, message);
        if what.en == "':'" && token.kind == Kind::Newline {
            diagnostic = diagnostic.with_fix(Fix {
                span: token.span,
                new_text: ":".to_string(),
                description: catalog::add_colon(),
            });
        }
        self.report(diagnostic);
        Failed
    }

    fn expect_sym(&mut self, s: &str) -> P<Span> {
        if self.at_sym(s) {
            return Ok(self.bump().span);
        }
        Err(self.expected(catalog::quoted(s)))
    }

    fn expect_kw(&mut self, k: &str) -> P<Span> {
        if self.at_kw(k) {
            return Ok(self.bump().span);
        }
        Err(self.expected(catalog::quoted(k)))
    }

    fn expect_name(&mut self) -> P<Name> {
        if self.at_name() {
            let token = self.bump();
            return Ok(Name {
                text: token.text.clone(),
                span: token.span,
            });
        }
        Err(self.expected(catalog::a_name()))
    }

    fn expect_newline(&mut self) -> P<()> {
        if *self.kind() == Kind::Newline {
            self.bump();
            return Ok(());
        }
        Err(self.expected(catalog::end_of_line()))
    }

    fn nested<T>(&mut self, f: impl FnOnce(&mut Self) -> P<T>) -> P<T> {
        if self.depth >= MAX_DEPTH {
            let span = self.peek().span;
            self.report(Diagnostic::error(
                "E0013",
                span,
                catalog::nested_too_deeply(),
            ));
            return Err(Failed);
        }
        self.depth += 1;
        let result = f(self);
        self.depth -= 1;
        result
    }

    /// Tries a parse without reporting; on failure restores the position.
    fn speculate<T>(&mut self, f: impl FnOnce(&mut Self) -> P<T>) -> Option<T> {
        let saved = (self.pos, self.inline_handlers, self.handler_end);
        self.speculating += 1;
        let result = f(self);
        self.speculating -= 1;
        match result {
            Ok(value) => Some(value),
            Err(Failed) => {
                (self.pos, self.inline_handlers, self.handler_end) = saved;
                None
            }
        }
    }

    // ------------------------------------------------------------ recovery

    /// Skips to the end of the logical line and over an indented block after it.
    fn sync_line(&mut self) {
        loop {
            match self.kind() {
                Kind::Newline => {
                    self.bump();
                    break;
                }
                Kind::Eof | Kind::Dedent => break,
                Kind::Indent => {
                    self.skip_block();
                    break;
                }
                _ => {
                    self.bump();
                }
            }
        }
        if *self.kind() == Kind::Indent {
            self.skip_block();
        }
    }

    fn skip_block(&mut self) {
        let mut level = 0usize;
        loop {
            match self.kind() {
                Kind::Indent => level += 1,
                Kind::Dedent => {
                    level = level.saturating_sub(1);
                    if level == 0 {
                        self.bump();
                        return;
                    }
                }
                Kind::Eof => return,
                _ => {}
            }
            self.bump();
        }
    }

    // ------------------------------------------------------------ files

    pub fn file(&mut self) -> File {
        let mut unsafe_module = false;
        if self.at_word("unsafe") && self.name_at(1) && self.token(1).text == "module" {
            self.bump();
            self.bump();
            if self.expect_newline().is_err() {
                self.sync_line();
            }
            unsafe_module = true;
        }
        let mut items = Vec::new();
        while *self.kind() != Kind::Eof {
            let before = self.pos;
            match self.top_item() {
                Ok(item) => items.push(item),
                Err(Failed) => self.sync_line(),
            }
            if self.pos == before {
                self.bump();
            }
        }
        File {
            unsafe_module,
            items,
        }
    }

    fn top_item(&mut self) -> P<Item> {
        if self.at_kw("import") || self.at_kw("from") {
            return self.import().map(Item::Import);
        }
        if self.at_kw("extend") {
            return self.extend().map(Item::Extend);
        }
        if self.at_word("test") && self.token(1).kind == Kind::Text {
            return self.test_block().map(Item::Test);
        }
        self.declaration().map(Item::Declaration)
    }

    fn module_path(&mut self) -> P<Vec<Name>> {
        let mut path = vec![self.expect_name()?];
        while self.at_sym(".") {
            self.bump();
            path.push(self.expect_name()?);
        }
        Ok(path)
    }

    fn import(&mut self) -> P<Import> {
        let start = self.peek().span;
        if self.at_kw("import") {
            self.bump();
            let module = self.module_path()?;
            let alias = if self.at_kw("as") {
                self.bump();
                Some(self.expect_name()?)
            } else {
                None
            };
            self.expect_newline()?;
            return Ok(Import {
                module,
                alias,
                names: Vec::new(),
                span: self.span_from(start),
            });
        }
        self.expect_kw("from")?;
        let module = self.module_path()?;
        self.expect_kw("import")?;
        let mut names = vec![self.expect_name()?];
        while self.at_sym(",") {
            self.bump();
            names.push(self.expect_name()?);
        }
        self.expect_newline()?;
        Ok(Import {
            module,
            alias: None,
            names,
            span: self.span_from(start),
        })
    }

    fn declaration(&mut self) -> P<Declaration> {
        let start = self.peek().span;
        let public = self.at_kw("public");
        if public {
            self.bump();
        }
        let kind = if self.at_kw("function") {
            DeclarationKind::Function(self.function()?)
        } else if self.at_word("external") && self.kw_at(1, "function") {
            self.bump();
            let signature = self.signature()?;
            self.expect_newline()?;
            DeclarationKind::External(signature)
        } else if self.at_kw("struct") {
            DeclarationKind::Struct(self.struct_decl()?)
        } else if self.at_kw("enum") {
            DeclarationKind::Enum(self.enum_decl()?)
        } else if self.at_word("error") && self.name_at(1) {
            DeclarationKind::Error(self.error_decl()?)
        } else if self.at_kw("interface") {
            DeclarationKind::Interface(self.interface()?)
        } else if self.at_kw("let") {
            DeclarationKind::Constant(self.constant()?)
        } else {
            let what = if public {
                catalog::declaration_after_public()
            } else {
                catalog::declaration_or_import()
            };
            return Err(self.expected(what));
        };
        Ok(Declaration {
            public,
            kind,
            span: self.span_from(start),
        })
    }

    fn function(&mut self) -> P<Function> {
        let signature = self.signature()?;
        self.expect_sym(":")?;
        let body = self.block()?;
        Ok(Function { signature, body })
    }

    fn signature(&mut self) -> P<Signature> {
        let start = self.expect_kw("function")?;
        let name = self.expect_name()?;
        let generics = self.generic_params()?;
        self.expect_sym("(")?;
        let params = self.params()?;
        self.expect_sym(")")?;
        let returns = if self.at_kw("returns") {
            self.bump();
            Some(self.ty()?)
        } else {
            None
        };
        let fails = self.fails_with()?;
        Ok(Signature {
            name,
            generics,
            params,
            returns,
            fails,
            span: self.span_from(start),
        })
    }

    fn fails_with(&mut self) -> P<Vec<Type>> {
        if !self.at_kw("fails") {
            return Ok(Vec::new());
        }
        self.bump();
        self.expect_kw("with")?;
        self.type_list()
    }

    fn type_list(&mut self) -> P<Vec<Type>> {
        let mut types = vec![self.ty()?];
        while self.at_sym(",") {
            self.bump();
            types.push(self.ty()?);
        }
        Ok(types)
    }

    fn generic_params(&mut self) -> P<Vec<GenericParam>> {
        let mut generics = Vec::new();
        if !self.at_sym("[") {
            return Ok(generics);
        }
        self.bump();
        loop {
            let name = self.expect_name()?;
            let mut bounds = Vec::new();
            if self.at_kw("with") {
                self.bump();
                bounds.push(self.ty()?);
                while self.at_kw("and") {
                    self.bump();
                    bounds.push(self.ty()?);
                }
            }
            generics.push(GenericParam { name, bounds });
            if !self.at_sym(",") {
                break;
            }
            self.bump();
        }
        self.expect_sym("]")?;
        Ok(generics)
    }

    /// `[ param { "," param } [ "," ] ]` before ")".
    fn params(&mut self) -> P<Vec<Param>> {
        let mut params = Vec::new();
        while !self.at_sym(")") {
            params.push(self.param()?);
            if !self.at_sym(",") {
                break;
            }
            self.bump();
        }
        Ok(params)
    }

    fn param(&mut self) -> P<Param> {
        let start = self.peek().span;
        let var = self.at_kw("var");
        if var {
            self.bump();
        }
        if self.at_kw("self") {
            self.bump();
            return Ok(Param::SelfParam {
                var,
                span: self.span_from(start),
            });
        }
        if !var && self.at_word("named") && self.name_at(1) {
            self.bump();
            let name = self.expect_name()?;
            self.expect_kw("as")?;
            let ty = self.ty()?;
            let default = self.default_value()?;
            return Ok(Param::Named { name, ty, default });
        }
        let name = self.expect_name()?;
        self.expect_kw("as")?;
        let ty = self.ty()?;
        let default = self.default_value()?;
        Ok(Param::Plain {
            var,
            name,
            ty,
            default,
        })
    }

    fn default_value(&mut self) -> P<Option<Expr>> {
        if self.at_sym("=") {
            self.bump();
            return Ok(Some(self.expression()?));
        }
        Ok(None)
    }

    fn fields(&mut self) -> P<Vec<Field>> {
        self.expect_newline()?;
        if *self.kind() != Kind::Indent {
            return Err(self.expected(catalog::block_of_fields()));
        }
        self.bump();
        let mut fields = Vec::new();
        while !matches!(self.kind(), Kind::Dedent | Kind::Eof) {
            let before = self.pos;
            match self.field() {
                Ok(field) => fields.push(field),
                Err(Failed) => self.sync_line(),
            }
            if self.pos == before {
                self.bump();
            }
        }
        if *self.kind() == Kind::Dedent {
            self.bump();
        }
        Ok(fields)
    }

    fn field(&mut self) -> P<Field> {
        let public = self.at_kw("public");
        if public {
            self.bump();
        }
        let var = self.at_kw("var");
        if var {
            self.bump();
        }
        let name = self.expect_name()?;
        self.expect_kw("as")?;
        let ty = self.ty()?;
        let default = self.default_value()?;
        self.expect_newline()?;
        Ok(Field {
            public,
            var,
            name,
            ty,
            default,
        })
    }

    fn struct_decl(&mut self) -> P<Struct> {
        self.expect_kw("struct")?;
        let name = self.expect_name()?;
        let generics = self.generic_params()?;
        let fields = if self.at_sym(":") {
            self.bump();
            Some(self.fields()?)
        } else {
            self.expect_newline()?;
            None
        };
        Ok(Struct {
            name,
            generics,
            fields,
        })
    }

    fn error_decl(&mut self) -> P<ErrorDecl> {
        self.bump(); // the contextual keyword `error`
        let name = self.expect_name()?;
        let fields = if self.at_sym(":") {
            self.bump();
            Some(self.fields()?)
        } else {
            self.expect_newline()?;
            None
        };
        Ok(ErrorDecl { name, fields })
    }

    /// `":" NEWLINE INDENT` before the members of a declaration.
    fn open_body(&mut self) -> P<()> {
        self.expect_sym(":")?;
        self.expect_newline()?;
        if *self.kind() != Kind::Indent {
            return Err(self.expected(catalog::indented_block()));
        }
        self.bump();
        Ok(())
    }

    /// Parses members until the end of the block with recovery after each one.
    fn members<T>(&mut self, mut member: impl FnMut(&mut Self) -> P<T>) -> Vec<T> {
        let mut items = Vec::new();
        while !matches!(self.kind(), Kind::Dedent | Kind::Eof) {
            let before = self.pos;
            match member(self) {
                Ok(item) => items.push(item),
                Err(Failed) => self.sync_line(),
            }
            if self.pos == before {
                self.bump();
            }
        }
        if *self.kind() == Kind::Dedent {
            self.bump();
        }
        items
    }

    fn enum_decl(&mut self) -> P<Enum> {
        self.expect_kw("enum")?;
        let name = self.expect_name()?;
        let generics = self.generic_params()?;
        self.open_body()?;
        let variants = self.members(|p| {
            let name = p.expect_name()?;
            let mut fields = Vec::new();
            if p.at_sym("(") {
                p.bump();
                loop {
                    let field = p.expect_name()?;
                    p.expect_kw("as")?;
                    fields.push((field, p.ty()?));
                    if !p.at_sym(",") {
                        break;
                    }
                    p.bump();
                }
                p.expect_sym(")")?;
            }
            p.expect_newline()?;
            Ok(Variant { name, fields })
        });
        Ok(Enum {
            name,
            generics,
            variants,
        })
    }

    fn interface(&mut self) -> P<Interface> {
        self.expect_kw("interface")?;
        let name = self.expect_name()?;
        let generics = self.generic_params()?;
        self.open_body()?;
        let members = self.members(|p| {
            let signature = p.signature()?;
            if p.at_sym(":") {
                p.bump();
                let body = p.block()?;
                return Ok((signature, Some(body)));
            }
            p.expect_newline()?;
            Ok((signature, None))
        });
        Ok(Interface {
            name,
            generics,
            members,
        })
    }

    fn extend(&mut self) -> P<Extend> {
        let start = self.expect_kw("extend")?;
        let generics = self.generic_params()?;
        let target = self.ty()?;
        let interface = if self.at_kw("with") {
            self.bump();
            Some(self.ty()?)
        } else {
            None
        };
        self.open_body()?;
        let members = self.members(|p| {
            let public = p.at_kw("public");
            if public {
                p.bump();
            }
            if p.at_word("external") && p.kw_at(1, "function") {
                p.bump();
                let signature = p.signature()?;
                p.expect_newline()?;
                return Ok(Member {
                    public,
                    external: true,
                    signature,
                    body: None,
                });
            }
            if !p.at_kw("function") {
                return Err(p.expected(catalog::a_function()));
            }
            let function = p.function()?;
            Ok(Member {
                public,
                external: false,
                signature: function.signature,
                body: Some(function.body),
            })
        });
        Ok(Extend {
            generics,
            target,
            interface,
            members,
            span: self.span_from(start),
        })
    }

    fn constant(&mut self) -> P<Constant> {
        self.expect_kw("let")?;
        let name = self.expect_name()?;
        let ty = if self.at_kw("as") {
            self.bump();
            Some(self.ty()?)
        } else {
            None
        };
        self.expect_sym("=")?;
        let value = self.expression()?;
        self.expect_newline()?;
        Ok(Constant { name, ty, value })
    }

    fn test_block(&mut self) -> P<Test> {
        let start = self.bump().span; // the contextual keyword `test`
        let token = self.bump();
        let description = self.text_parts(token);
        self.expect_sym(":")?;
        let body = self.block()?;
        Ok(Test {
            description,
            body,
            span: self.span_from(start),
        })
    }

    // ------------------------------------------------------------ statements

    fn block(&mut self) -> P<Block> {
        self.expect_newline()?;
        if *self.kind() != Kind::Indent {
            return Err(self.expected(catalog::indented_block()));
        }
        self.bump();
        self.nested(|p| {
            let statements = p.members(|p| p.statement());
            if statements.is_empty() {
                return Err(p.expected(catalog::a_statement()));
            }
            Ok(statements)
        })
    }

    /// `simple_statement NEWLINE | block` after the ':' of an if, else, case or on branch
    /// (RFC 0002). A one-line branch cannot take an inline handler, which needs a block.
    fn branch_block(&mut self) -> P<Block> {
        if *self.kind() == Kind::Newline {
            return self.block();
        }
        let start = self.peek().span;
        self.inline_handlers = 0;
        self.handler_end = None;
        let statement = self.simple()?;
        if self.inline_handlers > 0 {
            let span = self.handler_end.map_or(start, |(_, span)| span);
            self.report(Diagnostic::error(
                "E0024",
                span,
                catalog::handler_in_branch(),
            ));
            return Err(Failed);
        }
        self.expect_newline()?;
        Ok(vec![Statement {
            kind: StatementKind::Simple {
                statement,
                handler: None,
            },
            span: self.span_from(start),
        }])
    }

    fn statement(&mut self) -> P<Statement> {
        let start = self.peek().span;
        let kind = if self.at_kw("if") {
            self.if_statement()?
        } else if self.at_kw("for") {
            self.bump();
            let mut names = vec![self.expect_name()?];
            while self.at_sym(",") {
                self.bump();
                names.push(self.expect_name()?);
            }
            self.expect_kw("in")?;
            let iterable = self.expression()?;
            self.expect_sym(":")?;
            let body = self.block()?;
            StatementKind::For {
                names,
                iterable,
                body,
            }
        } else if self.at_kw("while") {
            self.bump();
            let condition = self.expression()?;
            self.expect_sym(":")?;
            let body = self.block()?;
            StatementKind::While { condition, body }
        } else if self.at_kw("match") {
            self.match_statement()?
        } else if self.at_kw("try") && self.sym_at(1, ":") {
            self.try_statement()?
        } else if self.at_kw("using") {
            self.bump();
            let name = self.expect_name()?;
            self.expect_sym("=")?;
            let value = self.expression()?;
            self.expect_sym(":")?;
            let body = self.block()?;
            StatementKind::Using { name, value, body }
        } else {
            self.simple_statement()?
        };
        Ok(Statement {
            kind,
            span: self.span_from(start),
        })
    }

    fn if_statement(&mut self) -> P<StatementKind> {
        self.expect_kw("if")?;
        let condition = self.expression()?;
        self.expect_sym(":")?;
        let mut branches = vec![(condition, self.branch_block()?)];
        let mut otherwise = None;
        while self.at_kw("else") {
            if self.kw_at(1, "if") {
                self.bump();
                self.bump();
                let condition = self.expression()?;
                self.expect_sym(":")?;
                branches.push((condition, self.branch_block()?));
            } else {
                self.bump();
                self.expect_sym(":")?;
                otherwise = Some(self.branch_block()?);
                break;
            }
        }
        Ok(StatementKind::If {
            branches,
            otherwise,
        })
    }

    fn match_statement(&mut self) -> P<StatementKind> {
        self.expect_kw("match")?;
        let subject = self.expression()?;
        self.open_body()?;
        let cases = self.members(|p| {
            p.expect_kw("case")?;
            let pattern = p.pattern()?;
            let guard = if p.at_kw("if") {
                p.bump();
                Some(p.expression()?)
            } else {
                None
            };
            p.expect_sym(":")?;
            let body = p.branch_block()?;
            Ok(Case {
                pattern,
                guard,
                body,
            })
        });
        if cases.is_empty() {
            return Err(self.expected(catalog::quoted("case")));
        }
        Ok(StatementKind::Match { subject, cases })
    }

    fn try_statement(&mut self) -> P<StatementKind> {
        self.expect_kw("try")?;
        self.expect_sym(":")?;
        let body = self.block()?;
        let mut handlers = Vec::new();
        while self.at_kw("on") {
            self.bump();
            let ty = self.ty()?;
            let name = if self.at_kw("as") {
                self.bump();
                Some(self.expect_name()?)
            } else {
                None
            };
            self.expect_sym(":")?;
            let body = self.branch_block()?;
            handlers.push(Handler { ty, name, body });
        }
        if handlers.is_empty() {
            return Err(self.expected(catalog::on_after_try()));
        }
        Ok(StatementKind::Try { body, handlers })
    }

    fn simple_statement(&mut self) -> P<StatementKind> {
        self.inline_handlers = 0;
        self.handler_end = None;
        let statement = self.simple()?;
        let handlers = self.inline_handlers;
        let at_end = matches!(self.handler_end, Some((end, _)) if end == self.pos);
        let handler_span = self.handler_end.map(|(_, span)| span);
        if handlers > 1 {
            let span = handler_span.unwrap_or(self.peek().span);
            self.report(Diagnostic::error(
                "E0024",
                span,
                catalog::one_inline_handler(),
            ));
        }
        if self.at_sym(":") {
            if handlers == 0 || !at_end {
                if handlers > 0 {
                    let span = handler_span.unwrap_or(self.peek().span);
                    self.report(Diagnostic::error(
                        "E0024",
                        span,
                        catalog::handler_not_last(),
                    ));
                    return Err(Failed);
                }
                return Err(self.expected(catalog::end_of_line()));
            }
            self.bump();
            let handler = self.block()?;
            return Ok(StatementKind::Simple {
                statement,
                handler: Some(handler),
            });
        }
        self.expect_newline()?;
        if handlers > 0 {
            let span = handler_span.unwrap_or(self.prev_span());
            self.report(Diagnostic::error(
                "E0024",
                span,
                catalog::handler_needs_block(),
            ));
        }
        Ok(StatementKind::Simple {
            statement,
            handler: None,
        })
    }

    fn simple(&mut self) -> P<Simple> {
        if self.at_kw("let") || self.at_kw("var") {
            let var = self.at_kw("var");
            self.bump();
            let name = self.expect_name()?;
            let ty = if self.at_kw("as") {
                self.bump();
                Some(self.ty()?)
            } else {
                None
            };
            self.expect_sym("=")?;
            let value = self.expression()?;
            return Ok(Simple::Let {
                var,
                name,
                ty,
                value,
            });
        }
        if self.at_kw("return") {
            self.bump();
            if matches!(self.kind(), Kind::Newline | Kind::Eof | Kind::Dedent) {
                return Ok(Simple::Return(None));
            }
            return Ok(Simple::Return(Some(self.expression()?)));
        }
        if self.at_kw("fail") {
            self.bump();
            return Ok(Simple::Fail(self.expression()?));
        }
        if self.at_kw("assert") {
            self.bump();
            return Ok(Simple::Assert(self.expression()?));
        }
        for (word, statement) in [
            ("break", Simple::Break),
            ("continue", Simple::Continue),
            ("pass", Simple::Pass),
        ] {
            if self.at_kw(word) {
                self.bump();
                return Ok(statement);
            }
        }
        let target = self.expression()?;
        if self.at_sym("=") {
            if !matches!(
                target.kind,
                ExprKind::Name(_) | ExprKind::Field { .. } | ExprKind::Index { .. }
            ) {
                self.report(Diagnostic::error(
                    "E0030",
                    target.span,
                    catalog::invalid_assignment_target(),
                ));
                return Err(Failed);
            }
            self.bump();
            let value = self.expression()?;
            return Ok(Simple::Assign { target, value });
        }
        Ok(Simple::Expr(target))
    }

    // ------------------------------------------------------------ expressions

    pub fn expression(&mut self) -> P<Expr> {
        self.nested(|p| {
            if p.at_kw("function") {
                p.lambda()
            } else {
                p.conditional()
            }
        })
    }

    fn lambda(&mut self) -> P<Expr> {
        let start = self.expect_kw("function")?;
        self.expect_sym("(")?;
        let mut params = Vec::new();
        while !self.at_sym(")") {
            let named = self.at_word("named") && self.name_at(1);
            if named {
                self.bump();
            }
            let name = self.expect_name()?;
            let ty = if self.at_kw("as") {
                self.bump();
                Some(self.ty()?)
            } else {
                None
            };
            params.push(LambdaParam { named, name, ty });
            if !self.at_sym(",") {
                break;
            }
            self.bump();
        }
        self.expect_sym(")")?;
        let returns = if self.at_kw("returns") {
            self.bump();
            Some(self.ty()?)
        } else {
            None
        };
        let fails = self.fails_with()?;
        self.expect_sym(":")?;
        let body = Box::new(self.expression()?);
        Ok(Expr {
            kind: ExprKind::Lambda {
                params,
                returns,
                fails,
                body,
            },
            span: self.span_from(start),
        })
    }

    fn conditional(&mut self) -> P<Expr> {
        let start = self.peek().span;
        let then = self.or_expr()?;
        if !self.at_kw("if") {
            return Ok(then);
        }
        self.bump();
        let condition = self.or_expr()?;
        self.expect_kw("else")?;
        let otherwise = self.expression()?;
        Ok(Expr {
            kind: ExprKind::Conditional {
                then: Box::new(then),
                condition: Box::new(condition),
                otherwise: Box::new(otherwise),
            },
            span: self.span_from(start),
        })
    }

    fn binary(&self, op: &'static str, left: Expr, right: Expr) -> Expr {
        let span = left.span.to(right.span);
        Expr {
            kind: ExprKind::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            },
            span,
        }
    }

    fn or_expr(&mut self) -> P<Expr> {
        let mut left = self.and_expr()?;
        while self.at_kw("or") {
            self.bump();
            let right = self.and_expr()?;
            left = self.binary("or", left, right);
        }
        Ok(left)
    }

    fn and_expr(&mut self) -> P<Expr> {
        let mut left = self.not_expr()?;
        while self.at_kw("and") {
            self.bump();
            let right = self.not_expr()?;
            left = self.binary("and", left, right);
        }
        Ok(left)
    }

    fn not_expr(&mut self) -> P<Expr> {
        if self.at_kw("not") {
            let start = self.bump().span;
            let operand = self.nested(|p| p.not_expr())?;
            return Ok(Expr {
                kind: ExprKind::Unary {
                    op: "not",
                    operand: Box::new(operand),
                },
                span: self.span_from(start),
            });
        }
        self.comparison()
    }

    fn comparison_op(&self) -> Option<&'static str> {
        if let Kind::Symbol(s) = self.kind() {
            if let Some(op) = COMPARISONS.iter().find(|c| **c == *s) {
                return Some(op);
            }
        }
        if self.at_kw("in") {
            return Some("in");
        }
        if self.at_kw("not") && self.kw_at(1, "in") {
            return Some("not in");
        }
        None
    }

    fn comparison(&mut self) -> P<Expr> {
        let left = self.additive()?;
        let expr = if self.at_kw("is") {
            self.bump();
            let pattern = self.pattern()?;
            let span = left.span.to(pattern.span);
            Expr {
                kind: ExprKind::Is {
                    value: Box::new(left),
                    pattern: Box::new(pattern),
                },
                span,
            }
        } else {
            let Some(op) = self.comparison_op() else {
                return Ok(left);
            };
            self.bump();
            if op == "not in" {
                self.bump();
            }
            let right = self.additive()?;
            self.binary(op, left, right)
        };
        if self.comparison_op().is_some() || self.at_kw("is") {
            let span = self.peek().span;
            self.report(Diagnostic::error(
                "E0020",
                span,
                catalog::chained_comparison(),
            ));
            return Err(Failed);
        }
        Ok(expr)
    }

    fn additive(&mut self) -> P<Expr> {
        let mut left = self.multiplicative()?;
        loop {
            let op = if self.at_sym("+") {
                "+"
            } else if self.at_sym("-") {
                "-"
            } else {
                return Ok(left);
            };
            self.bump();
            let right = self.multiplicative()?;
            left = self.binary(op, left, right);
        }
    }

    fn multiplicative(&mut self) -> P<Expr> {
        let mut left = self.unary()?;
        loop {
            let op = if self.at_sym("*") {
                "*"
            } else if self.at_sym("/") {
                "/"
            } else if self.at_sym("%") {
                "%"
            } else {
                return Ok(left);
            };
            self.bump();
            let right = self.unary()?;
            left = self.binary(op, left, right);
        }
    }

    fn unary(&mut self) -> P<Expr> {
        let start = self.peek().span;
        if self.at_sym("-") {
            self.bump();
            let operand = self.nested(|p| p.unary())?;
            return Ok(Expr {
                kind: ExprKind::Unary {
                    op: "-",
                    operand: Box::new(operand),
                },
                span: self.span_from(start),
            });
        }
        if self.at_kw("try") {
            self.bump();
            let operand = Box::new(self.nested(|p| p.unary())?);
            let mut handler = None;
            if self.at_kw("on") {
                self.bump();
                let ty = self.ty()?;
                let name = if self.at_kw("as") {
                    self.bump();
                    Some(self.expect_name()?)
                } else {
                    None
                };
                self.inline_handlers += 1;
                self.handler_end = Some((self.pos, start.to(self.prev_span())));
                handler = Some((ty, name));
            }
            return Ok(Expr {
                kind: ExprKind::Try { operand, handler },
                span: self.span_from(start),
            });
        }
        self.power()
    }

    fn power(&mut self) -> P<Expr> {
        let base = self.postfix()?;
        if self.at_sym("**") {
            self.bump();
            let exponent = self.nested(|p| p.unary())?;
            return Ok(self.binary("**", base, exponent));
        }
        Ok(base)
    }

    fn arguments(&mut self) -> P<Vec<Argument>> {
        self.expect_sym("(")?;
        let mut arguments: Vec<Argument> = Vec::new();
        while !self.at_sym(")") {
            let name = if self.at_name() && self.sym_at(1, "=") {
                let name = self.expect_name()?;
                self.bump();
                Some(name)
            } else {
                None
            };
            let value = self.expression()?;
            if name.is_none() && arguments.iter().any(|a| a.name.is_some()) {
                self.report(Diagnostic::error(
                    "E0023",
                    value.span,
                    catalog::positional_after_named(),
                ));
            }
            arguments.push(Argument { name, value });
            if !self.at_sym(",") {
                break;
            }
            self.bump();
        }
        self.expect_sym(")")?;
        Ok(arguments)
    }

    /// `"[" type { "," type } "]"` directly followed by "(" (the generic_call rule).
    fn generic_arguments(&mut self) -> P<Vec<Type>> {
        self.expect_sym("[")?;
        let types = self.type_list()?;
        self.expect_sym("]")?;
        if !self.at_sym("(") {
            return Err(self.expected(catalog::quoted("(")));
        }
        Ok(types)
    }

    fn postfix(&mut self) -> P<Expr> {
        let mut expr = self.primary()?;
        loop {
            let start = expr.span;
            if self.at_sym("(") {
                let arguments = self.arguments()?;
                expr = Expr {
                    kind: ExprKind::Call {
                        callee: Box::new(expr),
                        arguments,
                    },
                    span: self.span_from(start),
                };
            } else if self.at_sym("[") {
                let generic = self.name_at(1)
                    && self
                        .token(1)
                        .text
                        .starts_with(|c: char| c.is_ascii_uppercase());
                if let Some(types) = if generic {
                    self.speculate(|p| p.generic_arguments())
                } else {
                    None
                } {
                    let arguments = self.arguments()?;
                    expr = Expr {
                        kind: ExprKind::GenericCall {
                            callee: Box::new(expr),
                            types,
                            arguments,
                        },
                        span: self.span_from(start),
                    };
                } else {
                    self.bump();
                    let index = Box::new(self.expression()?);
                    self.expect_sym("]")?;
                    expr = Expr {
                        kind: ExprKind::Index {
                            target: Box::new(expr),
                            index,
                        },
                        span: self.span_from(start),
                    };
                }
            } else if self.at_sym(".") {
                self.bump();
                let name = self.expect_name()?;
                expr = Expr {
                    kind: ExprKind::Field {
                        target: Box::new(expr),
                        name,
                    },
                    span: self.span_from(start),
                };
            } else if self.at_sym("?.") {
                self.bump();
                let name = self.expect_name()?;
                expr = Expr {
                    kind: ExprKind::SafeField {
                        target: Box::new(expr),
                        name,
                    },
                    span: self.span_from(start),
                };
            } else {
                return Ok(expr);
            }
        }
    }

    fn primary(&mut self) -> P<Expr> {
        let token = self.peek();
        let start = token.span;
        let kind = match &token.kind {
            Kind::Int => {
                self.bump();
                ExprKind::Int(token.text.clone())
            }
            Kind::Float => {
                self.bump();
                ExprKind::Float(token.text.clone())
            }
            Kind::Text => {
                self.bump();
                ExprKind::Text(self.text_parts(token))
            }
            Kind::Name => {
                self.bump();
                ExprKind::Name(token.text.clone())
            }
            Kind::Keyword("true") => {
                self.bump();
                ExprKind::Bool(true)
            }
            Kind::Keyword("false") => {
                self.bump();
                ExprKind::Bool(false)
            }
            Kind::Keyword("self") => {
                self.bump();
                ExprKind::SelfValue
            }
            Kind::Symbol("(") => return self.paren(),
            Kind::Symbol("[") => return self.list(),
            Kind::Symbol("{") => return self.map(),
            _ => return Err(self.expected(catalog::an_expression())),
        };
        Ok(Expr { kind, span: start })
    }

    fn paren(&mut self) -> P<Expr> {
        let start = self.expect_sym("(")?;
        let first = self.expression()?;
        if self.at_sym(")") {
            self.bump();
            return Ok(Expr {
                kind: ExprKind::Paren(Box::new(first)),
                span: self.span_from(start),
            });
        }
        self.expect_sym(",")?;
        let mut items = vec![first];
        while !self.at_sym(")") {
            items.push(self.expression()?);
            if !self.at_sym(",") {
                break;
            }
            self.bump();
        }
        self.expect_sym(")")?;
        Ok(Expr {
            kind: ExprKind::Tuple(items),
            span: self.span_from(start),
        })
    }

    fn list(&mut self) -> P<Expr> {
        let start = self.expect_sym("[")?;
        if self.at_sym("]") {
            self.bump();
            return Ok(Expr {
                kind: ExprKind::List(Vec::new()),
                span: self.span_from(start),
            });
        }
        let first = self.expression()?;
        if self.at_kw("for") {
            self.bump();
            let mut names = vec![self.expect_name()?];
            while self.at_sym(",") {
                self.bump();
                names.push(self.expect_name()?);
            }
            self.expect_kw("in")?;
            let iterable = Box::new(self.or_expr()?);
            let condition = if self.at_kw("if") {
                self.bump();
                Some(Box::new(self.or_expr()?))
            } else {
                None
            };
            self.expect_sym("]")?;
            return Ok(Expr {
                kind: ExprKind::Comprehension {
                    element: Box::new(first),
                    names,
                    iterable,
                    condition,
                },
                span: self.span_from(start),
            });
        }
        let mut items = vec![first];
        while self.at_sym(",") {
            self.bump();
            if self.at_sym("]") {
                break;
            }
            items.push(self.expression()?);
        }
        self.expect_sym("]")?;
        Ok(Expr {
            kind: ExprKind::List(items),
            span: self.span_from(start),
        })
    }

    fn map(&mut self) -> P<Expr> {
        let start = self.expect_sym("{")?;
        let mut entries = Vec::new();
        while !self.at_sym("}") {
            let key = self.expression()?;
            self.expect_sym(":")?;
            let value = self.expression()?;
            entries.push((key, value));
            if !self.at_sym(",") {
                break;
            }
            self.bump();
        }
        self.expect_sym("}")?;
        Ok(Expr {
            kind: ExprKind::Map(entries),
            span: self.span_from(start),
        })
    }

    /// The pieces of a TEXT token; interpolations are parsed as expressions.
    fn text_parts(&mut self, token: &'a Token) -> Vec<TextPart> {
        let mut parts = Vec::new();
        for piece in &token.pieces {
            match piece {
                TextPiece::Literal(text) => parts.push(TextPart::Literal(text.clone())),
                TextPiece::Interpolation(tokens) => {
                    let mut inner = Parser::new(tokens);
                    inner.depth = self.depth;
                    inner.speculating = self.speculating;
                    let expr = inner.expression().ok().and_then(|expr| {
                        if *inner.kind() == Kind::Eof {
                            Some(expr)
                        } else {
                            let _ = inner.expected(catalog::brace_after_interpolation());
                            None
                        }
                    });
                    if inner.inline_handlers > 0 {
                        let span = inner.handler_end.map_or(token.span, |(_, span)| span);
                        inner.report(Diagnostic::error(
                            "E0024",
                            span,
                            catalog::handler_in_interpolation(),
                        ));
                    }
                    self.diagnostics.append(&mut inner.diagnostics);
                    let span = token.span;
                    parts.push(TextPart::Interpolation(expr.unwrap_or(Expr {
                        kind: ExprKind::Error,
                        span,
                    })));
                }
            }
        }
        parts
    }

    // ------------------------------------------------------------ patterns

    fn pattern(&mut self) -> P<Pattern> {
        self.nested(|p| {
            let first = p.bind_pattern()?;
            if !p.at_sym("|") {
                return Ok(first);
            }
            let start = first.span;
            let mut options = vec![first];
            while p.at_sym("|") {
                p.bump();
                options.push(p.bind_pattern()?);
            }
            Ok(Pattern {
                kind: PatternKind::Or(options),
                span: p.span_from(start),
            })
        })
    }

    fn bind_pattern(&mut self) -> P<Pattern> {
        let pattern = self.primary_pattern()?;
        if self.at_kw("as") {
            self.bump();
            let name = self.expect_name()?;
            let span = pattern.span.to(name.span);
            return Ok(Pattern {
                kind: PatternKind::Bind {
                    pattern: Box::new(pattern),
                    name,
                },
                span,
            });
        }
        Ok(pattern)
    }

    fn primary_pattern(&mut self) -> P<Pattern> {
        let start = self.peek().span;
        let token = self.peek();
        let kind = match &token.kind {
            Kind::Name if token.text == "_" => {
                self.bump();
                PatternKind::Wildcard
            }
            Kind::Symbol("-") if self.token(1).kind == Kind::Int => {
                self.bump();
                let number = self.bump();
                PatternKind::Literal(Literal::Int(format!("-{}", number.text)))
            }
            Kind::Int => {
                self.bump();
                PatternKind::Literal(Literal::Int(token.text.clone()))
            }
            Kind::Text => {
                self.bump();
                PatternKind::Literal(Literal::Text(self.text_parts(token)))
            }
            Kind::Keyword("true") => {
                self.bump();
                PatternKind::Literal(Literal::Bool(true))
            }
            Kind::Keyword("false") => {
                self.bump();
                PatternKind::Literal(Literal::Bool(false))
            }
            Kind::Name => {
                let name = self.module_path()?;
                if self.at_sym("(") {
                    self.bump();
                    let mut arguments = Vec::new();
                    while !self.at_sym(")") {
                        let label = if self.at_name() && self.sym_at(1, "=") {
                            let label = self.expect_name()?;
                            self.bump();
                            Some(label)
                        } else {
                            None
                        };
                        arguments.push((label, self.pattern()?));
                        if !self.at_sym(",") {
                            break;
                        }
                        self.bump();
                    }
                    self.expect_sym(")")?;
                    PatternKind::Constructor { name, arguments }
                } else {
                    PatternKind::Name(name)
                }
            }
            Kind::Symbol("[") => {
                self.bump();
                let mut elements = Vec::new();
                let mut rest_seen: Option<Span> = None;
                while !self.at_sym("]") {
                    if let Some(span) = rest_seen {
                        self.report(Diagnostic::error("E0034", span, catalog::rest_not_last()));
                        rest_seen = None;
                    }
                    if self.at_name() && self.sym_at(1, "...") {
                        let name = self.expect_name()?;
                        self.bump();
                        rest_seen = Some(name.span);
                        elements.push(ListElement::Rest(name));
                    } else {
                        elements.push(ListElement::Pattern(self.pattern()?));
                    }
                    if !self.at_sym(",") {
                        break;
                    }
                    self.bump();
                }
                self.expect_sym("]")?;
                PatternKind::List(elements)
            }
            Kind::Symbol("(") => {
                self.bump();
                let mut items = vec![self.pattern()?];
                self.expect_sym(",")?;
                while !self.at_sym(")") {
                    items.push(self.pattern()?);
                    if !self.at_sym(",") {
                        break;
                    }
                    self.bump();
                }
                self.expect_sym(")")?;
                PatternKind::Tuple(items)
            }
            _ => return Err(self.expected(catalog::a_pattern())),
        };
        Ok(Pattern {
            kind,
            span: self.span_from(start),
        })
    }

    // ------------------------------------------------------------ types

    fn ty(&mut self) -> P<Type> {
        self.nested(|p| {
            let start = p.peek().span;
            let path = p.module_path()?;
            let mut arguments = Vec::new();
            if p.at_sym("[") {
                p.bump();
                loop {
                    arguments.push(if p.at_sym("(") {
                        p.shape()?
                    } else {
                        TypeArgument::Type(p.ty()?)
                    });
                    if !p.at_sym(",") {
                        break;
                    }
                    p.bump();
                }
                p.expect_sym("]")?;
            }
            Ok(Type {
                path,
                arguments,
                span: p.span_from(start),
            })
        })
    }

    fn shape(&mut self) -> P<TypeArgument> {
        self.expect_sym("(")?;
        let mut params = Vec::new();
        while !self.at_sym(")") {
            if self.at_word("named") && self.name_at(1) && self.kw_at(2, "as") {
                self.bump();
                let name = self.expect_name()?;
                self.bump();
                params.push((Some(name), self.ty()?));
            } else {
                params.push((None, self.ty()?));
            }
            if !self.at_sym(",") {
                break;
            }
            self.bump();
        }
        self.expect_sym(")")?;
        let returns = if self.at_sym("->") {
            self.bump();
            Some(Box::new(self.ty()?))
        } else {
            None
        };
        let fails = self.fails_with()?;
        Ok(TypeArgument::Shape {
            params,
            returns,
            fails,
        })
    }
}
