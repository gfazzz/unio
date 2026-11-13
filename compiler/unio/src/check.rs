//! Checks of function bodies: names, types, calls, generics, methods, operators and patterns
//! (docs/semantics.md, sections 1–4, sprint S3), and errors, values, control flow, `match`,
//! resources and layers (sections 5–8, sprint S4).

use std::collections::{HashMap, HashSet};

use crate::ast::{self, ExprKind, PatternKind, StatementKind};
use crate::catalog;
use crate::diag::{Diagnostic, Fix, Related, Span};
use crate::effects::{self, Allocation, Atom, Callee, Flow, Holder, LambdaFact, Site, Use};
use crate::program::{
    closest, CallEdge, ConstId, DefKind, FnId, ModId, Owner, Program, Symbol, LAYER_NAMES,
};
use crate::symbol::Node;
use crate::types::{DefId, FnTy, ParamId, Prim, Ty};

/// Prelude declarations the rules refer to by name.
struct Known {
    option: Option<DefId>,
    result: Option<DefId>,
    range: Option<DefId>,
    iterator: Option<DefId>,
    resource: Option<DefId>,
    display: Option<DefId>,
}

/// A parameter as a call sees it.
#[derive(Clone)]
struct ParamSpec {
    name: Option<String>,
    ty: Ty,
    named_only: bool,
    has_default: bool,
    var: bool,
    /// Declared with a generic type bounded by `Resource`, such as `is_same(a as T, b as T)`:
    /// a resource may be passed to it (semantics 7.2).
    resource_bound: bool,
}

/// How a local name was introduced: it decides whether the name can change (L-10, L-11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LocalKind {
    Let,
    Var,
    Param,
    VarParam,
    Using,
}

struct Local {
    name: String,
    ty: Ty,
    kind: LocalKind,
    span: Span,
    /// Set when an expression names the local; a `let` of a required result that is never
    /// read is E0043 (L-30).
    read: std::cell::Cell<bool>,
    /// The values the local may hold, an index into `Facts::locals` (RFC 0005); None for a
    /// name bound by a pattern or a loop, whose value is not followed.
    flow: Option<usize>,
}

/// Where a place that is changed starts (L-11).
enum Root {
    Local(String, LocalKind),
    SelfValue(bool),
    Other,
}

/// A pattern as the exhaustiveness check sees it (docs/semantics.md 6.5).
#[derive(Clone, Debug)]
enum Pat {
    Wild,
    Variant(DefId, usize, Vec<Pat>),
    Bool(bool),
    Tuple(Vec<Pat>),
    List(Vec<Pat>, bool),
    Literal,
}

/// Errors collected while checking the operand of one `try`.
#[derive(Default)]
struct Frame {
    errors: Vec<Ty>,
    uncertain: bool,
}

fn span_key(s: Span) -> (usize, usize) {
    (s.start.offset, s.end.offset)
}

fn strip(e: &ast::Expr) -> &ast::Expr {
    match &e.kind {
        ExprKind::Paren(inner) => strip(inner),
        _ => e,
    }
}

/// Whether a loop body has a `break` of its own loop (not of a nested one).
fn has_break(block: &ast::Block) -> bool {
    block.iter().any(|s| match &s.kind {
        StatementKind::Simple { statement, handler } => {
            matches!(statement, ast::Simple::Break) || handler.as_ref().is_some_and(has_break)
        }
        StatementKind::If {
            branches,
            otherwise,
        } => {
            branches.iter().any(|(_, b)| has_break(b)) || otherwise.as_ref().is_some_and(has_break)
        }
        StatementKind::Match { cases, .. } => cases.iter().any(|c| has_break(&c.body)),
        StatementKind::Try { body, handlers } => {
            has_break(body) || handlers.iter().any(|h| has_break(&h.body))
        }
        StatementKind::Using { body, .. } => has_break(body),
        StatementKind::For { .. } | StatementKind::While { .. } => false,
    })
}

/// Whether a condition binds names with `is`, which its branches see and an expression read
/// after it does not (RFC 0005).
fn mentions_is(e: &ast::Expr) -> bool {
    match &e.kind {
        ExprKind::Is { .. } => true,
        ExprKind::Paren(inner) => mentions_is(inner),
        ExprKind::Binary { left, right, .. } => mentions_is(left) || mentions_is(right),
        ExprKind::Unary { operand, .. } => mentions_is(operand),
        _ => false,
    }
}

/// Whether an expression statement is a call, whose result must not be dropped (E0043).
fn is_call(e: &ast::Expr) -> bool {
    match &strip(e).kind {
        ExprKind::Call { .. } | ExprKind::GenericCall { .. } => true,
        ExprKind::Try { operand, .. } => is_call(operand),
        _ => false,
    }
}

pub fn check_program(program: &mut Program) {
    let known = Known {
        option: prelude_def(program, "Option"),
        result: prelude_def(program, "Result"),
        range: prelude_def(program, "Range"),
        iterator: prelude_def(program, "Iterator"),
        resource: prelude_def(program, "Resource"),
        display: prelude_def(program, "Display"),
    };
    let mut const_types: HashMap<ConstId, Ty> = HashMap::new();
    // Constants first, in order, so that functions see their types.
    for id in 0..program.consts.len() {
        let module = program.consts[id].module;
        let item = program.consts[id].item;
        let ast::Item::Declaration(d) = &program.modules[module].analysis.file.items[item] else {
            continue;
        };
        let ast::DeclarationKind::Constant(c) = &d.kind else {
            continue;
        };
        let c = c.clone();
        let value = c.value.clone();
        let annotated = program.consts[id].annotated.clone();
        let mut checker = Checker::new(program, &known, &const_types, module);
        checker.holder = Some(Holder::Const(id));
        let ty = checker.expr(&value, annotated.as_ref().unwrap_or(&Ty::Unknown));
        let flow = checker.value_of(&value, annotated.as_ref().unwrap_or(&ty));
        if !flow.is_empty() {
            checker.p.effect_facts.consts.insert(id, flow);
        }
        if let Some(expected) = &annotated {
            checker.expect(&ty, expected, value.span);
        } else if is_undetermined(&value) {
            checker.err("E0060", c.name.span, catalog::cannot_infer());
        }
        drop(checker);
        let ty = annotated.unwrap_or(ty);
        const_types.insert(id, ty);
    }
    for f in 0..program.funcs.len() {
        let Some(body) = program.funcs[f].body else {
            continue;
        };
        let block = match &program.modules[body.module].analysis.file.items[body.item] {
            ast::Item::Declaration(d) => match (&d.kind, body.member) {
                (ast::DeclarationKind::Function(func), None) => func.body.clone(),
                (ast::DeclarationKind::Interface(i), Some(m)) => {
                    i.members[m].1.clone().unwrap_or_default()
                }
                _ => continue,
            },
            ast::Item::Extend(e) => match body.member {
                Some(m) => e.members[m].body.clone().unwrap_or_default(),
                None => continue,
            },
            _ => continue,
        };
        let mut checker = Checker::new(program, &known, &const_types, body.module);
        checker.function(f, &block);
    }
    defaults(program, &known, &const_types);
    tests(program, &known, &const_types);
    conversions(program);
    externals(program);
}

/// The test blocks of every module. Each is checked; only the tests that run are part of the
/// effects analysis (RFC 0005, tests in the analysis): those of the modules `unio test` was
/// given whose description the filter selects. What any other test records is forgotten, so
/// its function values are not possible values of the program.
fn tests(program: &mut Program, known: &Known, const_types: &HashMap<ConstId, Ty>) {
    for module in 0..program.modules.len() {
        let tests: Vec<(ast::Block, Span, String)> = program.modules[module]
            .analysis
            .file
            .items
            .iter()
            .filter_map(|i| match i {
                ast::Item::Test(t) => Some((
                    t.body.clone(),
                    t.span,
                    crate::interp::description(&t.description),
                )),
                _ => None,
            })
            .collect();
        for (body, span, name) in tests {
            let runs = program.modules[module].given && program.tests.selects(&name);
            let mark = program.effect_facts.mark();
            let mut checker = Checker::new(program, known, const_types, module);
            checker.ret = Ty::Unit;
            if runs {
                let facts = &mut checker.p.effect_facts;
                facts.tests.push(effects::TestFact { module, span, name });
                checker.holder = Some(Holder::Test(facts.tests.len() - 1));
            }
            checker.scopes.push(Vec::new());
            checker.block(&body);
            drop(checker);
            if !runs {
                program.effect_facts.rewind(mark);
            }
        }
    }
}

/// S-5, S-17: the conversions of `std.json` never read or set a private field (E0046). The
/// type parameters of `to_value` and `from_value` need a convertible type; so does every type
/// parameter of a function that passes its own parameter on to one that needs it, such as
/// `encode` and `decode`, or a generic function of the program around them. At each call
/// where such a parameter gets a type that reaches a struct with a private field, through
/// containers, `Option`, tuples, fields and variants, the call is E0046.
fn conversions(program: &mut Program) {
    let json = program
        .modules
        .iter()
        .position(|m| m.stdlib && m.name == "std.json");
    let Some(json) = json else {
        return;
    };
    let mut needed: HashSet<ParamId> = program
        .funcs
        .iter()
        .filter(|f| f.module == json && matches!(f.name.as_str(), "to_value" | "from_value"))
        .flat_map(|f| f.generics.iter().copied())
        .collect();
    let mut found: Vec<(ModId, Span, Ty, (String, String))> = Vec::new();
    let mut done: HashSet<(usize, ParamId)> = HashSet::new();
    loop {
        let mut grew = false;
        for (i, call) in program.generic_calls.iter().enumerate() {
            for (param, ty) in &call.types {
                if !needed.contains(param) || !done.insert((i, *param)) {
                    continue;
                }
                let mut params = Vec::new();
                let mut seen = HashSet::new();
                if let Some(private) = private_field(program, ty, &mut params, &mut seen) {
                    found.push((call.module, call.span, ty.clone(), private));
                }
                for q in params {
                    grew |= needed.insert(q);
                }
            }
        }
        if !grew {
            break;
        }
    }
    program.conversion_params = needed;
    let mut reported = HashSet::new();
    for (module, span, ty, (owner, field)) in found {
        if reported.insert((module, span.start.offset)) {
            let shown = program.show(&ty);
            let message = catalog::private_conversion(&shown, &owner, &field);
            program.error(module, Diagnostic::error("E0046", span, message));
        }
    }
}

/// The first private field a JSON conversion of `ty` would read or set, as the name of its
/// type and the field; generic parameters met on the way are collected in `params`.
fn private_field(
    p: &Program,
    ty: &Ty,
    params: &mut Vec<ParamId>,
    seen: &mut HashSet<DefId>,
) -> Option<(String, String)> {
    match ty {
        Ty::Param(q) => {
            params.push(*q);
            None
        }
        Ty::List(t) | Ty::Set(t) => private_field(p, t, params, seen),
        Ty::Map(k, v) => {
            private_field(p, k, params, seen).or_else(|| private_field(p, v, params, seen))
        }
        Ty::Tuple(ts) => ts.iter().find_map(|t| private_field(p, t, params, seen)),
        Ty::Named(d, args) => {
            if !seen.insert(*d) {
                return None;
            }
            let def = &p.defs[*d];
            let map: HashMap<ParamId, Ty> =
                def.generics.iter().copied().zip(args.clone()).collect();
            match &def.kind {
                DefKind::Struct(fields) | DefKind::Error(fields) => {
                    if let Some(f) = fields.iter().find(|f| !f.public) {
                        return Some((def.name.clone(), f.name.clone()));
                    }
                    fields
                        .iter()
                        .find_map(|f| private_field(p, &f.ty.subst(&map, None), params, seen))
                }
                DefKind::Enum(variants) => variants.iter().find_map(|v| {
                    v.fields
                        .iter()
                        .find_map(|(_, t)| private_field(p, &t.subst(&map, None), params, seen))
                }),
                DefKind::Interface(_) => None,
            }
        }
        _ => None,
    }
}

/// S-7, S-10: an `external function` is a primitive of the runtime, declared only in an
/// `unsafe module` of the standard library; anywhere else it is E0086, so that a program
/// never reaches a call that the runtime cannot make.
fn externals(program: &mut Program) {
    let mut found = Vec::new();
    for (m, module) in program.modules.iter().enumerate() {
        let file = &module.analysis.file;
        let unsafe_module = file.unsafe_module;
        if module.stdlib && unsafe_module {
            continue;
        }
        for item in &file.items {
            let names: Vec<&ast::Name> = match item {
                ast::Item::Declaration(d) => match &d.kind {
                    ast::DeclarationKind::External(signature) => vec![&signature.name],
                    _ => Vec::new(),
                },
                ast::Item::Extend(e) => e
                    .members
                    .iter()
                    .filter(|member| member.external)
                    .map(|member| &member.signature.name)
                    .collect(),
                _ => Vec::new(),
            };
            for name in names {
                let message = catalog::external_outside_stdlib(&name.text, module.stdlib);
                found.push((m, Diagnostic::error("E0086", name.span, message)));
            }
        }
    }
    for (m, diagnostic) in found {
        program.error(m, diagnostic);
    }
}

/// The defaults of parameters and fields. Each is checked for names and types against the
/// type of its parameter or field (semantics 3.2); a default of a parameter sees the
/// generics of its function and the parameters before it. What a default calls, allocates
/// or uses counts for its function, or for every function that builds its type (semantics
/// 8.4).
fn defaults(program: &mut Program, known: &Known, const_types: &HashMap<ConstId, Ty>) {
    // Each default with the parameter or the field it is for.
    type Defaults = Vec<(Slot, ast::Expr)>;
    let mut work: Vec<(ModId, Node, Defaults)> = Vec::new();
    // The signatures of each module by the place of their names, as functions record them.
    let mut signatures: HashMap<(ModId, usize), &ast::Signature> = HashMap::new();
    for (m, module) in program.modules.iter().enumerate() {
        for item in &module.analysis.file.items {
            match item {
                ast::Item::Declaration(d) => match &d.kind {
                    ast::DeclarationKind::Function(func) => {
                        let sig = &func.signature;
                        signatures.insert((m, sig.name.span.start.offset), sig);
                    }
                    ast::DeclarationKind::External(sig) => {
                        signatures.insert((m, sig.name.span.start.offset), sig);
                    }
                    ast::DeclarationKind::Interface(i) => {
                        for (sig, _) in &i.members {
                            signatures.insert((m, sig.name.span.start.offset), sig);
                        }
                    }
                    _ => {}
                },
                ast::Item::Extend(e) => {
                    for member in &e.members {
                        let sig = &member.signature;
                        signatures.insert((m, sig.name.span.start.offset), sig);
                    }
                }
                _ => {}
            }
        }
    }
    for (f, def) in program.funcs.iter().enumerate() {
        let Some(sig) = signatures.get(&(def.module, def.span.start.offset)) else {
            continue;
        };
        let exprs: Vec<(Slot, ast::Expr)> = sig
            .params
            .iter()
            .filter_map(|p| match p {
                ast::Param::Named { default, .. } | ast::Param::Plain { default, .. } => {
                    Some(default.clone())
                }
                ast::Param::SelfParam { .. } => None,
            })
            .enumerate()
            .filter_map(|(i, default)| Some((Slot::Param(f, i), default?)))
            .collect();
        if !exprs.is_empty() {
            work.push((def.module, Node::Func(f), exprs));
        }
    }
    for (d, def) in program.defs.iter().enumerate() {
        let module = def.module;
        let exprs: Vec<(Slot, ast::Expr)> = program.modules[module]
            .analysis
            .file
            .items
            .iter()
            .filter_map(|item| match item {
                ast::Item::Declaration(decl) => match &decl.kind {
                    ast::DeclarationKind::Struct(x) if x.name.text == def.name => x.fields.as_ref(),
                    ast::DeclarationKind::Error(x) if x.name.text == def.name => x.fields.as_ref(),
                    _ => None,
                },
                _ => None,
            })
            .flatten()
            .filter_map(|field| {
                let slot = Slot::Field(d, field.name.text.clone());
                Some((slot, field.default.clone()?))
            })
            .collect();
        if !exprs.is_empty() {
            work.push((module, Node::Def(d), exprs));
        }
    }
    for (module, holder, exprs) in work {
        let mut checker = Checker::new(program, known, const_types, module);
        checker.holder = Some(Holder::from(holder));
        checker.scopes.push(Vec::new());
        // The parameters of a function with a body, each visible to the defaults after it
        // (runtime 1.3); the default of an external is evaluated alone and sees none.
        let mut params: Vec<(String, Ty, bool)> = Vec::new();
        let mut sees_params = false;
        match holder {
            Node::Func(f) => {
                params = checker.enter_function(f);
                let def = &checker.p.funcs[f];
                sees_params = def.body.is_some() || matches!(def.owner, Owner::Interface(_));
            }
            Node::Def(d) => {
                for g in checker.p.defs[d].generics.clone() {
                    checker.generics.insert(checker.p.params[g].name.clone(), g);
                }
            }
            _ => {}
        }
        let mut bound = 0;
        for (slot, e) in &exprs {
            let expected = match slot {
                Slot::Param(f, i) => {
                    if sees_params {
                        while bound < *i {
                            let (name, ty, var) = params[bound].clone();
                            checker.bind_param(*f, bound, name, ty, var);
                            bound += 1;
                        }
                    }
                    params.get(*i).map(|p| p.1.clone())
                }
                Slot::Field(d, name) => match &checker.p.defs[*d].kind {
                    DefKind::Struct(fields) | DefKind::Error(fields) => fields
                        .iter()
                        .find(|f| f.name == *name)
                        .map(|f| f.ty.clone()),
                    _ => None,
                },
            }
            .unwrap_or(Ty::Unknown);
            let ty = checker.expr(e, &expected);
            let flow = checker.value_of(
                e,
                if matches!(expected, Ty::Unknown) {
                    &ty
                } else {
                    &expected
                },
            );
            if !matches!(expected, Ty::Unknown) {
                checker.expect(&ty, &expected, e.span);
            }
            if flow.is_empty() {
                continue;
            }
            match slot {
                Slot::Param(f, i) => {
                    checker.p.effect_facts.defaults.insert((*f, *i), flow);
                }
                Slot::Field(d, name) => checker
                    .p
                    .effect_facts
                    .stores
                    .entry((*d, name.clone()))
                    .or_default()
                    .extend(flow),
            }
        }
    }
}

/// What a default is for: a parameter of a function, or a field of a type.
enum Slot {
    Param(FnId, usize),
    Field(DefId, String),
}

fn prelude_def(program: &Program, name: &str) -> Option<DefId> {
    match program.prelude.get(name) {
        Some((_, Symbol::Def(d))) => Some(*d),
        _ => None,
    }
}

/// `[]`, `{}` and `none` alone do not determine a type (E0060).
fn is_undetermined(e: &ast::Expr) -> bool {
    match &e.kind {
        ExprKind::List(items) => items.is_empty(),
        ExprKind::Map(items) => items.is_empty(),
        ExprKind::Name(n) => n == "none",
        ExprKind::Paren(inner) => is_undetermined(inner),
        _ => false,
    }
}

const BUILTIN_TYPE_NAMES: &[&str] = &[
    "Int", "Float", "Bool", "Text", "Bytes", "Path", "List", "Map", "Set", "Tuple", "Function",
    "Never",
];

const PRELUDE_VARIANTS: &[&str] = &["some", "none", "ok", "error"];

struct Checker<'a> {
    p: &'a mut Program,
    known: &'a Known,
    const_types: &'a HashMap<ConstId, Ty>,
    module: ModId,
    scopes: Vec<Vec<Local>>,
    /// Names bound by `is` whose branch has ended (E0050).
    expired: HashMap<String, Span>,
    ret: Ty,
    self_ty: Option<Ty>,
    generics: HashMap<String, ParamId>,
    vars: Vec<Option<Ty>>,
    interface: Option<DefId>,
    // Sprint S4.
    current_fn: Option<FnId>,
    /// The errors the current function, lambda or test may fail with.
    fn_fails: Vec<Ty>,
    /// The signature of the current function, for the fix of E0074.
    signature: Option<(Span, bool)>,
    try_frames: Vec<Frame>,
    /// The error types handled by enclosing `try:` blocks.
    handlers: Vec<Vec<Ty>>,
    /// The places of the types of those handlers, for the related locations of E0074 (C-8).
    handler_spans: Vec<Vec<Span>>,
    /// The function of the call being checked, taken by `call` (C-8).
    callee_fn: Option<FnId>,
    in_try_block: usize,
    loops: usize,
    /// The first scope of the current lambda: names below it are captured.
    lambda_floor: usize,
    /// The first scope of each enclosing lambda and whether it captures a value (an
    /// allocating closure, semantics 8.4).
    captures: Vec<(usize, bool)>,
    /// What has the effects of the code checked outside a function: a constant or the field
    /// defaults of a type.
    holder: Option<Holder>,
    /// The lambdas being checked, innermost last, as indexes into `Facts::lambdas`.
    lambda_stack: Vec<usize>,
    /// Each lambda checked, by its place, for the flows of function values (RFC 0005).
    lambda_ids: HashMap<(usize, usize), usize>,
    /// The struct or error of each field read, by the place of the read.
    field_owner: HashMap<(usize, usize), DefId>,
    /// The flows of the arguments of the last call checked, by parameter.
    last_args: Vec<(usize, Flow)>,
    self_var: bool,
    /// The name of the receiver of the `var self` method being called: it takes the place of a
    /// first `var` argument, so that it reaches no `var` parameter of the call (E0042, L-29).
    var_receiver: Option<String>,
    /// Expressions where a resource may be created (E0080).
    creation_ok: HashSet<(usize, usize)>,
    /// Names of resources used where they do not leave their block (E0081).
    use_ok: HashSet<(usize, usize)>,
    /// While positive, checks run again for their types only and report nothing.
    silent: usize,
    /// Steps left for the coverage check of the current `match` (semantics 6.5).
    coverage_budget: usize,
    /// Set when the alternatives of a pattern combine into more rows than are enumerated.
    pattern_overflow: std::cell::Cell<bool>,
    /// What the language server asked to record, before the types are resolved (T-10).
    noted: crate::lsp::facts::Noted,
}

impl Drop for Checker<'_> {
    /// Gives what was noted for the language server, with the types as far as they were
    /// inferred (T-10).
    fn drop(&mut self) {
        if !self.noted.is_empty() {
            let noted = std::mem::take(&mut self.noted);
            noted.finish(|t| self.resolved(t));
        }
    }
}

/// The number of pattern rows the coverage check of one `match` may examine. A match whose
/// patterns combine into more cases is not taken as covered and needs `case _` (semantics 6.5).
const COVERAGE_LIMIT: usize = 10_000;

impl<'a> Checker<'a> {
    fn new(
        p: &'a mut Program,
        known: &'a Known,
        const_types: &'a HashMap<ConstId, Ty>,
        module: ModId,
    ) -> Checker<'a> {
        Checker {
            p,
            known,
            const_types,
            module,
            scopes: Vec::new(),
            expired: HashMap::new(),
            ret: Ty::Unit,
            self_ty: None,
            generics: HashMap::new(),
            vars: Vec::new(),
            interface: None,
            current_fn: None,
            fn_fails: Vec::new(),
            signature: None,
            try_frames: Vec::new(),
            handlers: Vec::new(),
            handler_spans: Vec::new(),
            callee_fn: None,
            in_try_block: 0,
            loops: 0,
            lambda_floor: 0,
            captures: Vec::new(),
            holder: None,
            lambda_stack: Vec::new(),
            lambda_ids: HashMap::new(),
            field_owner: HashMap::new(),
            last_args: Vec::new(),
            self_var: false,
            var_receiver: None,
            creation_ok: HashSet::new(),
            use_ok: HashSet::new(),
            silent: 0,
            coverage_budget: COVERAGE_LIMIT,
            pattern_overflow: std::cell::Cell::new(false),
            noted: crate::lsp::facts::Noted::new(),
        }
    }

    fn err(&mut self, code: &'static str, span: Span, message: crate::diag::Msg) {
        if self.silent > 0 {
            return;
        }
        self.p
            .error(self.module, Diagnostic::error(code, span, message));
    }

    /// Like err, with the related locations of C-8.
    fn err_with(
        &mut self,
        code: &'static str,
        span: Span,
        message: crate::diag::Msg,
        related: impl IntoIterator<Item = Related>,
    ) {
        if self.silent > 0 {
            return;
        }
        let diagnostic = Diagnostic::error(code, span, message).with_all_related(related);
        self.p.error(self.module, diagnostic);
    }

    /// Like err_with, with the type the place expects, which the language server gives the
    /// editor for incomplete code (T-13).
    fn err_expecting(
        &mut self,
        code: &'static str,
        span: Span,
        message: crate::diag::Msg,
        expected: &Ty,
        related: impl IntoIterator<Item = Related>,
    ) {
        if self.silent > 0 {
            return;
        }
        let resolved = self.resolved(expected);
        let mut diagnostic = Diagnostic::error(code, span, message).with_all_related(related);
        if !resolved.is_partial() {
            diagnostic.expected = Some(self.p.show(&resolved));
        }
        self.p.error(self.module, diagnostic);
    }

    /// The span of the binding of a local name, if it has one.
    fn bound_span(&self, name: &str) -> Option<Span> {
        self.local_span(name)
            .filter(|span| *span != Span::default())
    }

    /// The binding of a local name as a related location (C-8).
    fn local_related(&self, name: &str) -> Option<Related> {
        let span = self.bound_span(name)?;
        let kind = self.find_local_full(name).map(|(_, _, k)| k);
        let message = if kind == Some(LocalKind::Using) {
            catalog::opened_by_using(name)
        } else {
            catalog::bound_here(name)
        };
        Some(Related::here(span, message))
    }

    /// The declaration of a node as a related location (C-8).
    fn node_related(&self, node: Node, message: crate::diag::Msg) -> Option<Related> {
        self.p.related_node(self.module, node, message)
    }

    /// The declarations of the user types among these, each once (C-8).
    fn types_related(&self, types: &[&Ty]) -> Vec<Related> {
        let mut found: Vec<Related> = Vec::new();
        for t in types {
            let t = self.resolved(t);
            if let Some(r) = self.p.related_type(self.module, &t) {
                if !found.contains(&r) {
                    found.push(r);
                }
            }
        }
        found
    }

    // ------------------------------------------------------------ functions

    fn function(&mut self, f: FnId, block: &ast::Block) {
        let def = &self.p.funcs[f];
        let fails = def.fails.clone();
        let name_span = def.span;
        self.current_fn = Some(f);
        self.signature = self.signature_span(f).map(|sp| (sp, !fails.is_empty()));
        let params = self.enter_function(f);
        self.scopes.push(Vec::new());
        for (i, (name, ty, var)) in params.into_iter().enumerate() {
            self.bind_param(f, i, name, ty, var);
        }
        let completes = self.block(block);
        let ret = self.resolved(&self.ret.clone());
        if completes && !matches!(ret, Ty::Unit | Ty::Never | Ty::Unknown) {
            let shown = self.p.show(&ret);
            let message = catalog::missing_return(&shown);
            self.err_expecting("E0082", name_span, message, &ret, None);
        }
    }

    /// Takes the generics, `Self`, result and errors of function `f` as the context of the
    /// checks, and gives its parameters (without `self`) with `Self` replaced.
    fn enter_function(&mut self, f: FnId) -> Vec<(String, Ty, bool)> {
        let def = &self.p.funcs[f];
        let mut generics: Vec<ParamId> = def.generics.clone();
        let owner = def.owner;
        let ret = def.ret.clone();
        let fails = def.fails.clone();
        self.self_var = def.self_param == Some(true);
        let params: Vec<(String, Ty, bool)> = def
            .params
            .iter()
            .map(|p| (p.name.clone(), p.ty.clone(), p.var))
            .collect();
        match owner {
            Owner::Impl(i) => {
                let target = self.p.impls[i].target.clone();
                generics.extend(self.p.impls[i].generics.iter().copied());
                self.self_ty = Some(target);
            }
            Owner::Interface(d) => {
                generics.extend(self.p.defs[d].generics.iter().copied());
                self.self_ty = Some(Ty::SelfTy);
                self.interface = Some(d);
            }
            Owner::Module => {}
        }
        for g in generics {
            self.generics.insert(self.p.params[g].name.clone(), g);
        }
        let self_ty = self.self_ty.clone();
        let subst = |t: &Ty| match &self_ty {
            Some(s) if !matches!(s, Ty::SelfTy) => t.subst(&HashMap::new(), Some(s)),
            _ => t.clone(),
        };
        self.ret = subst(&ret);
        self.fn_fails = fails.iter().map(&subst).collect();
        params
            .into_iter()
            .map(|(name, ty, var)| (name, subst(&ty), var))
            .collect()
    }

    /// Binds parameter `i` of function `f` in the innermost scope.
    fn bind_param(&mut self, f: FnId, i: usize, name: String, ty: Ty, var: bool) {
        let kind = if var {
            LocalKind::VarParam
        } else {
            LocalKind::Param
        };
        // A parameter holds what each call passes (RFC 0005).
        let flow = self.new_flow(vec![Atom::Param(Holder::Func(f), i)]);
        if self.noted.on {
            if let Some(param) = self.p.funcs[f].params.get(i) {
                let span = param.span;
                self.noted.local(self.module, &name, span, &ty, true);
            }
        }
        self.scopes.last_mut().unwrap().push(Local {
            name,
            ty,
            kind,
            span: Span::default(),
            read: std::cell::Cell::new(true),
            flow: Some(flow),
        });
    }

    /// The span of a function's signature, for the fix that extends its `fails with`.
    fn signature_span(&self, f: FnId) -> Option<Span> {
        let body = self.p.funcs[f].body?;
        match &self.p.modules[body.module].analysis.file.items[body.item] {
            ast::Item::Declaration(d) => match (&d.kind, body.member) {
                (ast::DeclarationKind::Function(func), None) => Some(func.signature.span),
                (ast::DeclarationKind::Interface(i), Some(m)) => Some(i.members[m].0.span),
                _ => None,
            },
            ast::Item::Extend(e) => body.member.map(|m| e.members[m].signature.span),
            _ => None,
        }
    }

    // ------------------------------------------------------------ scopes

    fn find_local(&self, name: &str) -> Option<Ty> {
        self.find_local_full(name).map(|(_, t, _)| t)
    }

    /// A local name: the index of its scope, its type and how it was introduced.
    fn find_local_full(&self, name: &str) -> Option<(usize, Ty, LocalKind)> {
        for (i, scope) in self.scopes.iter().enumerate().rev() {
            if let Some(local) = scope.iter().rev().find(|l| l.name == name) {
                return Some((i, local.ty.clone(), local.kind));
            }
        }
        None
    }

    fn bind(&mut self, name: &str, ty: Ty, span: Span) {
        self.bind_kind(name, ty, span, LocalKind::Let);
    }

    fn bind_kind(&mut self, name: &str, ty: Ty, span: Span, kind: LocalKind) {
        self.bind_flow(name, ty, span, kind, None);
    }

    /// Binds a local name whose values are followed when `flow` is given (RFC 0005).
    fn bind_flow(&mut self, name: &str, ty: Ty, span: Span, kind: LocalKind, flow: Option<usize>) {
        if name == "_" {
            return;
        }
        if self.find_local(name).is_some() {
            let first = self
                .bound_span(name)
                .map(|at| Related::here(at, catalog::first_declared_here(name)));
            self.err_with("E0055", span, catalog::local_repeats(name), first);
        }
        self.expired.remove(name);
        if self.noted.on {
            self.noted.local(self.module, name, span, &ty, false);
        }
        self.scopes.last_mut().unwrap().push(Local {
            name: name.to_string(),
            ty,
            kind,
            span,
            read: std::cell::Cell::new(false),
            flow,
        });
    }

    fn push(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn pop(&mut self) {
        let Some(scope) = self.scopes.pop() else {
            return;
        };
        for local in scope {
            if matches!(local.kind, LocalKind::Let | LocalKind::Var)
                && !local.read.get()
                && self.is_required_result(&local.ty)
            {
                let shown = self.p.show(&self.resolved(&local.ty));
                self.err(
                    "E0043",
                    local.span,
                    catalog::binding_never_read(&local.name, &shown),
                );
            }
        }
    }

    /// A `Result` or a `std.process.Output`: a value that must not be dropped (L-30).
    fn is_required_result(&self, ty: &Ty) -> bool {
        match self.resolved(ty) {
            Ty::Named(d, _) => {
                let def = &self.p.defs[d];
                Some(d) == prelude_def(self.p, "Result")
                    || (def.name == "Output" && self.p.modules[def.module].name == "std.process")
            }
            _ => false,
        }
    }

    /// Marks a local as read (L-30).
    fn mark_read(&self, name: &str) {
        for scope in self.scopes.iter().rev() {
            if let Some(local) = scope.iter().rev().find(|l| l.name == name) {
                local.read.set(true);
                return;
            }
        }
    }

    /// Ends the scope of `is` bindings, so that later uses report E0050.
    fn pop_expiring(&mut self, names: &[String]) {
        let spans: Vec<Option<Span>> = names.iter().map(|n| self.bound_span(n)).collect();
        self.pop();
        for (n, span) in names.iter().zip(spans) {
            if self.find_local(n).is_none() {
                self.expired.insert(n.clone(), span.unwrap_or_default());
            }
        }
    }

    // ------------------------------------------------------------ statements

    /// Checks a block; returns whether its end can be reached (docs/semantics.md 6.4).
    fn block(&mut self, block: &ast::Block) -> bool {
        self.push();
        let mut completes = true;
        for s in block {
            if !self.statement(s) {
                completes = false;
            }
        }
        self.pop();
        completes
    }

    fn statement(&mut self, s: &ast::Statement) -> bool {
        match &s.kind {
            StatementKind::Simple { statement, handler } => {
                // The inline handler runs instead of the rest of the statement: its name is in
                // scope only in its block.
                self.simple(statement, s.span, handler.as_ref())
            }
            StatementKind::If {
                branches,
                otherwise,
            } => {
                let mut completes = otherwise.is_none();
                for (condition, body) in branches {
                    let bindings = self.condition(condition);
                    self.push();
                    let names = self.bind_all(bindings);
                    completes |= self.block(body);
                    self.pop_expiring(&names);
                }
                if let Some(body) = otherwise {
                    completes |= self.block(body);
                }
                completes
            }
            StatementKind::For {
                names,
                iterable,
                body,
            } => {
                let ty = self.expr(iterable, &Ty::Unknown);
                let element = self.element_of(&ty, iterable.span);
                self.push();
                self.bind_destructured(names, element, iterable.span);
                self.loops += 1;
                self.block(body);
                self.loops -= 1;
                self.pop();
                true
            }
            StatementKind::While { condition, body } => {
                let bindings = self.condition(condition);
                self.push();
                let names = self.bind_all(bindings);
                self.loops += 1;
                self.block(body);
                self.loops -= 1;
                self.pop_expiring(&names);
                let forever = matches!(strip(condition).kind, ExprKind::Bool(true));
                !forever || has_break(body)
            }
            StatementKind::Match { subject, cases } => {
                let ty = self.expr(subject, &Ty::Unknown);
                let mut completes = false;
                for case in cases {
                    self.push();
                    let mut bindings = Vec::new();
                    self.pattern(&case.pattern, &ty, &mut bindings);
                    for (n, t, span) in bindings {
                        self.bind(&n, t, span);
                    }
                    let mut names = Vec::new();
                    if let Some(guard) = &case.guard {
                        let b = self.condition(guard);
                        names = self.bind_all(b);
                    }
                    completes |= self.block(&case.body);
                    self.pop_expiring(&names);
                }
                // A match that is not exhaustive is reported once (E0068), not again as a
                // reachable end of the function.
                self.check_exhaustive(&ty, cases, s.span);
                completes
            }
            StatementKind::Try { body, handlers } => {
                let types: Vec<Ty> = handlers
                    .iter()
                    .map(|h| self.resolve_type(&h.ty, false))
                    .collect();
                self.handlers.push(types.clone());
                self.handler_spans
                    .push(handlers.iter().map(|h| h.ty.span).collect());
                self.in_try_block += 1;
                let mut completes = self.block(body);
                self.in_try_block -= 1;
                self.handlers.pop();
                self.handler_spans.pop();
                for (h, ty) in handlers.iter().zip(types) {
                    self.push();
                    if let Some(name) = &h.name {
                        self.bind(&name.text, ty, name.span);
                    }
                    completes |= self.block(&h.body);
                    self.pop();
                }
                completes
            }
            StatementKind::Using { name, value, body } => {
                self.allow_creation(value);
                let ty = self.expr(value, &Ty::Unknown);
                self.implicit_call(&ty, "Resource", "release", value.span);
                self.push();
                self.bind_kind(&name.text, ty, name.span, LocalKind::Using);
                let completes = self.block(body);
                self.pop();
                completes
            }
        }
    }

    fn handler_of(e: &ast::Expr) -> Option<(ast::Type, Option<ast::Name>)> {
        match &e.kind {
            ExprKind::Try {
                handler: Some(h), ..
            } => Some(h.clone()),
            ExprKind::Try { operand, .. } => Checker::handler_of(operand),
            ExprKind::Binary { right, .. } => Checker::handler_of(right),
            ExprKind::Paren(inner) => Checker::handler_of(inner),
            _ => None,
        }
    }

    fn inline_handler(&mut self, e: &ast::Expr, block: Option<&ast::Block>, span: Span) {
        let Some(block) = block else { return };
        let Some((ty, name)) = Checker::handler_of(e) else {
            self.block(block);
            return;
        };
        let head = ty.span;
        let ty = self.resolve_type(&ty, false);
        self.push();
        if let Some(name) = name {
            self.bind(&name.text, ty, name.span);
        }
        // The block replaces the value of the expression, so it must not complete (L-14a).
        if self.block(block) {
            let related = Related::here(head, catalog::inline_handler_here());
            self.err_with("E0075", span, catalog::handler_completes(), Some(related));
        }
        self.pop();
    }

    fn is_never(&self, t: &Ty) -> bool {
        matches!(self.resolved(t), Ty::Never)
    }

    /// Checks a simple statement; returns whether the statement after it can be reached.
    fn simple(&mut self, s: &ast::Simple, span: Span, handler: Option<&ast::Block>) -> bool {
        match s {
            ast::Simple::Let {
                var,
                name,
                ty,
                value,
            } => {
                let annotated = ty.as_ref().map(|t| self.resolve_type(t, true));
                let found = self.expr(value, annotated.as_ref().unwrap_or(&Ty::Unknown));
                self.inline_handler(value, handler, span);
                let never = self.is_never(&found);
                let ty = match annotated {
                    Some(expected) => {
                        self.expect(&found, &expected, value.span);
                        expected
                    }
                    None => {
                        if is_undetermined(value) {
                            self.err("E0060", name.span, catalog::cannot_infer());
                        }
                        found
                    }
                };
                // A resource held by `let` has been reported (E0080 or E0081); the name then
                // behaves like a `using` name, so that its methods give no further messages.
                let kind = if self.is_resource(&self.resolved(&ty)) {
                    LocalKind::Using
                } else if *var {
                    LocalKind::Var
                } else {
                    LocalKind::Let
                };
                let values = self.value_of(value, &ty);
                let flow = self.new_flow(values);
                self.bind_flow(&name.text, ty, name.span, kind, Some(flow));
                !never
            }
            ast::Simple::Assign { target, value } => {
                self.check_assignment(target);
                let expected = self.expr(target, &Ty::Unknown);
                let found = self.expr(value, &expected);
                self.inline_handler(value, handler, span);
                self.expect(&found, &expected, value.span);
                self.assigned(target, value, &expected);
                !self.is_never(&found)
            }
            ast::Simple::Return(value) => {
                let ret = self.ret.clone();
                match value {
                    Some(v) => {
                        if self.is_resource(&self.resolved(&ret)) {
                            self.allow_creation(v);
                        }
                        let found = self.expr(v, &ret);
                        self.inline_handler(v, handler, span);
                        self.returned(v, &found);
                        if ret == Ty::Unit {
                            self.err("E0061", v.span, catalog::returns_nothing());
                        } else {
                            self.expect(&found, &ret, v.span);
                        }
                    }
                    None => {
                        if ret != Ty::Unit && ret != Ty::Never && !ret.is_unknown() {
                            let shown = self.p.show(&ret);
                            let message = catalog::missing_return_value(&shown);
                            self.err_expecting("E0061", span, message, &ret, None);
                        }
                    }
                }
                false
            }
            ast::Simple::Fail(e) => {
                let t = self.expr(e, &Ty::Unknown);
                let t = self.resolved(&t);
                match &t {
                    Ty::Named(d, _) if matches!(self.p.defs[*d].kind, DefKind::Error(_)) => {
                        self.raise(vec![t.clone()], e.span);
                    }
                    Ty::Unknown | Ty::Never => {}
                    other => {
                        let shown = self.p.show(other);
                        self.err("E0061", e.span, catalog::type_mismatch("error", &shown));
                    }
                }
                false
            }
            ast::Simple::Assert(e) => {
                let ty = self.expr(e, &Ty::bool());
                self.expect(&ty, &Ty::bool(), e.span);
                true
            }
            ast::Simple::Break | ast::Simple::Continue => {
                if self.loops == 0 {
                    let word = if matches!(s, ast::Simple::Break) {
                        "break"
                    } else {
                        "continue"
                    };
                    self.err("E0083", span, catalog::outside_loop(word));
                }
                false
            }
            ast::Simple::Pass => true,
            ast::Simple::Expr(e) => {
                let t = self.expr(e, &Ty::Unknown);
                self.inline_handler(e, handler, span);
                let t = self.resolved(&t);
                if is_call(e) && self.is_required_result(&t) {
                    let shown = self.p.show(&t);
                    self.err("E0043", e.span, catalog::result_discarded(&shown));
                }
                !matches!(t, Ty::Never)
            }
        }
    }

    // ------------------------------------------------------------ errors (section 5)

    /// Whether a handler (or a `fails with` entry) of type `handler` receives the error `err`.
    fn catches(&self, handler: &Ty, err: &Ty) -> bool {
        let error_iface = prelude_def(self.p, "Error");
        match (&self.resolved(handler), &self.resolved(err)) {
            (Ty::Unknown, _) | (_, Ty::Unknown) | (_, Ty::Never) => true,
            (Ty::Named(h, _), _) if Some(*h) == error_iface => true,
            (Ty::Named(h, _), Ty::Named(e, _)) => h == e,
            (Ty::Param(h), Ty::Param(e)) => h == e,
            _ => false,
        }
    }

    /// Errors that leave an expression: handled by an enclosing `try:`, or listed in the
    /// `fails with` of the function (L-15); otherwise E0074 with a fix that lists them.
    fn raise(&mut self, errors: Vec<Ty>, span: Span) {
        if self.silent > 0 {
            return;
        }
        for err in errors {
            let err = self.resolved(&err);
            if self
                .handlers
                .iter()
                .any(|hs| hs.iter().any(|h| self.catches(h, &err)))
                || self.fn_fails.iter().any(|f| self.catches(f, &err))
            {
                continue;
            }
            let shown = self.p.show(&err);
            let mut diagnostic =
                Diagnostic::error("E0074", span, catalog::error_not_handled(&shown));
            // The handlers of the enclosing `try:` blocks, innermost first, and the signature
            // whose `fails with` list may take the error (C-8).
            for (types, spans) in self.handlers.iter().zip(&self.handler_spans).rev() {
                for (t, at) in types.iter().zip(spans) {
                    let shown = self.p.show(&self.resolved(t));
                    diagnostic
                        .related
                        .push(Related::here(*at, catalog::handler_here(&shown)));
                }
            }
            if let (Some((sig, _)), Some(f)) = (self.signature, self.current_fn) {
                let name = crate::symbol::qualified_name(self.p, f);
                diagnostic.related.push(
                    Related::here(sig, catalog::enclosing_function_here(&self.p.funcs[f].name))
                        .with_symbol(name),
                );
            }
            if let Some((sig, has_fails)) = self.signature {
                let at = Span::new(sig.end, sig.end);
                let text = if has_fails {
                    format!(", {shown}")
                } else {
                    format!(" fails with {shown}")
                };
                diagnostic.fixes.push(Fix {
                    span: at,
                    new_text: text,
                    description: catalog::add_to_fails(&shown),
                });
            }
            self.p.error(self.module, diagnostic);
        }
    }

    /// A call of a function that can fail: inside `try`, inside a `try:` block, or E0073.
    fn fallible_call(&mut self, fails: Vec<Ty>, span: Span, callee: Option<FnId>) {
        if fails.is_empty() || self.silent > 0 {
            return;
        }
        if let Some(frame) = self.try_frames.last_mut() {
            frame.errors.extend(fails);
            return;
        }
        if self.in_try_block > 0 {
            self.raise(fails, span);
            return;
        }
        let shown: Vec<String> = fails.iter().map(|t| self.p.show(t)).collect();
        let related = callee.and_then(|f| {
            let name = self.p.funcs[f].name.clone();
            self.node_related(Node::Func(f), catalog::fails_with_here(&name))
        });
        self.err_with(
            "E0073",
            span,
            catalog::call_needs_try(&shown.join(", ")),
            related,
        );
    }

    fn uncertain(&mut self) {
        if let Some(frame) = self.try_frames.last_mut() {
            frame.uncertain = true;
        }
    }

    // ------------------------------------------------------------ values (section 6)

    /// Where a place starts, and the first field on the way that is not declared `var`.
    fn place(&mut self, e: &ast::Expr) -> (Root, Option<String>) {
        match &e.kind {
            ExprKind::Paren(inner) => self.place(inner),
            ExprKind::Name(n) => match self.find_local_full(n) {
                Some((_, _, kind)) => (Root::Local(n.clone(), kind), None),
                None => (Root::Other, None),
            },
            ExprKind::SelfValue => (Root::SelfValue(self.self_var), None),
            ExprKind::Index { target, .. } => self.place(target),
            ExprKind::Field { target, name } => {
                if self.module_ref(target).is_some() || self.type_ref(target).is_some() {
                    return (Root::Other, None);
                }
                let (root, bad) = self.place(target);
                if bad.is_some() {
                    return (root, bad);
                }
                self.silent += 1;
                let t = self.expr(target, &Ty::Unknown);
                self.silent -= 1;
                let var = match self.resolved(&t) {
                    Ty::Named(d, _) => match &self.p.defs[d].kind {
                        DefKind::Struct(f) | DefKind::Error(f) => {
                            f.iter().find(|f| f.name == name.text).is_none_or(|f| f.var)
                        }
                        _ => true,
                    },
                    _ => true,
                };
                (root, (!var).then(|| name.text.clone()))
            }
            _ => (Root::Other, None),
        }
    }

    fn mutable_root(root: &Root) -> bool {
        matches!(
            root,
            Root::Local(_, LocalKind::Var | LocalKind::VarParam | LocalKind::Using)
                | Root::SelfValue(true)
        )
    }

    fn root_name(root: &Root) -> String {
        match root {
            Root::Local(n, _) => n.clone(),
            Root::SelfValue(_) => "self".to_string(),
            Root::Other => "the value".to_string(),
        }
    }

    /// Checks that a place can change (L-10, L-11): E0077 for a name, E0078 for a part of one.
    fn check_assignment(&mut self, target: &ast::Expr) {
        if let ExprKind::Name(n) = &strip(target).kind {
            match self.find_local_full(n) {
                Some((_, _, LocalKind::Var | LocalKind::VarParam)) => {}
                Some(_) => {
                    let related = self.local_related(n);
                    self.err_with("E0077", target.span, catalog::assign_immutable(n), related);
                }
                None => {
                    if let Some((_, symbol)) = self.p.lookup(self.module, n) {
                        let node = crate::program::symbol_node(symbol);
                        let related = self.node_related(node, catalog::declared_here(n));
                        self.err_with("E0077", target.span, catalog::assign_immutable(n), related);
                    }
                }
            }
            return;
        }
        self.check_changeable(target, target.span);
    }

    fn check_changeable(&mut self, place: &ast::Expr, span: Span) {
        let (root, bad) = self.place(place);
        if !Checker::mutable_root(&root) {
            let name = Checker::root_name(&root);
            let related = match &root {
                Root::Local(n, _) => self.local_related(n),
                _ => None,
            };
            self.err_with("E0078", span, catalog::not_mutable(&name), related);
        } else if let Some(field) = bad {
            self.err("E0078", span, catalog::field_not_var(&field));
        }
    }

    // ------------------------------------------------------------ resources (section 7)

    /// Marks an expression where a resource may be created: `using`, `return` of a function
    /// that returns a resource, a field of a resource.
    fn allow_creation(&mut self, e: &ast::Expr) {
        self.creation_ok.insert(span_key(e.span));
        match &e.kind {
            ExprKind::Paren(inner) => self.allow_creation(inner),
            ExprKind::Try { operand, .. } => self.allow_creation(operand),
            _ => {}
        }
    }

    fn check_creation(&mut self, ty: &Ty, span: Span) {
        if self.silent > 0 || self.creation_ok.contains(&span_key(span)) {
            return;
        }
        let ty = self.resolved(ty);
        if self.is_resource(&ty) {
            let shown = self.p.show(&ty);
            self.err("E0080", span, catalog::resource_outside_using(&shown));
        }
    }

    // ------------------------------------------------------------ match (section 6.5)

    /// Reports E0068 when the cases do not cover every value; returns whether they do.
    fn check_exhaustive(&mut self, subject: &Ty, cases: &[ast::Case], span: Span) -> bool {
        let ty = self.resolved(subject);
        if ty.is_partial() || matches!(ty, Ty::Never) {
            return true;
        }
        let mut rows = Vec::new();
        self.pattern_overflow.set(false);
        for case in cases.iter().filter(|c| c.guard.is_none()) {
            for p in self.abstract_pattern(&case.pattern, &ty) {
                rows.push(vec![p]);
            }
        }
        self.coverage_budget = COVERAGE_LIMIT;
        let missing = self.missing(rows, vec![ty.clone()], 0);
        if missing.is_some() && self.pattern_overflow.get() || self.coverage_budget == 0 {
            self.err("E0068", span, catalog::match_too_many_cases());
            return false;
        }
        match missing {
            None => true,
            Some(witness) => {
                let example = witness.first().cloned().unwrap_or_else(|| "_".to_string());
                let related = self.types_related(&[&ty]);
                self.err_with(
                    "E0068",
                    span,
                    catalog::match_not_exhaustive(&example),
                    related,
                );
                false
            }
        }
    }

    fn pattern_variant(&self, path: &[ast::Name], subject: &Ty) -> Option<(DefId, usize)> {
        let last = &path.last()?.text;
        if let Some(found) = self.variant_in(subject, last) {
            return Some(found);
        }
        if path.len() == 1 && last.chars().next().is_some_and(char::is_uppercase) {
            return self.short_variants(last).first().copied();
        }
        None
    }

    fn variant_field_types(&self, d: DefId, v: usize, subject: &Ty) -> Vec<Ty> {
        let args = match subject {
            Ty::Named(sd, args) if *sd == d => args.clone(),
            _ => self.p.defs[d]
                .generics
                .iter()
                .map(|_| Ty::Unknown)
                .collect(),
        };
        let subst: HashMap<ParamId, Ty> =
            self.p.defs[d].generics.iter().copied().zip(args).collect();
        match &self.p.defs[d].kind {
            DefKind::Enum(vs) => vs[v]
                .fields
                .iter()
                .map(|(_, t)| t.subst(&subst, None))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The alternatives a pattern stands for.
    fn abstract_pattern(&self, p: &ast::Pattern, subject: &Ty) -> Vec<Pat> {
        match &p.kind {
            PatternKind::Wildcard => vec![Pat::Wild],
            PatternKind::Literal(ast::Literal::Bool(b)) => vec![Pat::Bool(*b)],
            PatternKind::Literal(_) => vec![Pat::Literal],
            PatternKind::Name(path) => match self.pattern_variant(path, subject) {
                Some((d, v)) => {
                    let k = self.variant_field_types(d, v, subject).len();
                    vec![Pat::Variant(d, v, vec![Pat::Wild; k])]
                }
                None => vec![Pat::Wild],
            },
            PatternKind::Constructor { name, arguments } => {
                let Some((d, v)) = self.pattern_variant(name, subject) else {
                    return vec![Pat::Wild];
                };
                let fields = self.variant_field_types(d, v, subject);
                let names: Vec<String> = match &self.p.defs[d].kind {
                    DefKind::Enum(vs) => vs[v].fields.iter().map(|(n, _)| n.clone()).collect(),
                    _ => Vec::new(),
                };
                let mut columns: Vec<Vec<Pat>> = vec![vec![Pat::Wild]; fields.len()];
                let mut position = 0;
                for (label, q) in arguments {
                    let index = match label {
                        Some(l) => names.iter().position(|n| *n == l.text),
                        None => {
                            position += 1;
                            Some(position - 1)
                        }
                    };
                    if let Some(i) = index.filter(|i| *i < fields.len()) {
                        columns[i] = self.abstract_pattern(q, &fields[i]);
                    }
                }
                product(&columns, &self.pattern_overflow)
                    .into_iter()
                    .map(|args| Pat::Variant(d, v, args))
                    .collect()
            }
            PatternKind::List(elements) => {
                let item = match subject {
                    Ty::List(t) => (**t).clone(),
                    _ => Ty::Unknown,
                };
                let mut columns = Vec::new();
                let mut rest = false;
                for el in elements {
                    match el {
                        ast::ListElement::Rest(_) => rest = true,
                        ast::ListElement::Pattern(q) => {
                            columns.push(self.abstract_pattern(q, &item))
                        }
                    }
                }
                product(&columns, &self.pattern_overflow)
                    .into_iter()
                    .map(|items| Pat::List(items, rest))
                    .collect()
            }
            PatternKind::Tuple(items) => {
                let types = match subject {
                    Ty::Tuple(ts) if ts.len() == items.len() => ts.clone(),
                    _ => vec![Ty::Unknown; items.len()],
                };
                let columns: Vec<Vec<Pat>> = items
                    .iter()
                    .zip(&types)
                    .map(|(q, t)| self.abstract_pattern(q, t))
                    .collect();
                product(&columns, &self.pattern_overflow)
                    .into_iter()
                    .map(Pat::Tuple)
                    .collect()
            }
            PatternKind::Bind { pattern, .. } => self.abstract_pattern(pattern, subject),
            PatternKind::Or(alternatives) => alternatives
                .iter()
                .flat_map(|a| self.abstract_pattern(a, subject))
                .collect(),
        }
    }

    /// A value that no row matches, written as patterns for the columns; None if every value
    /// is matched.
    fn missing(
        &mut self,
        rows: Vec<Vec<Pat>>,
        types: Vec<Ty>,
        depth: usize,
    ) -> Option<Vec<String>> {
        if types.is_empty() {
            return rows.is_empty().then(Vec::new);
        }
        // Too many cases to enumerate: not covered (semantics 6.5); the caller reports it.
        self.coverage_budget = self.coverage_budget.saturating_sub(rows.len().max(1));
        if self.coverage_budget == 0 {
            return Some(vec!["_".to_string(); types.len()]);
        }
        let _ = depth;
        let ty = self.resolved(&types[0]);
        let rest_types = types[1..].to_vec();
        // When no row names a constructor in the first column, only the rows that start with
        // `_` can match, whatever the value: the witness is `_` followed by theirs.
        if !rows.is_empty() && rows.iter().all(|row| matches!(row[0], Pat::Wild)) {
            let defaults: Vec<Vec<Pat>> = rows.iter().map(|row| row[1..].to_vec()).collect();
            return self
                .missing(defaults, rest_types, depth + 1)
                .map(|w| [vec!["_".to_string()], w].concat());
        }
        let enum_variants = match &ty {
            Ty::Named(d, _) => match &self.p.defs[*d].kind {
                DefKind::Enum(vs) => Some((*d, vs.len())),
                _ => None,
            },
            _ => None,
        };
        if let Some((d, count)) = enum_variants {
            for v in 0..count {
                let fields = self.variant_field_types(d, v, &ty);
                let k = fields.len();
                let specialized: Vec<Vec<Pat>> = rows
                    .iter()
                    .filter_map(|row| match &row[0] {
                        Pat::Wild => Some([vec![Pat::Wild; k], row[1..].to_vec()].concat()),
                        Pat::Variant(d2, v2, args) if *d2 == d && *v2 == v => {
                            Some([args.clone(), row[1..].to_vec()].concat())
                        }
                        _ => None,
                    })
                    .collect();
                let mut next = fields.clone();
                next.extend(rest_types.clone());
                if let Some(w) = self.missing(specialized, next, depth + 1) {
                    let name = match &self.p.defs[d].kind {
                        DefKind::Enum(vs) => vs[v].name.clone(),
                        _ => String::new(),
                    };
                    let shown = if k == 0 {
                        name
                    } else {
                        format!("{name}({})", w[..k].join(", "))
                    };
                    return Some([vec![shown], w[k..].to_vec()].concat());
                }
            }
            return None;
        }
        match &ty {
            Ty::Prim(Prim::Bool) => {
                for b in [true, false] {
                    let specialized: Vec<Vec<Pat>> = rows
                        .iter()
                        .filter_map(|row| match &row[0] {
                            Pat::Wild => Some(row[1..].to_vec()),
                            Pat::Bool(x) if *x == b => Some(row[1..].to_vec()),
                            _ => None,
                        })
                        .collect();
                    if let Some(w) = self.missing(specialized, rest_types.clone(), depth + 1) {
                        return Some([vec![b.to_string()], w].concat());
                    }
                }
                None
            }
            Ty::Tuple(ts) => {
                let k = ts.len();
                let specialized: Vec<Vec<Pat>> = rows
                    .iter()
                    .filter_map(|row| match &row[0] {
                        Pat::Wild => Some([vec![Pat::Wild; k], row[1..].to_vec()].concat()),
                        Pat::Tuple(items) if items.len() == k => {
                            Some([items.clone(), row[1..].to_vec()].concat())
                        }
                        _ => None,
                    })
                    .collect();
                let mut next = ts.clone();
                next.extend(rest_types.clone());
                self.missing(specialized, next, depth + 1)
                    .map(|w| [vec![format!("({})", w[..k].join(", "))], w[k..].to_vec()].concat())
            }
            Ty::List(item) => {
                let longest = rows
                    .iter()
                    .filter_map(|row| match &row[0] {
                        Pat::List(items, _) => Some(items.len()),
                        _ => None,
                    })
                    .max()
                    .unwrap_or(0);
                for k in 0..=longest + 1 {
                    let specialized: Vec<Vec<Pat>> = rows
                        .iter()
                        .filter_map(|row| match &row[0] {
                            Pat::Wild => Some([vec![Pat::Wild; k], row[1..].to_vec()].concat()),
                            Pat::List(items, false) if items.len() == k => {
                                Some([items.clone(), row[1..].to_vec()].concat())
                            }
                            Pat::List(items, true) if items.len() <= k => Some(
                                [
                                    items.clone(),
                                    vec![Pat::Wild; k - items.len()],
                                    row[1..].to_vec(),
                                ]
                                .concat(),
                            ),
                            _ => None,
                        })
                        .collect();
                    let mut next = vec![(**item).clone(); k];
                    next.extend(rest_types.clone());
                    if let Some(w) = self.missing(specialized, next, depth + 1) {
                        let mut items = w[..k].to_vec();
                        if k == longest + 1 {
                            items.push("...".to_string());
                        }
                        return Some(
                            [vec![format!("[{}]", items.join(", "))], w[k..].to_vec()].concat(),
                        );
                    }
                }
                None
            }
            _ => {
                let defaults: Vec<Vec<Pat>> = rows
                    .iter()
                    .filter(|row| matches!(row[0], Pat::Wild))
                    .map(|row| row[1..].to_vec())
                    .collect();
                self.missing(defaults, rest_types, depth + 1)
                    .map(|w| [vec!["_".to_string()], w].concat())
            }
        }
    }

    // ------------------------------------------------------------ calls, layers and effects

    /// Records a call for the call graph, `unio symbol`, the effects with the flows of its
    /// arguments (RFC 0005), and the layer rule (E0084).
    fn record_call(&mut self, f: FnId, span: Span, args: Vec<(usize, Flow)>) {
        if self.silent > 0 {
            return;
        }
        self.record_reference(f, span);
        self.add_use(Callee::Holder(Holder::Func(f)), args, span);
    }

    /// Records that a function is named, called or not: `unio symbol`, the content hashes and
    /// the layer rule. Naming a function as a value has none of its effects (RFC 0005).
    fn record_reference(&mut self, f: FnId, span: Span) {
        if self.silent > 0 {
            return;
        }
        self.p.usages.push((f, self.module, span));
        if let Some(caller) = self.current_fn {
            self.p.calls.push(CallEdge {
                caller,
                callee: f,
                module: self.module,
                span,
            });
        }
        let callee_module = self.p.funcs[f].module;
        if let (Some(here), Some(there)) = (
            self.p.module_layer(self.module),
            self.p.module_layer(callee_module),
        ) {
            if there > here {
                let name = self.p.funcs[f].name.clone();
                // The standard library is left out of related locations unless it is the
                // code being checked, as a layer violation is (C-8).
                let related = Related {
                    symbol: Some(crate::symbol::qualified_name(self.p, f)),
                    message: catalog::layer_function_here(&name),
                    file: (callee_module != self.module)
                        .then(|| self.p.modules[callee_module].analysis.path.clone()),
                    span: self.p.funcs[f].span,
                };
                self.err_with(
                    "E0084",
                    span,
                    catalog::layer_violation(&name, LAYER_NAMES[there], LAYER_NAMES[here]),
                    Some(related),
                );
            }
        }
    }

    /// What has the effects of the code being checked: the innermost lambda, the function,
    /// the constant or type whose initializer or field defaults are checked, or the test that
    /// runs (a test that does not run has none, and what it records is forgotten).
    fn effect_holder(&self) -> Option<Holder> {
        if let Some(&l) = self.lambda_stack.last() {
            return Some(Holder::Lambda(l));
        }
        self.current_fn.map(Holder::Func).or(self.holder)
    }

    fn add_use(&mut self, callee: Callee, args: Vec<(usize, Flow)>, span: Span) {
        if self.silent > 0 {
            return;
        }
        let Some(from) = self.effect_holder() else {
            return;
        };
        if matches!(&callee, Callee::Holder(h) if *h == from) && args.is_empty() {
            return;
        }
        self.p.effect_facts.uses.push(Use {
            from,
            callee,
            args,
            module: self.module,
            span,
        });
    }

    /// A use that is not a recorded call: an implicit call through an interface, or a use of
    /// a constant or of field defaults (semantics 8.4).
    fn link(&mut self, to: Node, span: Span) {
        self.add_use(Callee::Holder(Holder::from(to)), Vec::new(), span);
    }

    /// A local whose values are followed, holding the given values first (RFC 0005).
    fn new_flow(&mut self, flow: Flow) -> usize {
        self.p.effect_facts.locals.push(flow);
        self.p.effect_facts.locals.len() - 1
    }

    /// The local a name refers to, with the index of its values if they are followed.
    fn local_flow(&self, name: &str) -> Option<Option<usize>> {
        for scope in self.scopes.iter().rev() {
            if let Some(local) = scope.iter().rev().find(|l| l.name == name) {
                return Some(local.flow);
            }
        }
        None
    }

    /// The function values an expression of type `ty` may be, as far as its form shows them
    /// (RFC 0005). An expression whose values are not followed is any value of its type.
    fn value_of(&mut self, e: &ast::Expr, ty: &Ty) -> Flow {
        let ty = self.resolved(ty);
        if !effects::may_hold_function(&ty) {
            return Vec::new();
        }
        let unknown = vec![Atom::Top(ty.clone())];
        match &e.kind {
            ExprKind::Paren(inner) => self.value_of(inner, &ty),
            ExprKind::Try { operand, .. } => self.value_of(operand, &ty),
            ExprKind::Name(n) => match self.local_flow(n) {
                Some(Some(id)) => vec![Atom::Local(id)],
                Some(None) => unknown,
                None => match self.p.lookup(self.module, n) {
                    Some((_, Symbol::Func(f))) => vec![Atom::Fn(f)],
                    Some((_, Symbol::Const(c))) => vec![Atom::Const(c)],
                    _ => unknown,
                },
            },
            ExprKind::Field { target, name } => {
                if let Some(m) = self.module_ref(target) {
                    return match self.p.modules[m].symbols.get(&name.text) {
                        Some(Symbol::Func(f)) => vec![Atom::Fn(*f)],
                        Some(Symbol::Const(c)) => vec![Atom::Const(*c)],
                        _ => unknown,
                    };
                }
                match self.field_owner.get(&span_key(e.span)) {
                    Some(&d) => vec![Atom::Field(d, name.text.clone())],
                    None => unknown,
                }
            }
            ExprKind::Lambda { .. } => match self.lambda_ids.get(&span_key(e.span)) {
                Some(&l) => vec![Atom::Lambda(l)],
                None => unknown,
            },
            ExprKind::Conditional {
                then,
                condition,
                otherwise,
            } if !mentions_is(condition) => {
                let mut flow = self.value_of(then, &ty);
                flow.extend(self.value_of(otherwise, &ty));
                flow
            }
            ExprKind::Call { callee, .. } | ExprKind::GenericCall { callee, .. } => {
                match self.named_function(callee) {
                    Some(f) if matches!(self.p.funcs[f].ret, Ty::Func(_)) => vec![Atom::Ret(f)],
                    _ => unknown,
                }
            }
            _ => unknown,
        }
    }

    /// The function a callee names, if it names one: `f` or `alias.f`.
    fn named_function(&mut self, callee: &ast::Expr) -> Option<FnId> {
        match &callee.kind {
            ExprKind::Name(n) if self.local_flow(n).is_none() => {
                match self.p.lookup(self.module, n) {
                    Some((_, Symbol::Func(f))) => Some(f),
                    _ => None,
                }
            }
            ExprKind::Field { target, name } => {
                let m = self.module_ref(target)?;
                match self.p.modules[m].symbols.get(&name.text) {
                    Some(Symbol::Func(f)) => Some(*f),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// An assignment: the local or the field gets the values of the expression (RFC 0005).
    fn assigned(&mut self, target: &ast::Expr, value: &ast::Expr, ty: &Ty) {
        if self.silent > 0 {
            return;
        }
        let flow = self.value_of(value, ty);
        if flow.is_empty() {
            return;
        }
        match &strip(target).kind {
            ExprKind::Name(n) => {
                if let Some(Some(id)) = self.local_flow(n) {
                    self.p.effect_facts.locals[id].extend(flow);
                }
            }
            ExprKind::Field { name, .. } => {
                if let Some(&d) = self.field_owner.get(&span_key(strip(target).span)) {
                    self.store(d, &name.text, flow);
                }
            }
            _ => {}
        }
    }

    /// A value stored in a field of a struct or error (RFC 0005).
    fn store(&mut self, d: DefId, field: &str, flow: Flow) {
        if self.silent > 0 || flow.is_empty() {
            return;
        }
        self.p
            .effect_facts
            .stores
            .entry((d, field.to_string()))
            .or_default()
            .extend(flow);
    }

    /// A value a function returns (RFC 0005).
    fn returned(&mut self, value: &ast::Expr, ty: &Ty) {
        if self.silent > 0 || !self.lambda_stack.is_empty() {
            return;
        }
        let Some(f) = self.current_fn else { return };
        if !matches!(self.p.funcs[f].ret, Ty::Func(_)) {
            return;
        }
        let flow = self.value_of(value, ty);
        self.p
            .effect_facts
            .returns
            .entry(f)
            .or_default()
            .extend(flow);
    }

    /// An expression that allocates: `allocates` and the layer `alloc` for what holds it, and
    /// E0084 in a module of the `core` layer (S-11, semantics 8.4).
    fn allocates(&mut self, what: Allocation, span: Span) {
        if self.silent > 0 {
            return;
        }
        if let Some(at) = self.effect_holder() {
            self.p.effect_facts.sites.push(Site {
                at,
                what,
                module: self.module,
                span,
            });
        }
        if self.p.module_layer(self.module) == Some(0) {
            self.err(
                "E0084",
                span,
                catalog::layer_allocation(what, LAYER_NAMES[0]),
            );
        }
    }

    /// A call the language makes through an interface: `==` through `Equal`, `<` through
    /// `Order`, `+` through `Add`, interpolation through `Display`, `using` through `Resource`.
    /// The method of a known type is the one called; for any other type every implementation
    /// may be (the interface method, whose effects are their union). Built-in types call none.
    fn implicit_call(&mut self, ty: &Ty, interface: &str, method: &str, span: Span) {
        if self.silent > 0 {
            return;
        }
        let ty = self.resolved(ty);
        if matches!(ty, Ty::Prim(_) | Ty::Unknown | Ty::Never | Ty::Unit) {
            return;
        }
        let direct = match ty {
            Ty::Named(..) | Ty::Param(_) => self.find_method(&ty, method).map(|(f, _, _)| f),
            _ => None,
        };
        let called = direct.or_else(|| {
            let i = prelude_iface(self.p, interface)?;
            match &self.p.defs[i].kind {
                DefKind::Interface(fs) => {
                    fs.iter().copied().find(|f| self.p.funcs[*f].name == method)
                }
                _ => None,
            }
        });
        if let Some(f) = called {
            self.link(Node::Func(f), span);
        }
    }

    /// A local name used inside lambdas: each lambda whose first scope is above the name's
    /// captures it.
    fn note_capture(&mut self, scope: usize) {
        for (floor, captured) in &mut self.captures {
            if scope < *floor {
                *captured = true;
            }
        }
    }

    fn bind_all(&mut self, bindings: Vec<(String, Ty, Span)>) -> Vec<String> {
        let mut names = Vec::new();
        for (n, t, span) in bindings {
            self.bind(&n, t, span);
            names.push(n);
        }
        names
    }

    fn bind_destructured(&mut self, names: &[ast::Name], element: Ty, span: Span) {
        if names.len() == 1 {
            self.bind(&names[0].text, element, names[0].span);
            return;
        }
        let parts = match element {
            Ty::Tuple(ts) if ts.len() == names.len() => ts,
            Ty::Unknown => vec![Ty::Unknown; names.len()],
            other => {
                let shown = self.p.show(&other);
                self.err(
                    "E0061",
                    span,
                    catalog::cannot_destructure(&shown, names.len()),
                );
                vec![Ty::Unknown; names.len()]
            }
        };
        for (n, t) in names.iter().zip(parts) {
            self.bind(&n.text, t, n.span);
        }
    }

    /// The type of the items a `for` visits.
    fn element_of(&mut self, ty: &Ty, span: Span) -> Ty {
        match ty {
            Ty::List(t) | Ty::Set(t) => (**t).clone(),
            Ty::Map(k, v) => Ty::Tuple(vec![(**k).clone(), (**v).clone()]),
            Ty::Unknown | Ty::Never => Ty::Unknown,
            Ty::Named(d, _) if Some(*d) == self.known.range => Ty::int(),
            _ => {
                // Iterating calls `next`, after `iterator` for a value that is not a cursor.
                if let Some(t) = self.iterator_item(ty) {
                    self.implicit_call(ty, "Iterator", "next", span);
                    return t;
                }
                if let Some(cursor) = self.method_result(ty, "iterator") {
                    if let Some(t) = self.iterator_item(&cursor) {
                        if let Some((f, _, _)) = self.find_method(ty, "iterator") {
                            self.link(Node::Func(f), span);
                        }
                        self.implicit_call(&cursor, "Iterator", "next", span);
                        return t;
                    }
                }
                let shown = self.p.show(ty);
                self.err("E0061", span, catalog::not_iterable(&shown));
                Ty::Unknown
            }
        }
    }

    fn iterator_item(&mut self, ty: &Ty) -> Option<Ty> {
        let iterator = self.known.iterator?;
        let next = self.method_result(ty, "next")?;
        let _ = iterator;
        match next {
            Ty::Named(d, args) if Some(d) == self.known.option => args.first().cloned(),
            _ => None,
        }
    }

    fn method_result(&mut self, ty: &Ty, name: &str) -> Option<Ty> {
        let (f, subst, self_ty) = self.find_method(ty, name)?;
        Some(self.p.funcs[f].ret.subst(&subst, Some(&self_ty)))
    }

    // ------------------------------------------------------------ conditions and is

    /// Checks a condition and returns the names its `is` tests bind for the guarded branch.
    fn condition(&mut self, e: &ast::Expr) -> Vec<(String, Ty, Span)> {
        let (ty, bindings) = self.cond_expr(e);
        self.expect(&ty, &Ty::bool(), e.span);
        bindings
    }

    fn cond_expr(&mut self, e: &ast::Expr) -> (Ty, Vec<(String, Ty, Span)>) {
        match &e.kind {
            ExprKind::Is { value, pattern } => {
                let ty = self.expr(value, &Ty::Unknown);
                let mut bindings = Vec::new();
                self.pattern(pattern, &ty, &mut bindings);
                (Ty::bool(), bindings)
            }
            ExprKind::Paren(inner) => self.cond_expr(inner),
            ExprKind::Binary {
                op: "and",
                left,
                right,
            } => {
                let (lt, mut bindings) = self.cond_expr(left);
                self.expect(&lt, &Ty::bool(), left.span);
                self.push();
                for (n, t, span) in bindings.clone() {
                    self.bind(&n, t, span);
                }
                let (rt, more) = self.cond_expr(right);
                self.expect(&rt, &Ty::bool(), right.span);
                self.pop();
                bindings.extend(more);
                (Ty::bool(), bindings)
            }
            _ => (self.expr(e, &Ty::bool()), Vec::new()),
        }
    }

    // ------------------------------------------------------------ types

    fn resolve_type(&mut self, t: &ast::Type, value: bool) -> Ty {
        let generics = self.generics.clone();
        self.p.resolve_type(self.module, t, &generics, value)
    }

    fn resolved(&self, t: &Ty) -> Ty {
        t.resolve_vars(&self.vars, &Ty::Unknown)
    }

    fn fresh(&mut self) -> Ty {
        self.vars.push(None);
        Ty::Var(self.vars.len() - 1)
    }

    /// Binds the variables of `target` so that it matches `actual`.
    fn unify(&mut self, actual: &Ty, target: &Ty) {
        match (actual, target) {
            (_, Ty::Var(v)) => match self.vars[*v].clone() {
                Some(bound) => self.unify(actual, &bound),
                None => {
                    if !matches!(actual, Ty::Unknown | Ty::Never | Ty::Var(_)) {
                        self.vars[*v] = Some(actual.clone());
                    }
                }
            },
            (Ty::List(a), Ty::List(b)) | (Ty::Set(a), Ty::Set(b)) => self.unify(a, b),
            (Ty::Map(a1, b1), Ty::Map(a2, b2)) => {
                self.unify(a1, a2);
                self.unify(b1, b2);
            }
            (Ty::Tuple(xs), Ty::Tuple(ys)) if xs.len() == ys.len() => {
                for (x, y) in xs.iter().zip(ys) {
                    self.unify(x, y);
                }
            }
            (Ty::Named(d1, xs), Ty::Named(d2, ys)) if d1 == d2 && xs.len() == ys.len() => {
                for (x, y) in xs.iter().zip(ys) {
                    self.unify(x, y);
                }
            }
            (Ty::Named(_, _), Ty::Named(iface, ys)) if self.is_interface(*iface) => {
                // A value of a type that implements a generic interface binds its arguments.
                if let Some(args) = self.interface_args(actual, *iface) {
                    for (x, y) in args.iter().zip(ys) {
                        self.unify(x, y);
                    }
                }
            }
            (Ty::Func(f), Ty::Func(g)) if f.params.len() == g.params.len() => {
                for ((_, x), (_, y)) in f.params.iter().zip(&g.params) {
                    self.unify(x, y);
                }
                self.unify(&f.ret, &g.ret);
                for (x, y) in f.fails.iter().zip(&g.fails) {
                    self.unify(x, y);
                }
            }
            _ => {}
        }
    }

    fn is_interface(&self, d: DefId) -> bool {
        matches!(self.p.defs[d].kind, DefKind::Interface(_))
    }

    /// Whether a value of `actual` is accepted where `expected` is.
    fn compatible(&mut self, actual: &Ty, expected: &Ty) -> bool {
        let actual = self.resolved(actual);
        let expected = self.resolved(expected);
        self.compatible_resolved(&actual, &expected)
    }

    fn compatible_resolved(&mut self, actual: &Ty, expected: &Ty) -> bool {
        match (actual, expected) {
            (Ty::Unknown, _) | (_, Ty::Unknown) | (Ty::Never, _) => true,
            (_, Ty::Named(iface, args)) if self.is_interface(*iface) => {
                if let Ty::Named(d, a) = actual {
                    if d == iface {
                        return a.len() == args.len()
                            && a.iter()
                                .zip(args)
                                .all(|(x, y)| self.compatible_resolved(x, y));
                    }
                }
                self.implements(actual, *iface, args)
            }
            (Ty::List(a), Ty::List(b)) | (Ty::Set(a), Ty::Set(b)) => self.compatible_resolved(a, b),
            (Ty::Map(a1, b1), Ty::Map(a2, b2)) => {
                self.compatible_resolved(a1, a2) && self.compatible_resolved(b1, b2)
            }
            (Ty::Tuple(xs), Ty::Tuple(ys)) => {
                xs.len() == ys.len()
                    && xs
                        .iter()
                        .zip(ys)
                        .all(|(x, y)| self.compatible_resolved(x, y))
            }
            (Ty::Named(d1, xs), Ty::Named(d2, ys)) => {
                d1 == d2
                    && xs.len() == ys.len()
                    && xs
                        .iter()
                        .zip(ys)
                        .all(|(x, y)| self.compatible_resolved(x, y))
            }
            (Ty::Func(f), Ty::Func(g)) => {
                f.params.len() == g.params.len()
                    && f.params
                        .iter()
                        .zip(&g.params)
                        .all(|((_, x), (_, y))| self.compatible_resolved(y, x))
                    && (g.ret == Ty::Unit || self.compatible_resolved(&f.ret, &g.ret))
                    && f.fails
                        .iter()
                        .all(|x| g.fails.iter().any(|y| self.catches(y, x)))
            }
            (Ty::Var(_), _) | (_, Ty::Var(_)) => true,
            (Ty::SelfTy, _) | (_, Ty::SelfTy) => true,
            _ => actual == expected,
        }
    }

    fn expect(&mut self, actual: &Ty, expected: &Ty, span: Span) {
        if !self.compatible(actual, expected) {
            let (a, e) = (self.resolved(actual), self.resolved(expected));
            if self.is_range(&a) && matches!(e, Ty::List(_)) {
                // A Range is not a List (S-15): `.to_list()` stores its values.
                let shown = self.p.show(&e);
                self.range_as_list(span, catalog::range_as_list(&shown));
                return;
            }
            let (a, e) = (self.p.show(&a), self.p.show(&e));
            if self.silent == 0 {
                let related = self.types_related(&[expected, actual]);
                let mut diagnostic =
                    Diagnostic::error("E0061", span, catalog::type_mismatch(&e, &a))
                        .with_all_related(related);
                diagnostic.expected = Some(e);
                self.p.error(self.module, diagnostic);
            }
        }
    }

    fn is_range(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Named(d, _) if Some(*d) == self.known.range)
    }

    /// E0047 with the fix that appends `.to_list()` to the range expression at `span`.
    fn range_as_list(&mut self, span: Span, message: crate::diag::Msg) {
        if self.silent > 0 {
            return;
        }
        let mut diagnostic = Diagnostic::error("E0047", span, message);
        diagnostic.fixes.push(Fix {
            span: Span::new(span.end, span.end),
            new_text: ".to_list()".to_string(),
            description: catalog::add_to_list(),
        });
        self.p.error(self.module, diagnostic);
    }

    // ------------------------------------------------------------ interfaces

    /// The arguments with which `ty` implements the generic interface `iface`, if it does.
    fn interface_args(&mut self, ty: &Ty, iface: DefId) -> Option<Vec<Ty>> {
        for i in 0..self.p.impls.len() {
            let Some(Ty::Named(d, args)) = self.p.impls[i].interface.clone() else {
                continue;
            };
            if d != iface {
                continue;
            }
            if let Some(subst) = self.match_impl(i, ty) {
                return Some(args.iter().map(|a| a.subst(&subst, Some(ty))).collect());
            }
        }
        None
    }

    /// Whether `ty` matches the target of an impl; the bindings of its generic parameters.
    fn match_impl(&mut self, i: usize, ty: &Ty) -> Option<HashMap<ParamId, Ty>> {
        let target = self.p.impls[i].target.clone();
        if Program::type_key(&target) != Program::type_key(ty) {
            return None;
        }
        let generics = self.p.impls[i].generics.clone();
        let vars: Vec<Ty> = generics.iter().map(|_| self.fresh()).collect();
        let map: HashMap<ParamId, Ty> =
            generics.iter().copied().zip(vars.iter().cloned()).collect();
        let instantiated = target.subst(&map, None);
        self.unify(ty, &instantiated);
        let resolved: HashMap<ParamId, Ty> = generics
            .iter()
            .copied()
            .zip(vars.iter().map(|v| self.resolved(v)))
            .collect();
        // The impl applies only when its bounds hold for the bound arguments.
        for g in &generics {
            let t = resolved[g].clone();
            for bound in self.p.params[*g].bounds.clone() {
                if let Ty::Named(b, bargs) = bound {
                    let bargs: Vec<Ty> = bargs.iter().map(|x| x.subst(&resolved, None)).collect();
                    if !self.implements(&t, b, &bargs) {
                        return None;
                    }
                }
            }
        }
        Some(resolved)
    }

    /// Whether two types are the same as far as the check knows.
    fn same_type(&mut self, a: &Ty, b: &Ty) -> bool {
        self.compatible_resolved(a, b) && self.compatible_resolved(b, a)
    }

    /// Whether `ty` implements `iface` with the given arguments: `Iterator[Int]` is not
    /// `Iterator[Text]` (docs/semantics.md 2.5, 3.6).
    fn implements(&mut self, ty: &Ty, iface: DefId, args: &[Ty]) -> bool {
        let name = self.p.defs[iface].name.clone();
        match ty {
            Ty::Unknown | Ty::Never | Ty::Var(_) | Ty::SelfTy => return true,
            Ty::Param(p) => {
                let bounds = self.p.params[*p].bounds.clone();
                return bounds.iter().any(|b| match b {
                    Ty::Named(d, bargs) if *d == iface => {
                        args.is_empty()
                            || bargs.len() != args.len()
                            || bargs.iter().zip(args).all(|(x, y)| self.same_type(x, y))
                    }
                    _ => false,
                });
            }
            Ty::Named(d, _) if *d == iface => return true,
            _ => {}
        }
        let builtin = match (ty, name.as_str()) {
            (
                Ty::Prim(Prim::Int),
                "Equal" | "Order" | "Hash" | "Add" | "Subtract" | "Multiply" | "Divide",
            ) => true,
            (Ty::Prim(Prim::Float), "Equal" | "Add" | "Subtract" | "Multiply" | "Divide") => true,
            (Ty::Prim(Prim::Text), "Equal" | "Order" | "Hash" | "Add") => true,
            (Ty::Prim(Prim::Bool | Prim::Bytes | Prim::Path), "Equal" | "Hash") => true,
            (Ty::List(t) | Ty::Set(t), "Equal" | "Hash") => {
                let t = (**t).clone();
                return self.implements(&t, iface, args);
            }
            (Ty::Map(k, v), "Equal") => {
                let (k, v) = ((**k).clone(), (**v).clone());
                return self.implements(&k, iface, args) && self.implements(&v, iface, args);
            }
            (Ty::Tuple(ts), "Equal" | "Hash" | "Order") => {
                return ts.clone().iter().all(|t| self.implements(t, iface, args));
            }
            (Ty::Named(d, targs), "Equal" | "Hash") => {
                if self.is_interface(*d)
                    || self.is_resource(ty)
                    || (name == "Hash" && self.has_float_field(*d))
                {
                    false
                } else {
                    let targs = targs.clone();
                    targs.iter().all(|t| self.implements(t, iface, args))
                }
            }
            (Ty::Named(d, _), "Error" | "Display") => {
                matches!(self.p.defs[*d].kind, DefKind::Error(_))
            }
            _ => false,
        };
        if builtin {
            return true;
        }
        for i in 0..self.p.impls.len() {
            let impl_args = match &self.p.impls[i].interface {
                Some(Ty::Named(d, a)) if *d == iface => a.clone(),
                _ => continue,
            };
            if let Some(subst) = self.match_impl(i, ty) {
                if args.is_empty() || impl_args.len() != args.len() {
                    return true;
                }
                let impl_args: Vec<Ty> = impl_args
                    .iter()
                    .map(|a| a.subst(&subst, Some(ty)))
                    .collect();
                if impl_args
                    .iter()
                    .zip(args)
                    .all(|(x, y)| self.same_type(x, y))
                {
                    return true;
                }
            }
        }
        false
    }

    /// Whether a method named `name` is declared for the kind of `recv` but its bounds fail
    /// only because a type argument is made of IEEE Float, as `sorted` on `List[Float]`
    /// (L-32, E0045).
    fn float_blocks_method(&self, recv: &Ty, name: &str) -> bool {
        let arguments_float = match recv {
            Ty::List(t) | Ty::Set(t) => t.float_inside(),
            Ty::Map(k, v) => k.float_inside() || v.float_inside(),
            Ty::Named(_, args) | Ty::Tuple(args) => args.iter().any(Ty::float_inside),
            _ => false,
        };
        let Some(key) = Program::type_key(recv) else {
            return false;
        };
        arguments_float
            && self.p.impls.iter().any(|i| {
                Program::type_key(&i.target) == Some(key.clone())
                    && i.methods.iter().any(|m| self.p.funcs[*m].name == name)
            })
    }

    fn has_float_field(&self, d: DefId) -> bool {
        let fields: Vec<&Ty> = match &self.p.defs[d].kind {
            DefKind::Struct(f) | DefKind::Error(f) => f.iter().map(|f| &f.ty).collect(),
            DefKind::Enum(vs) => vs
                .iter()
                .flat_map(|v| v.fields.iter().map(|(_, t)| t))
                .collect(),
            DefKind::Interface(_) => Vec::new(),
        };
        fields.iter().any(|t| matches!(t, Ty::Prim(Prim::Float)))
    }

    fn is_resource(&mut self, ty: &Ty) -> bool {
        let Some(resource) = self.known.resource else {
            return false;
        };
        if matches!(ty, Ty::Unknown | Ty::Param(_)) {
            return false;
        }
        for i in 0..self.p.impls.len() {
            match &self.p.impls[i].interface {
                Some(Ty::Named(d, _)) if *d == resource => {}
                _ => continue,
            }
            if self.match_impl(i, ty).is_some() {
                return true;
            }
        }
        false
    }

    /// Finds a method of a type: its function, the bindings of the impl's parameters, and
    /// the type that `Self` stands for.
    fn find_method(&mut self, recv: &Ty, name: &str) -> Option<(FnId, HashMap<ParamId, Ty>, Ty)> {
        let recv = self.resolved(recv);
        match &recv {
            Ty::Param(p) => {
                for bound in self.p.params[*p].bounds.clone() {
                    if let Ty::Named(d, args) = &bound {
                        if let Some(hit) = self.interface_method(*d, args, name, &recv) {
                            return Some(hit);
                        }
                    }
                }
                return None;
            }
            Ty::SelfTy => {
                let d = self.interface?;
                let args: Vec<Ty> = self.p.defs[d]
                    .generics
                    .iter()
                    .map(|g| Ty::Param(*g))
                    .collect();
                return self.interface_method(d, &args, name, &recv);
            }
            Ty::Named(d, args) if self.is_interface(*d) => {
                let (d, args) = (*d, args.clone());
                return self.interface_method(d, &args, name, &recv);
            }
            _ => {}
        }
        Program::type_key(&recv)?;
        // Methods of impls first, then defaults of the interfaces the type implements.
        for i in 0..self.p.impls.len() {
            let methods = self.p.impls[i].methods.clone();
            let Some(f) = methods
                .iter()
                .copied()
                .find(|m| self.p.funcs[*m].name == name)
            else {
                continue;
            };
            if let Some(subst) = self.match_impl(i, &recv) {
                return Some((f, subst, recv.clone()));
            }
        }
        for i in 0..self.p.impls.len() {
            let Some(Ty::Named(d, args)) = self.p.impls[i].interface.clone() else {
                continue;
            };
            if self.match_impl(i, &recv).is_none() {
                continue;
            }
            if let Some(hit) = self.interface_method(d, &args, name, &recv) {
                return Some(hit);
            }
        }
        // Every error declaration implements Error, whose methods the compiler generates.
        if let Ty::Named(d, _) = &recv {
            if matches!(self.p.defs[*d].kind, DefKind::Error(_)) {
                if let Some(error) = prelude_def(self.p, "Error") {
                    return self.interface_method(error, &[], name, &recv);
                }
            }
        }
        None
    }

    fn interface_method(
        &self,
        d: DefId,
        args: &[Ty],
        name: &str,
        recv: &Ty,
    ) -> Option<(FnId, HashMap<ParamId, Ty>, Ty)> {
        let DefKind::Interface(methods) = &self.p.defs[d].kind else {
            return None;
        };
        let f = methods
            .iter()
            .copied()
            .find(|m| self.p.funcs[*m].name == name)?;
        let subst: HashMap<ParamId, Ty> = self.p.defs[d]
            .generics
            .iter()
            .copied()
            .zip(args.iter().cloned())
            .collect();
        Some((f, subst, recv.clone()))
    }

    // ------------------------------------------------------------ calls

    fn fn_params(
        &mut self,
        f: FnId,
        subst: &HashMap<ParamId, Ty>,
        self_ty: Option<&Ty>,
    ) -> Vec<ParamSpec> {
        let declared: Vec<(String, Ty, bool, bool, bool)> = self.p.funcs[f]
            .params
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    p.ty.clone(),
                    p.named_only,
                    p.has_default,
                    p.var,
                )
            })
            .collect();
        declared
            .into_iter()
            .map(|(name, ty, named_only, has_default, var)| {
                // A resource may be passed only to a parameter declared with a resource type
                // or a type parameter bounded by `Resource`: the callee can then only use it
                // and pass it on (semantics 7.2). A parameter of an unbounded type parameter,
                // such as `item as T` of `List.append`, may store what it gets, even when `T`
                // stands for a resource at this call (E0081, L-20).
                let bounded = matches!(&ty, Ty::Param(id) if self.p.params[*id]
                    .bounds
                    .iter()
                    .any(|b| matches!(b, Ty::Named(d, _) if Some(*d) == self.known.resource)));
                let own = ty.subst(&HashMap::new(), self_ty);
                let resource_bound = bounded || self.is_resource(&own);
                ParamSpec {
                    name: Some(name),
                    ty: ty.subst(subst, self_ty),
                    named_only,
                    has_default,
                    var,
                    resource_bound,
                }
            })
            .collect()
    }

    fn value_params(&mut self, f: &FnTy) -> Vec<ParamSpec> {
        f.params
            .iter()
            .map(|(label, ty)| {
                // A function value of a resource parameter only uses what it gets (7.2).
                let resolved = self.resolved(ty);
                ParamSpec {
                    name: label.clone(),
                    ty: ty.clone(),
                    named_only: label.is_some(),
                    has_default: false,
                    var: false,
                    resource_bound: self.is_resource(&resolved),
                }
            })
            .collect()
    }

    /// Checks the arguments of a call against parameters whose types may contain variables;
    /// returns the result type with the variables resolved.
    fn call(
        &mut self,
        params: Vec<ParamSpec>,
        ret: Ty,
        args: &[ast::Argument],
        span: Span,
        expected: &Ty,
    ) -> Ty {
        // The declaration of the function called, for E0040 and E0064 (C-8).
        let declared: Vec<Related> = self
            .callee_fn
            .take()
            .and_then(|f| {
                let name = self.p.funcs[f].name.clone();
                self.node_related(Node::Func(f), catalog::parameters_here(&name))
            })
            .into_iter()
            .collect();
        let mut assigned: Vec<Option<&ast::Argument>> = vec![None; params.len()];
        let plain: Vec<usize> = (0..params.len())
            .filter(|i| !params[*i].named_only)
            .collect();
        let mut next_plain = 0;
        let mut reported = false;
        for arg in args {
            match &arg.name {
                None => {
                    if next_plain < plain.len() {
                        assigned[plain[next_plain]] = Some(arg);
                        next_plain += 1;
                    } else if let Some(j) =
                        (0..params.len()).find(|j| params[*j].named_only && assigned[*j].is_none())
                    {
                        let label = params[j].name.clone().unwrap_or_default();
                        // The fix inserts the label only when the call leaves one named-only
                        // parameter without its name and has one positional argument too many:
                        // otherwise the argument could be for another one (RFC 0001).
                        let given: Vec<&str> = args
                            .iter()
                            .filter_map(|a| a.name.as_ref().map(|n| n.text.as_str()))
                            .collect();
                        let open = params
                            .iter()
                            .filter(|q| {
                                q.named_only
                                    && !q.name.as_deref().is_some_and(|n| given.contains(&n))
                            })
                            .count();
                        let positional = args.iter().filter(|a| a.name.is_none()).count();
                        let open = if positional == plain.len() + 1 {
                            open
                        } else {
                            0
                        };
                        let at = arg.value.span;
                        let mut diagnostic =
                            Diagnostic::error("E0040", at, catalog::missing_label(&label))
                                .with_all_related(declared.clone());
                        if open == 1 && !label.is_empty() {
                            diagnostic.fixes.push(Fix {
                                span: Span::new(at.start, at.start),
                                new_text: format!("{label} = "),
                                description: catalog::insert_label(&label),
                            });
                        }
                        if self.silent == 0 {
                            self.p.error(self.module, diagnostic);
                        }
                        reported = true;
                        assigned[j] = Some(arg);
                    } else {
                        if !reported {
                            self.err_with(
                                "E0064",
                                arg.value.span,
                                catalog::too_many_arguments(params.len()),
                                declared.clone(),
                            );
                            reported = true;
                        }
                        self.expr(&arg.value, &Ty::Unknown);
                    }
                }
                Some(name) => match params
                    .iter()
                    .position(|p| p.name.as_deref() == Some(name.text.as_str()))
                {
                    Some(i) if assigned[i].is_none() => assigned[i] = Some(arg),
                    Some(_) => {
                        let message = catalog::argument_twice(&name.text);
                        self.err_with("E0064", name.span, message, declared.clone());
                        reported = true;
                        self.expr(&arg.value, &Ty::Unknown);
                    }
                    None => {
                        let message = catalog::unknown_argument(&name.text);
                        self.err_with("E0064", name.span, message, declared.clone());
                        reported = true;
                        self.expr(&arg.value, &Ty::Unknown);
                    }
                },
            }
        }
        if !reported {
            for (i, p) in params.iter().enumerate() {
                if assigned[i].is_none() && !p.has_default {
                    let name = p.name.clone().unwrap_or_else(|| format!("{}", i + 1));
                    let message = catalog::missing_argument(&name);
                    let ty = p.ty.clone();
                    self.err_expecting("E0064", span, message, &ty, declared.clone());
                    break;
                }
            }
        }
        self.unify(expected, &ret);
        // Arguments that are not lambdas bind the variables first; lambdas then see them.
        let order: Vec<usize> = (0..params.len())
            .filter(|i| {
                !matches!(
                    assigned[*i].map(|a| &a.value.kind),
                    Some(ExprKind::Lambda { .. })
                )
            })
            .chain((0..params.len()).filter(|i| {
                matches!(
                    assigned[*i].map(|a| &a.value.kind),
                    Some(ExprKind::Lambda { .. })
                )
            }))
            .collect();
        let mut var_roots: Vec<(String, Option<Span>)> = self
            .var_receiver
            .take()
            .into_iter()
            .map(|name| (name, None))
            .collect();
        for (i, p) in params.iter().enumerate() {
            let Some(arg) = assigned[i] else { continue };
            if p.var && self.silent == 0 {
                // A var parameter changes the caller's variable (L-11, RFC 0001).
                let (root, bad) = self.place(&arg.value);
                if !Checker::mutable_root(&root) || bad.is_some() {
                    let name = Checker::root_name(&root);
                    let related = match &root {
                        Root::Local(n, _) => self.local_related(n),
                        _ => None,
                    };
                    self.err_with(
                        "E0079",
                        arg.value.span,
                        catalog::var_argument(&name),
                        related,
                    );
                } else {
                    let name = Checker::root_name(&root);
                    if let Some((_, first)) = var_roots.iter().find(|(n, _)| *n == name) {
                        let related = first.map(|at| Related::here(at, catalog::first_one_here()));
                        let message = catalog::var_argument_twice(&name);
                        self.err_with("E0042", arg.value.span, message, related);
                    }
                    var_roots.push((name, Some(arg.value.span)));
                }
            }
            if p.resource_bound {
                self.use_ok.insert(span_key(arg.value.span));
            }
        }
        let mut flows = Vec::new();
        for i in order {
            let Some(arg) = assigned[i] else { continue };
            let expected_arg = self.resolved_keep(&params[i].ty);
            let found = self.expr(&arg.value, &expected_arg);
            self.unify(&found, &params[i].ty);
            self.expect(&found, &params[i].ty, arg.value.span);
            // The values an argument may be, for the latent effects of the callee (RFC 0005).
            let flow = self.value_of(&arg.value, &found);
            if !flow.is_empty() {
                if params[i].var {
                    // A var parameter may give the caller's name any value of its type.
                    if let ExprKind::Name(n) = &strip(&arg.value).kind {
                        if let Some(Some(id)) = self.local_flow(n) {
                            let ty = self.resolved(&found);
                            self.p.effect_facts.locals[id].push(Atom::Top(ty));
                        }
                    }
                }
                flows.push((i, flow));
            }
        }
        self.last_args = flows;
        self.resolved(&ret)
    }

    /// Resolves the bound variables and keeps the unbound ones.
    fn resolved_keep(&self, t: &Ty) -> Ty {
        fn go(t: &Ty, vars: &[Option<Ty>]) -> Ty {
            match t {
                Ty::Var(v) => match vars.get(*v).cloned().flatten() {
                    Some(b) => go(&b, vars),
                    None => Ty::Var(*v),
                },
                Ty::List(x) => Ty::List(Box::new(go(x, vars))),
                Ty::Set(x) => Ty::Set(Box::new(go(x, vars))),
                Ty::Map(k, v) => Ty::Map(Box::new(go(k, vars)), Box::new(go(v, vars))),
                Ty::Tuple(ts) => Ty::Tuple(ts.iter().map(|x| go(x, vars)).collect()),
                Ty::Named(d, ts) => Ty::Named(*d, ts.iter().map(|x| go(x, vars)).collect()),
                Ty::Func(f) => Ty::Func(Box::new(FnTy {
                    params: f
                        .params
                        .iter()
                        .map(|(l, x)| (l.clone(), go(x, vars)))
                        .collect(),
                    ret: go(&f.ret, vars),
                    fails: f.fails.iter().map(|x| go(x, vars)).collect(),
                })),
                other => other.clone(),
            }
        }
        go(t, &self.vars)
    }

    /// Calls a declared function or method: instantiates its generic parameters.
    #[allow(clippy::too_many_arguments)]
    fn call_function(
        &mut self,
        f: FnId,
        base: HashMap<ParamId, Ty>,
        self_ty: Option<Ty>,
        explicit: Option<&[ast::Type]>,
        args: &[ast::Argument],
        span: Span,
        expected: &Ty,
    ) -> Ty {
        let generics = self.p.funcs[f].generics.clone();
        let mut subst = base;
        let mut vars = Vec::new();
        for g in &generics {
            let v = self.fresh();
            vars.push(v.clone());
            subst.insert(*g, v);
        }
        if let Some(types) = explicit {
            if types.len() != generics.len() {
                let name = self.p.funcs[f].name.clone();
                let related = self.node_related(Node::Func(f), catalog::declared_here(&name));
                self.err_with(
                    "E0059",
                    span,
                    catalog::type_arity(&name, generics.len(), types.len()),
                    related,
                );
            }
            for (v, t) in vars.iter().zip(types) {
                let t = self.resolve_type(t, true);
                self.unify(&t, v);
            }
        }
        let params = self.fn_params(f, &subst, self_ty.as_ref());
        let ret = self.p.funcs[f].ret.subst(&subst, self_ty.as_ref());
        let fails: Vec<Ty> = self.p.funcs[f]
            .fails
            .iter()
            .map(|t| t.subst(&subst, self_ty.as_ref()))
            .collect();
        self.callee_fn = Some(f);
        let result = self.call(params, ret, args, span, expected);
        let flows = std::mem::take(&mut self.last_args);
        self.record_call(f, span, flows);
        self.fallible_call(fails, span, Some(f));
        if self.silent == 0 && !subst.is_empty() {
            let types = subst.iter().map(|(g, t)| (*g, self.resolved(t))).collect();
            self.p.generic_calls.push(crate::program::GenericCall {
                caller: self.current_fn,
                types,
                module: self.module,
                span,
            });
        }
        // Constraints of the generic parameters (E0062).
        for (g, v) in generics.iter().zip(&vars) {
            let t = self.resolved(v);
            if t.is_partial() {
                continue;
            }
            for bound in self.p.params[*g].bounds.clone() {
                if let Ty::Named(b, bargs) = bound {
                    if !self.implements(&t, b, &bargs) {
                        let (ts, bs) = (self.p.show(&t), self.p.defs[b].name.clone());
                        if matches!(bs.as_str(), "Order" | "Hash") && t.float_inside() {
                            // IEEE Float where a total order or a hash is needed (L-32).
                            self.err("E0045", span, catalog::float_constraint(&ts, &bs));
                        } else {
                            let related = self.types_related(&[&t]);
                            let message = catalog::constraint_not_met(&ts, &bs);
                            self.err_with("E0062", span, message, related);
                        }
                    }
                }
            }
        }
        result
    }

    /// Builds a struct or error value from named fields.
    fn construct(
        &mut self,
        def: DefId,
        explicit: Option<&[ast::Type]>,
        args: &[ast::Argument],
        span: Span,
        expected: &Ty,
    ) -> Ty {
        let generics = self.p.defs[def].generics.clone();
        let vars: Vec<Ty> = generics.iter().map(|_| self.fresh()).collect();
        let subst: HashMap<ParamId, Ty> =
            generics.iter().copied().zip(vars.iter().cloned()).collect();
        if let Some(types) = explicit {
            for (v, t) in vars.iter().zip(types) {
                let t = self.resolve_type(t, true);
                self.unify(&t, v);
            }
        }
        let result = Ty::Named(def, vars);
        self.unify(expected, &result);
        let fields: Vec<(String, Ty, bool, bool)> = match &self.p.defs[def].kind {
            DefKind::Struct(f) | DefKind::Error(f) => f
                .iter()
                .map(|f| {
                    (
                        f.name.clone(),
                        f.ty.subst(&subst, None),
                        f.public,
                        f.has_default,
                    )
                })
                .collect(),
            _ => Vec::new(),
        };
        // Building a value may evaluate the defaults of its fields.
        if fields.iter().any(|f| f.3) {
            self.link(Node::Def(def), span);
        }
        // The fields of a resource may hold resources created right here (L-20).
        let unknown_args: Vec<Ty> = self.p.defs[def]
            .generics
            .iter()
            .map(|_| Ty::Unknown)
            .collect();
        if self.is_resource(&Ty::Named(def, unknown_args)) {
            for arg in args {
                self.allow_creation(&arg.value);
            }
        }
        let foreign = self.p.defs[def].module != self.module;
        let type_name = self.p.defs[def].name.clone();
        let declared = self.node_related(Node::Def(def), catalog::type_declared_here(&type_name));
        if foreign && fields.iter().any(|f| !f.2) {
            let message = catalog::private_fields(&type_name);
            self.err_with("E0057", span, message, declared.clone());
        }
        let mut given = HashSet::new();
        for arg in args {
            let Some(name) = &arg.name else {
                self.err_with(
                    "E0063",
                    arg.value.span,
                    catalog::fields_are_named(&type_name),
                    declared.clone(),
                );
                self.expr(&arg.value, &Ty::Unknown);
                continue;
            };
            match fields.iter().find(|f| f.0 == name.text) {
                Some((_, ty, _, _)) => {
                    given.insert(name.text.clone());
                    let ty = ty.clone();
                    let expected_field = self.resolved_keep(&ty);
                    let found = self.expr(&arg.value, &expected_field);
                    self.unify(&found, &ty);
                    self.expect(&found, &ty, arg.value.span);
                    let flow = self.value_of(&arg.value, &found);
                    self.store(def, &name.text, flow);
                }
                None => {
                    self.err_with(
                        "E0063",
                        name.span,
                        catalog::unknown_field(&name.text, &type_name),
                        declared.clone(),
                    );
                    self.expr(&arg.value, &Ty::Unknown);
                }
            }
        }
        for (name, ty, _, has_default) in &fields {
            if !has_default && !given.contains(name) {
                let message = catalog::missing_field(name, &type_name);
                self.err_expecting("E0063", span, message, ty, declared.clone());
                break;
            }
        }
        self.resolved(&result)
    }

    /// Builds an enum value: `Add(title = x)`, `some(v)`, `Value.Text("a")`.
    fn construct_variant(
        &mut self,
        def: DefId,
        variant: usize,
        args: &[ast::Argument],
        span: Span,
        expected: &Ty,
    ) -> Ty {
        let generics = self.p.defs[def].generics.clone();
        let vars: Vec<Ty> = generics.iter().map(|_| self.fresh()).collect();
        let subst: HashMap<ParamId, Ty> =
            generics.iter().copied().zip(vars.iter().cloned()).collect();
        let result = Ty::Named(def, vars);
        self.unify(expected, &result);
        let fields: Vec<(String, Ty)> = match &self.p.defs[def].kind {
            DefKind::Enum(vs) => vs[variant]
                .fields
                .iter()
                .map(|(n, t)| (n.clone(), t.subst(&subst, None)))
                .collect(),
            _ => Vec::new(),
        };
        let params = fields
            .into_iter()
            .map(|(n, t)| ParamSpec {
                name: Some(n),
                ty: t,
                named_only: false,
                has_default: false,
                var: false,
                resource_bound: false,
            })
            .collect();
        self.call(params, result, args, span, &Ty::Unknown)
    }

    // ------------------------------------------------------------ names

    /// Variants that a short name may mean in this module.
    fn short_variants(&self, name: &str) -> Vec<(DefId, usize)> {
        let mut found = Vec::new();
        let consider = |d: DefId, found: &mut Vec<(DefId, usize)>| {
            if let DefKind::Enum(vs) = &self.p.defs[d].kind {
                if let Some(i) = vs.iter().position(|v| v.name == name) {
                    if !found.contains(&(d, i)) {
                        found.push((d, i));
                    }
                }
            }
        };
        for symbol in self.p.modules[self.module].symbols.values() {
            if let Symbol::Def(d) = symbol {
                consider(*d, &mut found);
            }
        }
        for binding in self.p.modules[self.module].imports.values() {
            if let crate::program::Binding::Symbol(_, Symbol::Def(d)) = binding {
                consider(*d, &mut found);
            }
        }
        if found.is_empty() {
            for (_, symbol) in self.p.prelude.values() {
                if let Symbol::Def(d) = symbol {
                    consider(*d, &mut found);
                }
            }
        }
        found
    }

    /// The variant of a short name, reporting E0072 when the short form is ambiguous.
    fn variant_named(&mut self, name: &str, span: Span) -> Option<(DefId, usize)> {
        let found = self.short_variants(name);
        let first = *found.first()?;
        if !PRELUDE_VARIANTS.contains(&name) {
            // A declaration of the module hides a prelude name (docs/semantics.md 1.4), so
            // only the module's own names, its imports and the built-in types collide.
            let own = self.p.modules[self.module].symbols.contains_key(name)
                || self.p.modules[self.module].imports.contains_key(name);
            let collides = found.len() > 1 || BUILTIN_TYPE_NAMES.contains(&name) || own;
            if collides {
                let enum_name = self.p.defs[first.0].name.clone();
                let mut diagnostic =
                    Diagnostic::error("E0072", span, catalog::variant_collision(name, &enum_name));
                // The enums with the variant, and a declaration of the module with its name.
                for (d, _) in &found {
                    let message = catalog::type_declared_here(&self.p.defs[*d].name);
                    diagnostic
                        .related
                        .extend(self.node_related(Node::Def(*d), message));
                }
                if let Some(symbol) = self.p.modules[self.module].symbols.get(name).copied() {
                    let node = crate::program::symbol_node(symbol);
                    let message = catalog::declared_here(name);
                    diagnostic.related.extend(self.node_related(node, message));
                }
                // The full form is the fix when it is unambiguous: one enum has the variant and
                // its name means that enum here (R-9, the migration of concept 0.4).
                let visible = matches!(
                    self.p.lookup(self.module, &enum_name),
                    Some((_, Symbol::Def(d))) if d == first.0
                );
                if found.len() == 1 && visible {
                    diagnostic.fixes.push(Fix {
                        span,
                        new_text: format!("{enum_name}.{name}"),
                        description: catalog::write_variant_in_full(name, &enum_name),
                    });
                }
                if self.silent == 0 {
                    self.p.error(self.module, diagnostic);
                }
            }
        }
        Some(first)
    }

    fn unknown_name(&mut self, name: &str, span: Span) {
        if self.p.modules[self.module].failed_imports.contains(name) {
            // The import that should bind the name has been reported (semantics 2.8).
            return;
        }
        if let Some(at) = self.expired.get(name).copied() {
            let related =
                (at != Span::default()).then(|| Related::here(at, catalog::bound_here(name)));
            self.err_with("E0050", span, catalog::is_name_not_bound(name), related);
            return;
        }
        let mut candidates: Vec<String> = Vec::new();
        for scope in &self.scopes {
            candidates.extend(scope.iter().map(|l| l.name.clone()));
        }
        candidates.extend(self.p.modules[self.module].symbols.keys().cloned());
        candidates.extend(self.p.modules[self.module].imports.keys().cloned());
        candidates.extend(self.p.prelude.keys().cloned());
        let mut diagnostic = Diagnostic::error("E0056", span, catalog::unknown_name(name));
        if let Some(best) = closest(name, candidates.iter().map(String::as_str)) {
            diagnostic.fixes.push(Fix {
                span,
                new_text: best.to_string(),
                description: catalog::did_you_mean(best),
            });
        }
        self.p.error(self.module, diagnostic);
    }

    fn symbol_type(&mut self, owner: ModId, symbol: Symbol, span: Span) -> Ty {
        match symbol {
            Symbol::Func(f) => {
                // Naming a function makes a value; its body runs where the value is called.
                self.record_reference(f, span);
                if self.silent == 0 {
                    self.p.effect_facts.named.push(f);
                }
                let def = &self.p.funcs[f];
                let generics = def.generics.clone();
                let subst: HashMap<ParamId, Ty> =
                    generics.iter().map(|g| (*g, Ty::Unknown)).collect();
                let _ = owner;
                Ty::Func(Box::new(FnTy {
                    params: def
                        .params
                        .iter()
                        .map(|p| {
                            (
                                p.named_only.then(|| p.name.clone()),
                                p.ty.subst(&subst, None),
                            )
                        })
                        .collect(),
                    ret: def.ret.subst(&subst, None),
                    fails: def.fails.iter().map(|t| t.subst(&subst, None)).collect(),
                }))
            }
            Symbol::Const(c) => {
                self.link(Node::Const(c), span);
                self.const_types
                    .get(&c)
                    .cloned()
                    .or_else(|| self.p.consts[c].annotated.clone())
                    .unwrap_or(Ty::Unknown)
            }
            Symbol::Def(d) => {
                let name = self.p.defs[d].name.clone();
                self.err("E0061", span, catalog::type_is_not_a_value(&name));
                Ty::Unknown
            }
        }
    }

    fn check_visible(&mut self, owner: ModId, public: bool, name: &str, span: Span, node: Node) {
        if owner != self.module && !public {
            let related = self.node_related(node, catalog::declared_here(name));
            self.err_with("E0057", span, catalog::private_used(name), related);
        }
    }

    fn name_value(&mut self, name: &str, span: Span, expected: &Ty) -> Ty {
        if let Some((scope, t, kind)) = self.find_local_full(name) {
            self.mark_read(name);
            self.note_capture(scope);
            if self.silent == 0 && !t.is_unknown() {
                let resolved = self.resolved(&t);
                let captured = scope < self.lambda_floor;
                let used_in_place = self.use_ok.contains(&span_key(span));
                let parameter = matches!(kind, LocalKind::Param | LocalKind::VarParam);
                let escapes = kind == LocalKind::Using && !used_in_place;
                if (captured || escapes || parameter && !used_in_place)
                    && self.is_resource(&resolved)
                {
                    // A resource parameter may be used and passed on, not kept (semantics 7.2).
                    let message = if parameter {
                        catalog::resource_parameter_escapes(name)
                    } else {
                        catalog::resource_escapes(name)
                    };
                    let related = self.local_related(name);
                    self.err_with("E0081", span, message, related);
                }
            }
            return t;
        }
        if !self.short_variants(name).is_empty()
            && self
                .p
                .lookup(self.module, name)
                .is_none_or(|(_, s)| matches!(s, Symbol::Def(_)))
        {
            let (d, v) = self.variant_named(name, span).unwrap();
            return self.construct_variant(d, v, &[], span, expected);
        }
        if let Some((owner, symbol)) = self.p.lookup(self.module, name) {
            let public = self.p.symbol_public(symbol);
            let node = crate::program::symbol_node(symbol);
            self.check_visible(owner, public, name, span, node);
            return self.symbol_type(owner, symbol, span);
        }
        if self.p.module_alias(self.module, name).is_some() {
            self.err("E0061", span, catalog::module_is_not_a_value(name));
            return Ty::Unknown;
        }
        self.unknown_name(name, span);
        Ty::Unknown
    }

    /// Interprets `T`, `alias.T`, `Enum`, or `alias.Enum` written as an expression.
    fn type_ref(&mut self, e: &ast::Expr) -> Option<DefId> {
        match &e.kind {
            ExprKind::Name(n) => {
                if self.find_local(n).is_some() {
                    return None;
                }
                match self.p.lookup(self.module, n) {
                    Some((_, Symbol::Def(d))) => Some(d),
                    _ => None,
                }
            }
            ExprKind::Field { target, name } => {
                let ExprKind::Name(alias) = &target.kind else {
                    return None;
                };
                if self.find_local(alias).is_some() {
                    return None;
                }
                let m = self.p.module_alias(self.module, alias)?;
                match self.p.modules[m].symbols.get(&name.text) {
                    Some(Symbol::Def(d)) => Some(*d),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn module_ref(&self, e: &ast::Expr) -> Option<ModId> {
        let ExprKind::Name(n) = &e.kind else {
            return None;
        };
        if self.find_local(n).is_some() {
            return None;
        }
        self.p.module_alias(self.module, n)
    }

    // ------------------------------------------------------------ expressions

    fn expr(&mut self, e: &ast::Expr, expected: &Ty) -> Ty {
        let ty = self.expr_kind(e, expected);
        if self.noted.on {
            // The type of the expression and, for a local name, the binding it names (T-10).
            let local = match &e.kind {
                ExprKind::Name(n) => self.local_span(n),
                _ => None,
            };
            self.noted.expr(self.module, e.span, &ty, local);
            // The type a name is expected to have: the language server puts a name in the
            // place of missing code to learn the expected type there (T-13).
            if matches!(e.kind, ExprKind::Name(_)) && !matches!(expected, Ty::Unknown) {
                self.noted.expected(self.module, e.span, expected);
            }
        }
        ty
    }

    /// The place where the innermost local of this name is bound.
    fn local_span(&self, name: &str) -> Option<Span> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.iter().rev().find(|l| l.name == name))
            .map(|l| l.span)
    }

    fn expr_kind(&mut self, e: &ast::Expr, expected: &Ty) -> Ty {
        match &e.kind {
            ExprKind::Int(_) => Ty::int(),
            ExprKind::Float(_) => Ty::float(),
            ExprKind::Bool(_) => Ty::bool(),
            ExprKind::Text(parts) => {
                if parts
                    .iter()
                    .any(|p| matches!(p, ast::TextPart::Interpolation(_)))
                {
                    self.allocates(Allocation::Interpolation, e.span);
                }
                self.text(parts);
                Ty::text()
            }
            ExprKind::SelfValue => {
                self.note_capture(0);
                self.self_ty.clone().unwrap_or(Ty::Unknown)
            }
            ExprKind::Name(n) => self.name_value(n, e.span, expected),
            ExprKind::Paren(inner) => self.expr(inner, expected),
            ExprKind::Tuple(items) => {
                let expected_items = match self.resolved_keep(expected) {
                    Ty::Tuple(ts) if ts.len() == items.len() => ts,
                    _ => vec![Ty::Unknown; items.len()],
                };
                Ty::Tuple(
                    items
                        .iter()
                        .zip(&expected_items)
                        .map(|(i, t)| self.expr(i, t))
                        .collect(),
                )
            }
            ExprKind::List(items) => {
                self.allocates(Allocation::List, e.span);
                let mut element = match self.resolved_keep(expected) {
                    Ty::List(t) => *t,
                    _ => Ty::Unknown,
                };
                let mut first: Option<Ty> = None;
                for item in items {
                    let hint = if matches!(element, Ty::Unknown | Ty::Var(_)) {
                        first.clone().unwrap_or(element.clone())
                    } else {
                        element.clone()
                    };
                    let t = self.expr(item, &hint);
                    if !matches!(hint, Ty::Unknown | Ty::Var(_)) {
                        self.expect(&t, &hint, item.span);
                    }
                    if first.is_none() && !t.is_unknown() {
                        first = Some(t);
                    }
                }
                if matches!(element, Ty::Unknown | Ty::Var(_)) {
                    element = first.unwrap_or(Ty::Unknown);
                }
                Ty::List(Box::new(self.resolved(&element)))
            }
            ExprKind::Comprehension {
                element,
                names,
                iterable,
                condition,
            } => {
                self.allocates(Allocation::Comprehension, e.span);
                let ty = self.expr(iterable, &Ty::Unknown);
                let item = self.element_of(&ty, iterable.span);
                self.push();
                self.bind_destructured(names, item, iterable.span);
                let mut expiring = Vec::new();
                if let Some(c) = condition {
                    let b = self.condition(c);
                    expiring = self.bind_all(b);
                }
                let hint = match self.resolved_keep(expected) {
                    Ty::List(t) => *t,
                    _ => Ty::Unknown,
                };
                let t = self.expr(element, &hint);
                self.pop_expiring(&expiring);
                Ty::List(Box::new(t))
            }
            ExprKind::Map(entries) => {
                self.allocates(Allocation::Map, e.span);
                let (mut k, mut v) = match self.resolved_keep(expected) {
                    Ty::Map(k, v) => (*k, *v),
                    _ => (Ty::Unknown, Ty::Unknown),
                };
                // A key type written in an annotation is checked where it is written; one
                // inferred from the entries is checked here (L-9).
                let inferred_key = matches!(k, Ty::Unknown | Ty::Var(_))
                    && !matches!(self.resolved_keep(expected), Ty::Map(ref k, _) if k.is_unknown());
                for (key, value) in entries {
                    let kt = self.expr(key, &k);
                    let vt = self.expr(value, &v);
                    if matches!(k, Ty::Unknown | Ty::Var(_)) {
                        k = kt;
                    }
                    if matches!(v, Ty::Unknown | Ty::Var(_)) {
                        v = vt;
                    }
                }
                let key = self.resolved(&k);
                if inferred_key && key.float_inside() {
                    self.err("E0045", e.span, catalog::float_key(false));
                    return Ty::Map(Box::new(Ty::Unknown), Box::new(self.resolved(&v)));
                }
                Ty::Map(Box::new(key), Box::new(self.resolved(&v)))
            }
            ExprKind::Lambda {
                params,
                returns,
                fails,
                body,
            } => self.lambda(params, returns.as_ref(), fails, body, e.span, expected),
            ExprKind::Is { .. } => {
                let (ty, bindings) = self.cond_expr(e);
                for (n, _, at) in bindings {
                    if self.find_local(&n).is_none() {
                        self.expired.insert(n, at);
                    }
                }
                ty
            }
            ExprKind::Conditional {
                then,
                condition,
                otherwise,
            } => {
                let bindings = self.condition(condition);
                self.push();
                let names = self.bind_all(bindings);
                let t = self.expr(then, expected);
                self.pop_expiring(&names);
                let hint = if matches!(t, Ty::Never | Ty::Unknown) {
                    expected.clone()
                } else {
                    t.clone()
                };
                let o = self.expr(otherwise, &hint);
                if matches!(t, Ty::Never | Ty::Unknown) {
                    o
                } else {
                    self.expect(&o, &t, otherwise.span);
                    t
                }
            }
            ExprKind::Binary { op, left, right } => self.binary(op, left, right, e.span, expected),
            ExprKind::Unary { op, operand } => {
                let t = self.expr(operand, expected);
                match *op {
                    "not" => {
                        self.expect(&t, &Ty::bool(), operand.span);
                        Ty::bool()
                    }
                    _ => {
                        if !matches!(
                            self.resolved(&t),
                            Ty::Unknown
                                | Ty::Never
                                | Ty::Prim(Prim::Int | Prim::Float)
                                | Ty::Param(_)
                        ) {
                            let shown = self.p.show(&t);
                            self.err("E0065", e.span, catalog::operator_not_defined("-", &shown));
                        }
                        t
                    }
                }
            }
            ExprKind::Try { operand, handler } => {
                self.try_frames.push(Frame::default());
                let t = self.expr(operand, expected);
                let resolved = self.resolved(&t);
                let frame = self.try_frames.pop().unwrap_or_default();
                let mut errors = frame.errors;
                let value = match &resolved {
                    Ty::Named(d, args) if Some(*d) == self.known.result => {
                        if let Some(err) = args.get(1) {
                            errors.push(err.clone());
                        }
                        args.first().cloned().unwrap_or(Ty::Unknown)
                    }
                    other => other.clone(),
                };
                if errors.is_empty() && !frame.uncertain && !resolved.is_partial() {
                    self.err("E0076", e.span, catalog::try_cannot_fail());
                }
                if let Some((ty, _)) = handler {
                    let handled = self.resolve_type(ty, false);
                    errors.retain(|x| !self.catches(&handled, x));
                }
                self.raise(errors, e.span);
                value
            }
            ExprKind::Call { callee, arguments } => {
                let t = self.call_expr(callee, None, arguments, e.span, expected);
                self.check_creation(&t, e.span);
                t
            }
            ExprKind::GenericCall {
                callee,
                types,
                arguments,
            } => {
                let t = self.call_expr(callee, Some(types), arguments, e.span, expected);
                self.check_creation(&t, e.span);
                t
            }
            ExprKind::Index { target, index } => {
                let t = self.expr(target, &Ty::Unknown);
                match self.resolved(&t) {
                    Ty::List(item) => {
                        let i = self.expr(index, &Ty::int());
                        self.expect(&i, &Ty::int(), index.span);
                        *item
                    }
                    Ty::Map(k, v) => {
                        let i = self.expr(index, &k);
                        self.expect(&i, &k, index.span);
                        *v
                    }
                    Ty::Unknown | Ty::Never => {
                        self.expr(index, &Ty::Unknown);
                        Ty::Unknown
                    }
                    other if self.is_range(&other) => {
                        self.expr(index, &Ty::int());
                        self.range_as_list(target.span, catalog::range_method("[...]"));
                        Ty::Unknown
                    }
                    other => {
                        self.expr(index, &Ty::Unknown);
                        let shown = self.p.show(&other);
                        self.err("E0065", e.span, catalog::not_indexable(&shown));
                        Ty::Unknown
                    }
                }
            }
            ExprKind::Field { target, name } => self.field(target, name, e.span, expected),
            ExprKind::SafeField { target, name } => {
                let t = self.expr(target, &Ty::Unknown);
                match self.resolved(&t) {
                    Ty::Named(d, args) if Some(d) == self.known.option => {
                        let inner = args.first().cloned().unwrap_or(Ty::Unknown);
                        let f = self.field_of(&inner, name);
                        match f {
                            Ty::Named(d2, _) if Some(d2) == self.known.option => f,
                            other => Ty::Named(d, vec![other]),
                        }
                    }
                    Ty::Unknown => Ty::Unknown,
                    other => {
                        let shown = self.p.show(&other);
                        self.err(
                            "E0061",
                            target.span,
                            catalog::type_mismatch("Option[...]", &shown),
                        );
                        Ty::Unknown
                    }
                }
            }
            ExprKind::Error => Ty::Unknown,
        }
    }

    fn text(&mut self, parts: &[ast::TextPart]) {
        for part in parts {
            if let ast::TextPart::Interpolation(e) = part {
                let t = self.expr(e, &Ty::Unknown);
                let t = self.resolved(&t);
                self.implicit_call(&t, "Display", "to_text", e.span);
                if let Some(display) = self.known.display {
                    if !self.implements(&t, display, &[]) {
                        let shown = self.p.show(&t);
                        let related = self.types_related(&[&t]);
                        self.err_with("E0067", e.span, catalog::not_display(&shown), related);
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn lambda(
        &mut self,
        params: &[ast::LambdaParam],
        returns: Option<&ast::Type>,
        fails: &[ast::Type],
        body: &ast::Expr,
        span: Span,
        expected: &Ty,
    ) -> Ty {
        let expected_fn = match self.resolved_keep(expected) {
            Ty::Func(f) if f.params.len() == params.len() => Some(*f),
            _ => None,
        };
        let mut types = Vec::new();
        for (i, p) in params.iter().enumerate() {
            let t = match &p.ty {
                Some(t) => self.resolve_type(t, true),
                None => match &expected_fn {
                    Some(f) => self.resolved_keep(&f.params[i].1),
                    None => {
                        self.err(
                            "E0049",
                            p.name.span,
                            catalog::lambda_needs_types(&p.name.text),
                        );
                        Ty::Unknown
                    }
                },
            };
            types.push((p.named.then(|| p.name.text.clone()), t));
        }
        let ret = match returns {
            Some(t) => self.resolve_type(t, true),
            None => expected_fn
                .as_ref()
                .map(|f| self.resolved_keep(&f.ret))
                .unwrap_or(Ty::Unknown),
        };
        let lambda_fails: Vec<Ty> = if fails.is_empty() {
            expected_fn
                .as_ref()
                .map(|f| f.fails.iter().map(|t| self.resolved_keep(t)).collect())
                .unwrap_or_default()
        } else {
            fails.iter().map(|t| self.resolve_type(t, true)).collect()
        };
        // A lambda has its own errors, loops and captured names (docs/semantics.md 5, 7.2).
        let saved_fails = std::mem::replace(&mut self.fn_fails, lambda_fails.clone());
        let saved_frames = std::mem::take(&mut self.try_frames);
        let saved_handlers = std::mem::take(&mut self.handlers);
        let saved_handler_spans = std::mem::take(&mut self.handler_spans);
        let saved_try = std::mem::replace(&mut self.in_try_block, 0);
        let saved_loops = std::mem::replace(&mut self.loops, 0);
        let saved_floor = std::mem::replace(&mut self.lambda_floor, self.scopes.len());
        self.captures.push((self.scopes.len(), false));
        // The body is a holder of its own: its effects count where the lambda is called
        // (RFC 0005).
        let id = (self.silent == 0).then(|| {
            let parent = self.effect_holder();
            let lambda_params = params
                .iter()
                .zip(&types)
                .map(|(p, (_, t))| (p.name.text.clone(), self.resolved(t)))
                .collect();
            self.p.effect_facts.lambdas.push(LambdaFact {
                parent,
                module: self.module,
                span,
                params: lambda_params,
                ty: Ty::Unknown,
            });
            let id = self.p.effect_facts.lambdas.len() - 1;
            self.lambda_ids.insert(span_key(span), id);
            self.lambda_stack.push(id);
            id
        });
        self.push();
        for (i, (p, (_, t))) in params.iter().zip(&types).enumerate() {
            let t = if matches!(t, Ty::Var(_)) {
                Ty::Unknown
            } else {
                t.clone()
            };
            let flow = id.map(|l| self.new_flow(vec![Atom::Param(Holder::Lambda(l), i)]));
            self.bind_flow(&p.name.text, t, p.name.span, LocalKind::Let, flow);
        }
        let saved = std::mem::replace(&mut self.ret, ret.clone());
        let found = self.expr(body, &ret);
        self.ret = saved;
        self.pop();
        if id.is_some() {
            self.lambda_stack.pop();
        }
        self.fn_fails = saved_fails;
        self.try_frames = saved_frames;
        self.handlers = saved_handlers;
        self.handler_spans = saved_handler_spans;
        self.in_try_block = saved_try;
        self.loops = saved_loops;
        self.lambda_floor = saved_floor;
        if let Some((_, true)) = self.captures.pop() {
            self.allocates(Allocation::Closure, span);
        }
        if returns.is_some() {
            self.expect(&found, &ret, body.span);
        }
        let ret = if returns.is_some() {
            ret
        } else if matches!(ret, Ty::Unit) {
            Ty::Unit
        } else {
            found
        };
        let ty = Ty::Func(Box::new(FnTy {
            params: types,
            ret,
            fails: lambda_fails,
        }));
        if let Some(l) = id {
            self.p.effect_facts.lambdas[l].ty = self.resolved(&ty);
        }
        ty
    }

    fn binary(
        &mut self,
        op: &str,
        left: &ast::Expr,
        right: &ast::Expr,
        span: Span,
        expected: &Ty,
    ) -> Ty {
        match op {
            "and" => {
                let (lt, bindings) = self.cond_expr(left);
                self.expect(&lt, &Ty::bool(), left.span);
                self.push();
                let names = self.bind_all(bindings);
                let rt = self.expr(right, &Ty::bool());
                self.pop_expiring(&names);
                self.expect(&rt, &Ty::bool(), right.span);
                Ty::bool()
            }
            "or" => {
                let lt = self.expr(left, &Ty::Unknown);
                match self.resolved(&lt) {
                    Ty::Named(d, args) if Some(d) == self.known.option => {
                        let inner = args.first().cloned().unwrap_or(Ty::Unknown);
                        let hint = if expected.is_unknown() {
                            inner.clone()
                        } else {
                            expected.clone()
                        };
                        let rt = self.expr(right, &hint);
                        if self.compatible(&rt, &inner) {
                            inner
                        } else {
                            // The run gives the left Option itself when it has a value.
                            self.p.option_ors.insert((self.module, span_key(span)));
                            self.expect(&rt, &lt, right.span);
                            lt
                        }
                    }
                    Ty::Unknown => {
                        self.expr(right, &Ty::Unknown);
                        Ty::Unknown
                    }
                    _ => {
                        self.expect(&lt, &Ty::bool(), left.span);
                        let rt = self.expr(right, &Ty::bool());
                        self.expect(&rt, &Ty::bool(), right.span);
                        Ty::bool()
                    }
                }
            }
            "==" | "!=" | "<" | "<=" | ">" | ">=" => {
                // Comparing reads the operands; `==` on a resource is E0066, not E0081.
                self.use_ok.insert(span_key(left.span));
                self.use_ok.insert(span_key(right.span));
                let lt = self.expr(left, &Ty::Unknown);
                let rt = self.expr(right, &lt);
                let (l, r) = (self.resolved(&lt), self.resolved(&rt));
                if !self.compatible(&r, &l) && !self.compatible(&l, &r) {
                    let (ls, rs) = (self.p.show(&l), self.p.show(&r));
                    self.err("E0065", span, catalog::operands_differ(op, &ls, &rs));
                    return Ty::bool();
                }
                let subject = if l.is_unknown() { r.clone() } else { l.clone() };
                if matches!(op, "==" | "!=") {
                    self.implicit_call(&subject, "Equal", "equals", span);
                } else {
                    self.implicit_call(&subject, "Order", "compare", span);
                }
                if matches!(op, "==" | "!=") {
                    if self.is_resource(&subject) {
                        self.err("E0066", span, catalog::resource_equality());
                        return Ty::bool();
                    }
                    if let Some(equal) = prelude_iface(self.p, "Equal") {
                        if !matches!(subject, Ty::Prim(Prim::Float))
                            && !self.implements(&subject, equal, &[])
                        {
                            let shown = self.p.show(&subject);
                            self.err("E0065", span, catalog::operator_not_defined(op, &shown));
                        }
                    }
                } else if let Some(order) = prelude_iface(self.p, "Order") {
                    if !matches!(subject, Ty::Prim(Prim::Float))
                        && !self.implements(&subject, order, &[])
                    {
                        let shown = self.p.show(&subject);
                        if subject.float_inside() {
                            self.err("E0045", span, catalog::float_operator(op, &shown));
                        } else {
                            self.err("E0065", span, catalog::operator_not_defined(op, &shown));
                        }
                    }
                }
                Ty::bool()
            }
            "in" | "not in" => {
                let rt = self.expr(right, &Ty::Unknown);
                let r = self.resolved(&rt);
                match self.find_method(&r, "contains") {
                    Some((f, subst, self_ty)) => {
                        self.link(Node::Func(f), span);
                        let params = self.fn_params(f, &subst, Some(&self_ty));
                        let expected_item =
                            params.first().map(|p| p.ty.clone()).unwrap_or(Ty::Unknown);
                        let lt = self.expr(left, &expected_item);
                        self.expect(&lt, &expected_item, left.span);
                    }
                    None => {
                        self.expr(left, &Ty::Unknown);
                        if !matches!(r, Ty::Unknown | Ty::Never) {
                            let shown = self.p.show(&r);
                            self.err("E0061", right.span, catalog::no_membership(&shown));
                        }
                    }
                }
                Ty::bool()
            }
            _ => {
                let lt = self.expr(left, expected);
                let rt = self.expr(right, &lt);
                let (l, r) = (self.resolved(&lt), self.resolved(&rt));
                if l.is_unknown()
                    || r.is_unknown()
                    || matches!(l, Ty::Never)
                    || matches!(r, Ty::Never)
                {
                    return if l.is_unknown() { r } else { l };
                }
                if !self.compatible(&r, &l) {
                    let (ls, rs) = (self.p.show(&l), self.p.show(&r));
                    let message = if op == "+" && l == Ty::text() {
                        catalog::text_plus(&rs)
                    } else {
                        catalog::operands_differ(op, &ls, &rs)
                    };
                    self.err("E0065", span, message);
                    return l;
                }
                let interface = match op {
                    "+" => "Add",
                    "-" => "Subtract",
                    "*" => "Multiply",
                    "/" => "Divide",
                    _ => "",
                };
                if op == "+" && l == Ty::text() {
                    self.allocates(Allocation::Concatenation, span);
                } else if !interface.is_empty() {
                    let method = interface.to_lowercase();
                    self.implicit_call(&l, interface, &method, span);
                }
                let defined = if interface.is_empty() {
                    matches!(l, Ty::Prim(Prim::Int | Prim::Float) | Ty::Param(_))
                } else {
                    match prelude_iface(self.p, interface) {
                        Some(i) => self.implements(&l, i, &[]),
                        None => true,
                    }
                };
                if !defined {
                    let shown = self.p.show(&l);
                    self.err("E0065", span, catalog::operator_not_defined(op, &shown));
                }
                l
            }
        }
    }

    fn field_of(&mut self, ty: &Ty, name: &ast::Name) -> Ty {
        let ty = self.resolved(ty);
        match &ty {
            Ty::Named(d, args) => {
                let generics = self.p.defs[*d].generics.clone();
                let subst: HashMap<ParamId, Ty> =
                    generics.iter().copied().zip(args.iter().cloned()).collect();
                let found = match &self.p.defs[*d].kind {
                    DefKind::Struct(f) | DefKind::Error(f) => f
                        .iter()
                        .find(|f| f.name == name.text)
                        .map(|f| (f.ty.subst(&subst, None), f.public)),
                    _ => None,
                };
                match found {
                    Some((t, public)) => {
                        let owner = self.p.defs[*d].module;
                        self.check_visible(owner, public, &name.text, name.span, Node::Def(*d));
                        t
                    }
                    None => {
                        let shown = self.p.show(&ty);
                        let related = self.types_related(&[&ty]);
                        let message = catalog::no_field(&name.text, &shown);
                        self.err_with("E0056", name.span, message, related);
                        Ty::Unknown
                    }
                }
            }
            Ty::Unknown | Ty::Never | Ty::Param(_) | Ty::SelfTy | Ty::Var(_) => Ty::Unknown,
            other => {
                let shown = self.p.show(other);
                self.err("E0056", name.span, catalog::no_field(&name.text, &shown));
                Ty::Unknown
            }
        }
    }

    fn field(&mut self, target: &ast::Expr, name: &ast::Name, span: Span, expected: &Ty) -> Ty {
        if let Some(m) = self.module_ref(target) {
            return match self.p.modules[m].symbols.get(&name.text).copied() {
                Some(symbol) => {
                    let public = self.p.symbol_public(symbol);
                    let node = crate::program::symbol_node(symbol);
                    self.check_visible(m, public, &name.text, name.span, node);
                    self.symbol_type(m, symbol, name.span)
                }
                None => {
                    let module_name = self.p.modules[m].name.clone();
                    self.err(
                        "E0056",
                        name.span,
                        catalog::not_in_module(&name.text, &module_name),
                    );
                    Ty::Unknown
                }
            };
        }
        if let Some(d) = self.type_ref(target) {
            if let DefKind::Enum(vs) = &self.p.defs[d].kind {
                if let Some(v) = vs.iter().position(|v| v.name == name.text) {
                    return self.construct_variant(d, v, &[], span, expected);
                }
            }
            let type_name = self.p.defs[d].name.clone();
            self.err(
                "E0056",
                name.span,
                catalog::no_member(&name.text, &type_name),
            );
            return Ty::Unknown;
        }
        self.use_ok.insert(span_key(target.span));
        let t = self.expr(target, &Ty::Unknown);
        if let Ty::Named(d, _) = self.resolved(&t) {
            self.field_owner.insert(span_key(span), d);
        }
        self.field_of(&t, name)
    }

    fn call_expr(
        &mut self,
        callee: &ast::Expr,
        types: Option<&Vec<ast::Type>>,
        args: &[ast::Argument],
        span: Span,
        expected: &Ty,
    ) -> Ty {
        let explicit = types.map(|t| t.as_slice());
        match &callee.kind {
            ExprKind::Name(n) if self.find_local(n).is_none() => {
                if !self.short_variants(n).is_empty()
                    && self
                        .p
                        .lookup(self.module, n)
                        .is_none_or(|(_, s)| matches!(s, Symbol::Def(_)))
                {
                    let (d, v) = self.variant_named(n, callee.span).unwrap();
                    return self.construct_variant(d, v, args, span, expected);
                }
                match self.p.lookup(self.module, n) {
                    Some((owner, Symbol::Func(f))) => {
                        let public = self.p.funcs[f].public;
                        self.check_visible(owner, public, n, callee.span, Node::Func(f));
                        self.call_function(f, HashMap::new(), None, explicit, args, span, expected)
                    }
                    Some((owner, Symbol::Def(d)))
                        if matches!(
                            self.p.defs[d].kind,
                            DefKind::Struct(_) | DefKind::Error(_)
                        ) =>
                    {
                        let public = self.p.defs[d].public;
                        self.check_visible(owner, public, n, callee.span, Node::Def(d));
                        self.construct(d, explicit, args, span, expected)
                    }
                    _ => {
                        let t = self.expr(callee, &Ty::Unknown);
                        let flow = self.value_of(callee, &t);
                        self.call_value(&t, flow, args, span, expected, callee.span)
                    }
                }
            }
            ExprKind::Field { target, name } => {
                if let Some(m) = self.module_ref(target) {
                    return match self.p.modules[m].symbols.get(&name.text).copied() {
                        Some(Symbol::Func(f)) => {
                            let public = self.p.funcs[f].public;
                            self.check_visible(m, public, &name.text, name.span, Node::Func(f));
                            self.call_function(
                                f,
                                HashMap::new(),
                                None,
                                explicit,
                                args,
                                span,
                                expected,
                            )
                        }
                        Some(Symbol::Def(d))
                            if matches!(
                                self.p.defs[d].kind,
                                DefKind::Struct(_) | DefKind::Error(_)
                            ) =>
                        {
                            let public = self.p.defs[d].public;
                            self.check_visible(m, public, &name.text, name.span, Node::Def(d));
                            self.construct(d, explicit, args, span, expected)
                        }
                        _ => {
                            let t = self.field(target, name, span, &Ty::Unknown);
                            let flow = self.value_of(callee, &t);
                            self.call_value(&t, flow, args, span, expected, callee.span)
                        }
                    };
                }
                if let Some(d) = self.type_ref(target) {
                    if let DefKind::Enum(vs) = &self.p.defs[d].kind {
                        if let Some(v) = vs.iter().position(|v| v.name == name.text) {
                            return self.construct_variant(d, v, args, span, expected);
                        }
                    }
                    // A function of the type: a member without self (L-23d).
                    let target_ty = Ty::Named(
                        d,
                        self.p.defs[d]
                            .generics
                            .iter()
                            .map(|_| Ty::Unknown)
                            .collect(),
                    );
                    if let Some((f, subst, self_ty)) = self.find_method(&target_ty, &name.text) {
                        if self.p.funcs[f].self_param.is_none() {
                            let (owner, public) = (self.p.funcs[f].module, self.p.funcs[f].public);
                            self.check_visible(owner, public, &name.text, name.span, Node::Func(f));
                            return self.call_function(
                                f,
                                subst,
                                Some(self_ty),
                                explicit,
                                args,
                                span,
                                expected,
                            );
                        }
                    }
                    let type_name = self.p.defs[d].name.clone();
                    self.err(
                        "E0056",
                        name.span,
                        catalog::no_member(&name.text, &type_name),
                    );
                    for a in args {
                        self.expr(&a.value, &Ty::Unknown);
                    }
                    return Ty::Unknown;
                }
                self.use_ok.insert(span_key(target.span));
                let recv = self.expr(target, &Ty::Unknown);
                let recv = self.resolved(&recv);
                if matches!(recv, Ty::Unknown | Ty::Never) {
                    self.uncertain();
                    for a in args {
                        self.expr(&a.value, &Ty::Unknown);
                    }
                    return Ty::Unknown;
                }
                if let Some((f, subst, self_ty)) = self.find_method(&recv, &name.text) {
                    let (owner, public) = (self.p.funcs[f].module, self.p.funcs[f].public);
                    self.check_visible(owner, public, &name.text, name.span, Node::Func(f));
                    if self.p.funcs[f].self_param == Some(true) && self.silent == 0 {
                        // A `var self` method changes its receiver (L-11).
                        let (root, bad) = self.place(target);
                        if !Checker::mutable_root(&root) {
                            let who = Checker::root_name(&root);
                            self.err(
                                "E0078",
                                target.span,
                                catalog::method_changes(&name.text, &who),
                            );
                        } else if let Some(field) = bad {
                            self.err("E0078", target.span, catalog::field_not_var(&field));
                        } else {
                            self.var_receiver = Some(Checker::root_name(&root));
                        }
                    }
                    let result =
                        self.call_function(f, subst, Some(self_ty), explicit, args, span, expected);
                    self.var_receiver = None;
                    return result;
                }
                // A field that holds a function.
                if let Ty::Named(d, _) = &recv {
                    let has_field = match &self.p.defs[*d].kind {
                        DefKind::Struct(f) | DefKind::Error(f) => {
                            f.iter().any(|f| f.name == name.text)
                        }
                        _ => false,
                    };
                    if has_field {
                        let t = self.field_of(&recv, name);
                        let flow = vec![Atom::Field(*d, name.text.clone())];
                        return self.call_value(&t, flow, args, span, expected, callee.span);
                    }
                }
                let shown = self.p.show(&recv);
                if self.is_range(&recv)
                    && self
                        .find_method(&Ty::List(Box::new(Ty::int())), &name.text)
                        .is_some()
                {
                    // A list method on a Range (S-15).
                    let message = catalog::range_method(&name.text);
                    self.range_as_list(target.span, message);
                } else if self.float_blocks_method(&recv, &name.text) {
                    let element = match &recv {
                        Ty::List(t) | Ty::Set(t) => self.p.show(t),
                        _ => shown.clone(),
                    };
                    self.err(
                        "E0045",
                        name.span,
                        catalog::float_method(&name.text, &element),
                    );
                } else {
                    let related = self.types_related(&[&recv]);
                    let message = catalog::no_method(&name.text, &shown);
                    self.err_with("E0056", name.span, message, related);
                }
                for a in args {
                    self.expr(&a.value, &Ty::Unknown);
                }
                Ty::Unknown
            }
            ExprKind::SafeField { target, name } => {
                let t = self.expr(target, &Ty::Unknown);
                let inner = match self.resolved(&t) {
                    Ty::Named(d, args) if Some(d) == self.known.option => {
                        args.first().cloned().unwrap_or(Ty::Unknown)
                    }
                    _ => Ty::Unknown,
                };
                let result = match self.find_method(&inner, &name.text) {
                    Some((f, subst, self_ty)) => self.call_function(
                        f,
                        subst,
                        Some(self_ty),
                        explicit,
                        args,
                        span,
                        &Ty::Unknown,
                    ),
                    None => {
                        for a in args {
                            self.expr(&a.value, &Ty::Unknown);
                        }
                        Ty::Unknown
                    }
                };
                match (self.known.option, result) {
                    (Some(o), r @ Ty::Named(d, _)) if d == o => r,
                    (Some(o), r) => Ty::Named(o, vec![r]),
                    (None, r) => r,
                }
            }
            _ => {
                let t = self.expr(callee, &Ty::Unknown);
                let flow = self.value_of(callee, &t);
                self.call_value(&t, flow, args, span, expected, callee.span)
            }
        }
    }

    /// A call through a function value: it has the effects of the values `flow` may be
    /// (RFC 0005).
    fn call_value(
        &mut self,
        t: &Ty,
        flow: Flow,
        args: &[ast::Argument],
        span: Span,
        expected: &Ty,
        callee: Span,
    ) -> Ty {
        match self.resolved(t) {
            Ty::Func(f) => {
                let params = self.value_params(&f);
                let result = self.call(params, f.ret.clone(), args, span, expected);
                let flows = std::mem::take(&mut self.last_args);
                self.add_use(Callee::Value(flow), flows, span);
                self.fallible_call(f.fails.clone(), span, None);
                result
            }
            Ty::Unknown | Ty::Never => {
                self.uncertain();
                for a in args {
                    self.expr(&a.value, &Ty::Unknown);
                }
                Ty::Unknown
            }
            other => {
                let shown = self.p.show(&other);
                self.err("E0061", callee, catalog::not_callable(&shown));
                for a in args {
                    self.expr(&a.value, &Ty::Unknown);
                }
                Ty::Unknown
            }
        }
    }

    // ------------------------------------------------------------ patterns

    fn pattern(&mut self, p: &ast::Pattern, subject: &Ty, out: &mut Vec<(String, Ty, Span)>) {
        let subject = self.resolved(subject);
        match &p.kind {
            PatternKind::Wildcard => {}
            PatternKind::Literal(l) => {
                let t = match l {
                    ast::Literal::Int(_) => Ty::int(),
                    ast::Literal::Text(parts) => {
                        self.text(parts);
                        Ty::text()
                    }
                    ast::Literal::Bool(_) => Ty::bool(),
                };
                self.expect(&t, &subject, p.span);
            }
            PatternKind::Name(path) => {
                if path.len() == 1 {
                    let n = &path[0].text;
                    if let Some(v) = self.variant_in(&subject, n) {
                        let _ = v;
                        return;
                    }
                    if !self.short_variants(n).is_empty()
                        && (subject.is_unknown()
                            || n.chars().next().is_some_and(char::is_uppercase))
                    {
                        if let Some((d, _)) = self.variant_named(n, path[0].span) {
                            let t = Ty::Named(
                                d,
                                self.p.defs[d]
                                    .generics
                                    .iter()
                                    .map(|_| Ty::Unknown)
                                    .collect(),
                            );
                            self.expect(&t, &subject, p.span);
                        }
                        return;
                    }
                    if n.chars().next().is_some_and(char::is_uppercase) {
                        // A type test: `UsageError as e` inside error(...).
                        match self.p.lookup(self.module, n) {
                            Some((_, Symbol::Def(d))) => {
                                let t = Ty::Named(
                                    d,
                                    self.p.defs[d]
                                        .generics
                                        .iter()
                                        .map(|_| Ty::Unknown)
                                        .collect(),
                                );
                                if !self.compatible(&t, &subject) {
                                    let (e, a) = (self.p.show(&subject), self.p.show(&t));
                                    let message = catalog::type_mismatch(&e, &a);
                                    let related = self.types_related(&[&subject, &t]);
                                    self.err_expecting("E0061", p.span, message, &subject, related);
                                }
                            }
                            _ => self.unknown_name(n, path[0].span),
                        }
                        return;
                    }
                    out.push((n.clone(), subject, path[0].span));
                } else {
                    self.qualified_variant(path, &subject, &[], p.span, out);
                }
            }
            PatternKind::Constructor { name, arguments } => {
                if name.len() == 1 {
                    let n = &name[0].text;
                    let found = match self.variant_in(&subject, n) {
                        Some((d, v)) => Some((d, v)),
                        None => self.variant_named(n, name[0].span),
                    };
                    match found {
                        Some((d, v)) => self.variant_fields(d, v, &subject, arguments, p.span, out),
                        None => {
                            self.unknown_name(n, name[0].span);
                            for (_, a) in arguments {
                                self.pattern(a, &Ty::Unknown, out);
                            }
                        }
                    }
                } else {
                    self.qualified_variant(name, &subject, arguments, p.span, out);
                }
            }
            PatternKind::List(elements) => {
                let item = match &subject {
                    Ty::List(t) => (**t).clone(),
                    Ty::Unknown => Ty::Unknown,
                    other => {
                        let shown = self.p.show(other);
                        self.err("E0061", p.span, catalog::type_mismatch(&shown, "List"));
                        Ty::Unknown
                    }
                };
                for el in elements {
                    match el {
                        ast::ListElement::Rest(n) => {
                            out.push((n.text.clone(), Ty::List(Box::new(item.clone())), n.span))
                        }
                        ast::ListElement::Pattern(q) => self.pattern(q, &item, out),
                    }
                }
            }
            PatternKind::Tuple(items) => {
                let parts = match &subject {
                    Ty::Tuple(ts) if ts.len() == items.len() => ts.clone(),
                    Ty::Unknown => vec![Ty::Unknown; items.len()],
                    other => {
                        let shown = self.p.show(other);
                        self.err(
                            "E0061",
                            p.span,
                            catalog::cannot_destructure(&shown, items.len()),
                        );
                        vec![Ty::Unknown; items.len()]
                    }
                };
                for (q, t) in items.iter().zip(parts) {
                    self.pattern(q, &t, out);
                }
            }
            PatternKind::Bind { pattern, name } => {
                self.pattern(pattern, &subject, out);
                // A type test binds the tested type.
                let bound = match &pattern.kind {
                    PatternKind::Name(path)
                        if path.len() == 1
                            && path[0].text.chars().next().is_some_and(char::is_uppercase)
                            && self.variant_in(&subject, &path[0].text).is_none() =>
                    {
                        match self.p.lookup(self.module, &path[0].text) {
                            Some((_, Symbol::Def(d))) => Ty::Named(d, Vec::new()),
                            _ => subject.clone(),
                        }
                    }
                    _ => subject.clone(),
                };
                out.push((name.text.clone(), bound, name.span));
            }
            PatternKind::Or(alternatives) => {
                let mut first = Vec::new();
                for (i, a) in alternatives.iter().enumerate() {
                    let mut b = Vec::new();
                    self.pattern(a, &subject, &mut b);
                    if i == 0 {
                        first = b;
                    }
                }
                out.extend(first);
            }
        }
    }

    /// The variant `name` of the subject's enum, if the subject is an enum that has it.
    fn variant_in(&self, subject: &Ty, name: &str) -> Option<(DefId, usize)> {
        let Ty::Named(d, _) = subject else {
            return None;
        };
        let DefKind::Enum(vs) = &self.p.defs[*d].kind else {
            return None;
        };
        vs.iter().position(|v| v.name == name).map(|v| (*d, v))
    }

    fn qualified_variant(
        &mut self,
        path: &[ast::Name],
        subject: &Ty,
        arguments: &[(Option<ast::Name>, ast::Pattern)],
        span: Span,
        out: &mut Vec<(String, Ty, Span)>,
    ) {
        let (type_path, variant) = path.split_at(path.len() - 1);
        let variant = &variant[0];
        let def = match type_path {
            [t] => match self.p.lookup(self.module, &t.text) {
                Some((_, Symbol::Def(d))) => Some(d),
                _ => None,
            },
            [alias, t] => self.p.module_alias(self.module, &alias.text).and_then(|m| {
                match self.p.modules[m].symbols.get(&t.text) {
                    Some(Symbol::Def(d)) => Some(*d),
                    _ => None,
                }
            }),
            _ => None,
        };
        let Some(d) = def else {
            if self.p.modules[self.module]
                .failed_imports
                .contains(&type_path[0].text)
            {
                return;
            }
            let written = type_path
                .iter()
                .map(|n| n.text.as_str())
                .collect::<Vec<_>>()
                .join(".");
            self.err("E0056", type_path[0].span, catalog::unknown_type(&written));
            return;
        };
        let index = match &self.p.defs[d].kind {
            DefKind::Enum(vs) => vs.iter().position(|v| v.name == variant.text),
            _ => None,
        };
        match index {
            Some(v) => self.variant_fields(d, v, subject, arguments, span, out),
            None => {
                let type_name = self.p.defs[d].name.clone();
                self.err(
                    "E0056",
                    variant.span,
                    catalog::no_member(&variant.text, &type_name),
                );
            }
        }
    }

    fn variant_fields(
        &mut self,
        d: DefId,
        v: usize,
        subject: &Ty,
        arguments: &[(Option<ast::Name>, ast::Pattern)],
        span: Span,
        out: &mut Vec<(String, Ty, Span)>,
    ) {
        let args = match subject {
            Ty::Named(sd, args) if *sd == d => args.clone(),
            Ty::Unknown => self.p.defs[d]
                .generics
                .iter()
                .map(|_| Ty::Unknown)
                .collect(),
            other => {
                let (e, a) = (self.p.show(other), self.p.defs[d].name.clone());
                let message = catalog::type_mismatch(&e, &a);
                let related = self.types_related(&[other]);
                let expected = other.clone();
                self.err_expecting("E0061", span, message, &expected, related);
                self.p.defs[d]
                    .generics
                    .iter()
                    .map(|_| Ty::Unknown)
                    .collect()
            }
        };
        let subst: HashMap<ParamId, Ty> =
            self.p.defs[d].generics.iter().copied().zip(args).collect();
        let fields: Vec<(String, Ty)> = match &self.p.defs[d].kind {
            DefKind::Enum(vs) => vs[v]
                .fields
                .iter()
                .map(|(n, t)| (n.clone(), t.subst(&subst, None)))
                .collect(),
            _ => Vec::new(),
        };
        let mut position = 0;
        for (name, q) in arguments {
            let t = match name {
                Some(n) => fields.iter().find(|f| f.0 == n.text).map(|f| f.1.clone()),
                None => {
                    position += 1;
                    fields.get(position - 1).map(|f| f.1.clone())
                }
            };
            match t {
                Some(t) => self.pattern(q, &t, out),
                None => {
                    let variant = match &self.p.defs[d].kind {
                        DefKind::Enum(vs) => vs[v].name.clone(),
                        _ => String::new(),
                    };
                    self.err(
                        "E0063",
                        q.span,
                        catalog::variant_fields(&variant, fields.len()),
                    );
                    self.pattern(q, &Ty::Unknown, out);
                }
            }
        }
    }
}

fn prelude_iface(p: &Program, name: &str) -> Option<DefId> {
    prelude_def(p, name)
}

/// Every combination of one pattern from each column.
fn product(columns: &[Vec<Pat>], overflow: &std::cell::Cell<bool>) -> Vec<Vec<Pat>> {
    let mut result = vec![Vec::new()];
    for column in columns {
        let mut next = Vec::new();
        for prefix in &result {
            for p in column {
                let mut row = prefix.clone();
                row.push(p.clone());
                next.push(row);
            }
        }
        result = next;
        if result.len() > 256 {
            // Too many combinations to enumerate: the pattern counts as matching nothing, and
            // the match is not taken as covered (semantics 6.5).
            overflow.set(true);
            return Vec::new();
        }
    }
    result
}
