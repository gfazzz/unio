//! The units of native code and the references they need at run time (docs/runtime.md 8.4).
//!
//! A unit is a lowered body: a function, a constant, the default of a field or of a parameter
//! of an external function, or a lambda. The build and the executable both lower the same
//! program and enumerate its units with [`collect`], in the same order; [`refs`] then walks
//! each unit in one canonical order and lists the lowered statements, nodes, patterns, and
//! literal values that its native code passes to the runtime. The build numbers them, and the
//! executable fills a table of their addresses with the same numbers before `main` runs, so
//! native code reaches its call-site caches and literals with one load.

use crate::interp::Interp;
use crate::lower::{Block, Code, ExprCode, LambdaCode, Node, PStep, Part, Pat, Place, Stmt, S};
use crate::program::DefKind;
use std::rc::Rc;
use unio_runtime::value::Value;

pub(crate) enum Body {
    Func(usize, Rc<Code>),
    Expr(Rc<ExprCode>),
    Lambda(Rc<LambdaCode>),
}

pub(crate) struct Unit {
    pub module: usize,
    pub body: Body,
}

/// One reference of a unit, by address.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Ref {
    Stmt(*const S),
    Node(*const Node),
    Pat(*const Pat),
    Value(*const Value),
}

impl Ref {
    pub fn address(self) -> usize {
        match self {
            Ref::Stmt(p) => p as usize,
            Ref::Node(p) => p as usize,
            Ref::Pat(p) => p as usize,
            Ref::Value(p) => p as usize,
        }
    }
}

/// Every unit of the program, lowering each body: functions with a body by number, then
/// constants, defaults of fields, defaults of external parameters, then the lambdas found
/// inside the units before them.
pub(crate) fn collect(interp: &Interp) -> Vec<Unit> {
    let program = &interp.program;
    let mut units = Vec::new();
    for (f, def) in program.funcs.iter().enumerate() {
        if def.body.is_none() {
            continue;
        }
        if interp.code(f).is_none() {
            continue;
        }
        if let Some(code) = interp.codes[f].get() {
            units.push(Unit {
                module: def.module,
                body: Body::Func(f, Rc::clone(code)),
            });
        }
    }
    for c in 0..program.consts.len() {
        if let Some(code) = interp.const_code(c) {
            units.push(Unit {
                module: program.consts[c].module,
                body: Body::Expr(code),
            });
        }
    }
    for (d, def) in program.defs.iter().enumerate() {
        if let DefKind::Struct(fields) | DefKind::Error(fields) = &def.kind {
            for (i, field) in fields.iter().enumerate() {
                if !field.has_default {
                    continue;
                }
                if let Some(code) = interp.field_default_code(d, i) {
                    units.push(Unit {
                        module: def.module,
                        body: Body::Expr(code),
                    });
                }
            }
        }
    }
    for (f, def) in program.funcs.iter().enumerate() {
        if def.body.is_some() {
            continue;
        }
        for (i, param) in def.params.iter().enumerate() {
            if !param.has_default {
                continue;
            }
            if let Some(code) = interp.extern_default_code(f, i) {
                units.push(Unit {
                    module: def.module,
                    body: Body::Expr(code),
                });
            }
        }
    }
    let mut next = 0;
    while next < units.len() {
        let mut lambdas = Vec::new();
        let mut ignored = Vec::new();
        refs(&units[next], &mut ignored, &mut lambdas);
        let module = units[next].module;
        for code in lambdas {
            units.push(Unit {
                module,
                body: Body::Lambda(code),
            });
        }
        next += 1;
    }
    units
}

/// The references of a unit in canonical order, and the lambdas it creates.
pub(crate) fn refs(unit: &Unit, out: &mut Vec<Ref>, lambdas: &mut Vec<Rc<LambdaCode>>) {
    let mut w = Walk { out, lambdas };
    match &unit.body {
        Body::Func(_, code) => {
            for p in &code.params {
                if let Some(d) = &p.default {
                    w.node(d);
                }
            }
            w.block(&code.body);
        }
        Body::Expr(code) => w.node(&code.body),
        Body::Lambda(code) => w.node(&code.body),
    }
}

struct Walk<'a> {
    out: &'a mut Vec<Ref>,
    lambdas: &'a mut Vec<Rc<LambdaCode>>,
}

impl Walk<'_> {
    fn block(&mut self, b: &Block) {
        for s in &b.stmts {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        self.s(&s.kind);
    }

    fn s(&mut self, s: &S) {
        self.out.push(Ref::Stmt(s));
        match s {
            S::Let(_, n) | S::Fail(n) | S::Assert(n, _) | S::Expr(n) => self.node(n),
            S::Return(n) => {
                if let Some(n) = n {
                    self.node(n);
                }
            }
            S::Assign(place, n) => {
                self.place(place);
                self.node(n);
            }
            S::SetIndex(place, k, v) => {
                self.place(place);
                self.node(k);
                self.node(v);
            }
            S::Concat(_, parts, general) => {
                for p in parts {
                    self.node(p);
                }
                self.node(general);
            }
            S::Break | S::Continue | S::Pass => {}
            S::Handled { inner, block, .. } => {
                self.s(inner);
                self.block(block);
            }
            S::If(branches, otherwise) => {
                for (c, b) in branches {
                    self.node(c);
                    self.block(b);
                }
                if let Some(b) = otherwise {
                    self.block(b);
                }
            }
            S::While(c, b) => {
                self.node(c);
                self.block(b);
            }
            S::For(_, n, b) | S::Using(_, n, b) => {
                self.node(n);
                self.block(b);
            }
            S::Match(n, cases) => {
                self.node(n);
                for case in cases {
                    self.out.push(Ref::Pat(&case.pattern));
                    if let Some(g) = &case.guard {
                        self.node(g);
                    }
                    self.block(&case.body);
                }
            }
            S::Try(b, handlers) => {
                self.block(b);
                for h in handlers {
                    self.block(&h.body);
                }
            }
        }
    }

    fn place(&mut self, place: &Place) {
        for step in &place.steps {
            if let PStep::Index(n) = step {
                self.node(n);
            }
        }
    }

    fn node(&mut self, n: &Node) {
        if let Node::Value(v) = n {
            self.out.push(Ref::Value(v));
            return;
        }
        self.out.push(Ref::Node(n));
        match n {
            Node::Value(_)
            | Node::Local(_)
            | Node::Const(_)
            | Node::FnRef(_)
            | Node::Variant(..)
            | Node::Fault(_) => {}
            Node::Text(parts) => {
                for p in parts {
                    if let Part::Value(v) = p {
                        self.node(v);
                    }
                }
            }
            Node::Tuple(items) | Node::List(items) => {
                for i in items {
                    self.node(i);
                }
            }
            Node::Map(entries) => {
                for (k, v) in entries {
                    self.node(k);
                    self.node(v);
                }
            }
            Node::Comprehension(c) => {
                self.node(&c.iterable);
                if let Some(cond) = &c.condition {
                    self.node(cond);
                }
                self.node(&c.element);
            }
            Node::Lambda(code) => self.lambdas.push(Rc::clone(code)),
            Node::Is(v, pat) => {
                self.node(v);
                self.out.push(Ref::Pat(&**pat));
            }
            Node::Conditional(parts) => {
                for p in parts.iter() {
                    self.node(p);
                }
            }
            Node::And(l, r) | Node::Or(l, r, _) | Node::Binary(_, l, r) | Node::Index(l, r) => {
                self.node(l);
                self.node(r);
            }
            Node::Not(v) | Node::Negate(v) | Node::Try(v, _) => self.node(v),
            Node::Field(t, _, _) | Node::SafeField(t, _) => self.node(t),
            Node::Call(call) => {
                for a in &call.args {
                    self.node(&a.value);
                    if let Some(p) = &a.place {
                        self.place(p);
                    }
                }
            }
            Node::CallValue(callee, args) => {
                self.node(callee);
                for a in args {
                    self.node(a);
                }
            }
            Node::Construct(c) => {
                for (_, v) in &c.args {
                    self.node(v);
                }
            }
            Node::Method(m) => {
                self.node(&m.receiver);
                for a in &m.args {
                    self.node(&a.value);
                    if let Some(p) = &a.place {
                        self.place(p);
                    }
                }
                if let Some(p) = &m.place {
                    self.place(p);
                }
            }
        }
    }
}
