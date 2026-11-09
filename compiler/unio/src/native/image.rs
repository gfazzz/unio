//! The program image of an executable made by `unio build` (docs/runtime.md 8.4): what the run
//! needs of the checked program, prepared by the build, so that the executable starts without
//! the front end. It holds the declarations (modules, types with their fields and variants,
//! functions with their parameters, constants, implementations), every lowered body that native
//! code refers to, the permissions the manifest declares, and, apart from them, what traces
//! show: the file of each module and the line of each function. It holds no source and no path
//! of the machine that built it: the files of modules are relative to the root of their package
//! or `std:` for the standard library ([`Program::trace_path`]).
//!
//! The format is versioned. It starts with [`MAGIC`], the format version [`FORMAT`] as a 32-bit
//! little-endian number, the identity of the compiler ([`compiler`]), and the identity of the
//! toolchain that linked the executable ([`super::toolchain`]), which the run does not check and
//! [`made_by`] reads back; then come sections in
//! a fixed order: the declared permissions, the modules, the generic parameters, the types, the
//! functions, the constants, the implementations, the diagnostic metadata, and the units of
//! native code without the lambdas, which the executable finds inside them as the build did
//! ([`super::units::add_lambdas`]). Numbers are LEB128 (integers zigzag-encoded), floats their
//! IEEE bits, texts a length and UTF-8 bytes, lists a length and their items. Declarations are
//! numbered in the order of the build and named by their module and name, from which the
//! interpreter derives the identity of each type as for `unio run`; external functions are
//! bound by name at start. The same sources give the same bytes (C-4).

use crate::interp::Interp;
use crate::lower::{
    ArgCode, Block, CallCode, CaseCode, Code, ComprehensionCode, ConstructCode, ExprCode,
    HandlerCode, LambdaCode, ListPat, MethodArg, MethodCode, NameFallback, Node, PStep, ParamCode,
    Part, Pat, Place, Stmt, TypeBase, TypeCode, S,
};
use crate::program::{
    BodyRef, ConstDef, Def, DefKind, FieldDef, FnDef, Impl, Owner, ParamDef, ParamInfo,
    PreparedModule, Program, VariantDef,
};
use crate::types::{FnTy, Prim, Ty};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use unio_runtime::sandbox::Declared;
use unio_runtime::value::Value;

use super::support::OPS;
use super::units::{Body, Key, Unit};

/// The first bytes of an image.
pub const MAGIC: &[u8; 8] = b"UNIOPRG\0";
/// The version of the format; a change of the layout of the image or of the lowered form
/// changes it. Format 2 added the identity of the toolchain after that of the compiler.
pub const FORMAT: u32 = 2;

/// The identity of this compiler and of the runtime built with it: the version and a hash of
/// their sources (`build.rs`). An image is run only by the runtime of the compiler that made it.
pub fn compiler() -> String {
    format!(
        "unio {} {}",
        env!("CARGO_PKG_VERSION"),
        env!("UNIO_SOURCE_HASH")
    )
}

/// Why an executable does not run its image; it ends with code 104 (`EXIT_IMAGE`).
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    Format(u32),
    Compiler(String),
    Damaged(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Format(found) => write!(
                f,
                "this executable holds a program image of format {found}, and its runtime reads format {FORMAT}; build it again with one version of unio"
            ),
            Refusal::Compiler(found) => write!(
                f,
                "this executable holds a program image made by another compiler ({found}) than its runtime ({}); build it again with one version of unio",
                compiler()
            ),
            Refusal::Damaged(what) => {
                write!(f, "the program image of this executable is damaged ({what})")
            }
        }
    }
}

// ---------------------------------------------------------------- writing

struct W {
    out: Vec<u8>,
}

impl W {
    fn u(&mut self, mut n: u64) {
        loop {
            let byte = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                self.out.push(byte);
                return;
            }
            self.out.push(byte | 0x80);
        }
    }
    fn n(&mut self, n: usize) {
        self.u(n as u64);
    }
    fn i(&mut self, i: i64) {
        self.u(((i << 1) ^ (i >> 63)) as u64);
    }
    fn b(&mut self, b: bool) {
        self.out.push(u8::from(b));
    }
    fn tag(&mut self, t: u8) {
        self.out.push(t);
    }
    fn s(&mut self, s: &str) {
        self.n(s.len());
        self.out.extend_from_slice(s.as_bytes());
    }
    fn opt_s(&mut self, s: Option<&str>) {
        match s {
            Some(s) => {
                self.b(true);
                self.s(s);
            }
            None => self.b(false),
        }
    }
    fn list<T>(&mut self, items: &[T], mut each: impl FnMut(&mut W, &T)) {
        self.n(items.len());
        for item in items {
            each(self, item);
        }
    }
    fn texts(&mut self, items: &[String]) {
        self.list(items, |w, s| w.s(s));
    }
    fn numbers(&mut self, items: &[usize]) {
        self.list(items, |w, n| w.n(*n));
    }
}

fn prim_tag(p: Prim) -> u8 {
    match p {
        Prim::Int => 0,
        Prim::Float => 1,
        Prim::Bool => 2,
        Prim::Text => 3,
        Prim::Bytes => 4,
        Prim::Path => 5,
    }
}

fn put_ty(w: &mut W, ty: &Ty) {
    match ty {
        Ty::Unknown => w.tag(0),
        Ty::Never => w.tag(1),
        Ty::Unit => w.tag(2),
        Ty::Prim(p) => {
            w.tag(3);
            w.tag(prim_tag(*p));
        }
        Ty::List(t) => {
            w.tag(4);
            put_ty(w, t);
        }
        Ty::Map(k, v) => {
            w.tag(5);
            put_ty(w, k);
            put_ty(w, v);
        }
        Ty::Set(t) => {
            w.tag(6);
            put_ty(w, t);
        }
        Ty::Tuple(ts) => {
            w.tag(7);
            w.list(ts, put_ty);
        }
        Ty::Func(f) => {
            w.tag(8);
            w.list(&f.params, |w, (label, t)| {
                w.opt_s(label.as_deref());
                put_ty(w, t);
            });
            put_ty(w, &f.ret);
            w.list(&f.fails, put_ty);
        }
        Ty::Named(d, args) => {
            w.tag(9);
            w.n(*d);
            w.list(args, put_ty);
        }
        Ty::Param(p) => {
            w.tag(10);
            w.n(*p);
        }
        Ty::Var(v) => {
            w.tag(11);
            w.n(*v);
        }
        Ty::SelfTy => w.tag(12),
    }
}

fn put_value(w: &mut W, v: &Value) -> Result<(), String> {
    match v {
        Value::Unit => w.tag(0),
        Value::Int(i) => {
            w.tag(1);
            w.i(*i);
        }
        Value::Float(x) => {
            w.tag(2);
            w.u(x.to_bits());
        }
        Value::Bool(b) => {
            w.tag(3);
            w.b(*b);
        }
        Value::Text(t) => {
            w.tag(4);
            w.s(t.as_str());
        }
        Value::Path(t) => {
            w.tag(5);
            w.s(t.as_str());
        }
        Value::Bytes(b) => {
            w.tag(6);
            w.n(b.len());
            w.out.extend_from_slice(&b[..]);
        }
        Value::List(items) | Value::Tuple(items) => {
            w.tag(if matches!(v, Value::List(_)) { 7 } else { 8 });
            w.n(items.len());
            for item in items.iter() {
                put_value(w, item)?;
            }
        }
        _ => return Err("a literal value of a kind the image does not hold".into()),
    }
    Ok(())
}

/// Writes lowered code; literal values of kinds a literal cannot have are an error.
struct Put<'w> {
    w: &'w mut W,
}

impl Put<'_> {
    fn slot(&mut self, s: u32) {
        self.w.u(u64::from(s));
    }

    fn slots(&mut self, s: &[u32]) {
        self.w.n(s.len());
        for x in s {
            self.slot(*x);
        }
    }

    fn block(&mut self, b: &Block) -> Result<(), String> {
        self.w.n(b.stmts.len());
        for s in &b.stmts {
            self.w.u(u64::from(s.line));
            self.s(&s.kind)?;
        }
        self.slot(b.clear.0);
        self.slot(b.clear.1);
        Ok(())
    }

    fn opt_node(&mut self, n: Option<&Node>) -> Result<(), String> {
        match n {
            Some(n) => {
                self.w.b(true);
                self.node(n)
            }
            None => {
                self.w.b(false);
                Ok(())
            }
        }
    }

    fn nodes(&mut self, ns: &[Node]) -> Result<(), String> {
        self.w.n(ns.len());
        ns.iter().try_for_each(|n| self.node(n))
    }

    fn opt_slot(&mut self, s: Option<u32>) {
        match s {
            Some(s) => {
                self.w.b(true);
                self.slot(s);
            }
            None => self.w.b(false),
        }
    }

    fn s(&mut self, s: &S) -> Result<(), String> {
        match s {
            S::Let(slot, n) => {
                self.w.tag(0);
                self.slot(*slot);
                self.node(n)?;
            }
            S::Assign(p, n) => {
                self.w.tag(1);
                self.place(p)?;
                self.node(n)?;
            }
            S::SetIndex(p, k, v) => {
                self.w.tag(2);
                self.place(p)?;
                self.node(k)?;
                self.node(v)?;
            }
            S::Concat(slot, parts, general) => {
                self.w.tag(3);
                self.slot(*slot);
                self.nodes(parts)?;
                self.node(general)?;
            }
            S::Return(n) => {
                self.w.tag(4);
                self.opt_node(n.as_ref())?;
            }
            S::Fail(n) => {
                self.w.tag(5);
                self.node(n)?;
            }
            S::Assert(n, text) => {
                self.w.tag(6);
                self.node(n)?;
                self.w.s(text);
            }
            S::Break => self.w.tag(7),
            S::Continue => self.w.tag(8),
            S::Pass => self.w.tag(9),
            S::Expr(n) => {
                self.w.tag(10);
                self.node(n)?;
            }
            S::Handled {
                inner,
                ty,
                bind,
                block,
            } => {
                self.w.tag(11);
                self.s(inner)?;
                self.w.s(ty);
                self.opt_slot(*bind);
                self.block(block)?;
            }
            S::If(branches, otherwise) => {
                self.w.tag(12);
                self.w.n(branches.len());
                for (c, b) in branches {
                    self.node(c)?;
                    self.block(b)?;
                }
                match otherwise {
                    Some(b) => {
                        self.w.b(true);
                        self.block(b)?;
                    }
                    None => self.w.b(false),
                }
            }
            S::While(c, b) => {
                self.w.tag(13);
                self.node(c)?;
                self.block(b)?;
            }
            S::For(names, n, b) => {
                self.w.tag(14);
                self.slots(names);
                self.node(n)?;
                self.block(b)?;
            }
            S::Match(n, cases) => {
                self.w.tag(15);
                self.node(n)?;
                self.w.n(cases.len());
                for case in cases {
                    self.pat(&case.pattern)?;
                    self.opt_node(case.guard.as_ref())?;
                    self.block(&case.body)?;
                }
            }
            S::Try(b, handlers) => {
                self.w.tag(16);
                self.block(b)?;
                self.w.n(handlers.len());
                for h in handlers {
                    self.w.s(&h.ty);
                    self.opt_slot(h.bind);
                    self.block(&h.body)?;
                }
            }
            S::Using(slot, n, b) => {
                self.w.tag(17);
                self.slot(*slot);
                self.node(n)?;
                self.block(b)?;
            }
        }
        Ok(())
    }

    fn place(&mut self, p: &Place) -> Result<(), String> {
        self.slot(p.root);
        self.w.n(p.steps.len());
        for step in &p.steps {
            match step {
                PStep::Field(name) => {
                    self.w.tag(0);
                    self.w.s(name);
                }
                PStep::Index(n) => {
                    self.w.tag(1);
                    self.node(n)?;
                }
            }
        }
        Ok(())
    }

    fn opt_place(&mut self, p: Option<&Place>) -> Result<(), String> {
        match p {
            Some(p) => {
                self.w.b(true);
                self.place(p)
            }
            None => {
                self.w.b(false);
                Ok(())
            }
        }
    }

    fn pat(&mut self, p: &Pat) -> Result<(), String> {
        match p {
            Pat::Wildcard => self.w.tag(0),
            Pat::Int(i) => {
                self.w.tag(1);
                match i {
                    Some(i) => {
                        self.w.b(true);
                        self.w.i(*i);
                    }
                    None => self.w.b(false),
                }
            }
            Pat::Bool(b) => {
                self.w.tag(2);
                self.w.b(*b);
            }
            Pat::Text(t) => {
                self.w.tag(3);
                self.w.s(t);
            }
            Pat::Name { last, fallback, .. } => {
                self.w.tag(4);
                self.w.s(last);
                match fallback {
                    NameFallback::Bind(s) => {
                        self.w.tag(0);
                        self.slot(*s);
                    }
                    NameFallback::Const(c) => {
                        self.w.tag(1);
                        self.w.n(*c);
                    }
                    NameFallback::No => self.w.tag(2),
                }
            }
            Pat::Constructor { last, args } => {
                self.w.tag(5);
                self.w.s(last);
                self.w.n(args.len());
                for (name, p) in args {
                    self.w.opt_s(name.as_deref());
                    self.pat(p)?;
                }
            }
            Pat::List {
                elements,
                fixed,
                rest,
            } => {
                self.w.tag(6);
                self.w.n(elements.len());
                for e in elements {
                    match e {
                        ListPat::Pattern(p) => {
                            self.w.tag(0);
                            self.pat(p)?;
                        }
                        ListPat::Rest(s) => {
                            self.w.tag(1);
                            self.slot(*s);
                        }
                    }
                }
                self.w.n(*fixed);
                self.w.b(*rest);
            }
            Pat::Tuple(ps) => {
                self.w.tag(7);
                self.w.n(ps.len());
                ps.iter().try_for_each(|p| self.pat(p))?;
            }
            Pat::Bind(p, s) => {
                self.w.tag(8);
                self.pat(p)?;
                self.slot(*s);
            }
            Pat::Or(ps) => {
                self.w.tag(9);
                self.w.n(ps.len());
                ps.iter().try_for_each(|p| self.pat(p))?;
            }
        }
        Ok(())
    }

    fn type_code(&mut self, t: &TypeCode) {
        self.w.opt_s(t.generic.as_deref());
        match t.base {
            TypeBase::Int => self.w.tag(0),
            TypeBase::Float => self.w.tag(1),
            TypeBase::Bool => self.w.tag(2),
            TypeBase::Text => self.w.tag(3),
            TypeBase::Bytes => self.w.tag(4),
            TypeBase::Path => self.w.tag(5),
            TypeBase::List => self.w.tag(6),
            TypeBase::Map => self.w.tag(7),
            TypeBase::Set => self.w.tag(8),
            TypeBase::Tuple => self.w.tag(9),
            TypeBase::Def(d) => {
                self.w.tag(10);
                self.w.n(d);
            }
            TypeBase::Unknown => self.w.tag(11),
        }
        self.w.n(t.args.len());
        for a in &t.args {
            self.type_code(a);
        }
    }

    fn lambda(&mut self, l: &LambdaCode) -> Result<(), String> {
        self.w.n(l.slots);
        self.w.n(l.params);
        self.w.n(l.captures.len());
        for (a, b) in &l.captures {
            self.slot(*a);
            self.slot(*b);
        }
        self.node(&l.body)
    }

    fn node(&mut self, n: &Node) -> Result<(), String> {
        match n {
            Node::Value(v) => {
                self.w.tag(0);
                put_value(self.w, v)?;
            }
            Node::Local(s) => {
                self.w.tag(1);
                self.slot(*s);
            }
            Node::Const(c) => {
                self.w.tag(2);
                self.w.n(*c);
            }
            Node::FnRef(f) => {
                self.w.tag(3);
                self.w.n(*f);
            }
            Node::Variant(d, name) => {
                self.w.tag(4);
                self.w.n(*d);
                self.w.s(name);
            }
            Node::Text(parts) => {
                self.w.tag(5);
                self.w.n(parts.len());
                for p in parts {
                    match p {
                        Part::Literal(s) => {
                            self.w.tag(0);
                            self.w.s(s);
                        }
                        Part::Value(n) => {
                            self.w.tag(1);
                            self.node(n)?;
                        }
                    }
                }
            }
            Node::Tuple(items) => {
                self.w.tag(6);
                self.nodes(items)?;
            }
            Node::List(items) => {
                self.w.tag(7);
                self.nodes(items)?;
            }
            Node::Map(entries) => {
                self.w.tag(8);
                self.w.n(entries.len());
                for (k, v) in entries {
                    self.node(k)?;
                    self.node(v)?;
                }
            }
            Node::Comprehension(c) => {
                self.w.tag(9);
                self.node(&c.element)?;
                self.slots(&c.names);
                self.node(&c.iterable)?;
                self.opt_node(c.condition.as_ref())?;
            }
            Node::Lambda(l) => {
                self.w.tag(10);
                self.lambda(l)?;
            }
            Node::Is(v, p) => {
                self.w.tag(11);
                self.node(v)?;
                self.pat(p)?;
            }
            Node::Conditional(parts) => {
                self.w.tag(12);
                for p in parts.iter() {
                    self.node(p)?;
                }
            }
            Node::And(l, r) => {
                self.w.tag(13);
                self.node(l)?;
                self.node(r)?;
            }
            Node::Or(l, r, option) => {
                self.w.tag(14);
                self.node(l)?;
                self.node(r)?;
                self.w.b(*option);
            }
            Node::Not(v) => {
                self.w.tag(15);
                self.node(v)?;
            }
            Node::Negate(v) => {
                self.w.tag(16);
                self.node(v)?;
            }
            Node::Binary(op, l, r) => {
                self.w.tag(17);
                self.w.n(OPS.iter().position(|o| o == op).unwrap_or(0));
                self.node(l)?;
                self.node(r)?;
            }
            Node::Try(v, result) => {
                self.w.tag(18);
                self.node(v)?;
                self.w.b(*result);
            }
            Node::Call(call) => {
                self.w.tag(19);
                self.w.n(call.f);
                self.w.n(call.args.len());
                for a in &call.args {
                    self.w.n(a.target);
                    self.node(&a.value)?;
                    self.opt_place(a.place.as_ref())?;
                }
                self.w.n(call.types.len());
                for t in &call.types {
                    self.type_code(t);
                }
                self.w.n(call.count);
            }
            Node::CallValue(callee, args) => {
                self.w.tag(20);
                self.node(callee)?;
                self.nodes(args)?;
            }
            Node::Construct(c) => {
                self.w.tag(21);
                self.w.n(c.def);
                self.w.opt_s(c.variant.as_deref());
                self.w.n(c.args.len());
                for (i, v) in &c.args {
                    self.w.n(*i);
                    self.node(v)?;
                }
                self.w.n(c.fields);
            }
            Node::Method(m) => {
                self.w.tag(22);
                self.node(&m.receiver)?;
                self.w.s(&m.name);
                self.w.n(m.args.len());
                for a in &m.args {
                    self.w.opt_s(a.name.as_deref());
                    self.node(&a.value)?;
                    self.opt_place(a.place.as_ref())?;
                }
                self.opt_place(m.place.as_ref())?;
                self.w.b(m.positional);
            }
            Node::Index(t, i) => {
                self.w.tag(23);
                self.node(t)?;
                self.node(i)?;
            }
            Node::Field(t, name, _) => {
                self.w.tag(24);
                self.node(t)?;
                self.w.s(name);
            }
            Node::SafeField(t, name) => {
                self.w.tag(25);
                self.node(t)?;
                self.w.s(name);
            }
            Node::Fault(m) => {
                self.w.tag(26);
                self.w.s(m);
            }
        }
        Ok(())
    }
}

fn put_fields(w: &mut W, fields: &[FieldDef]) {
    w.list(fields, |w, f| {
        w.s(&f.name);
        put_ty(w, &f.ty);
        w.b(f.public);
        w.b(f.var);
        w.b(f.has_default);
    });
}

fn put_program(w: &mut W, program: &Program) {
    w.list(&program.modules, |w, m| {
        w.s(&m.name);
        w.b(m.stdlib);
        w.b(m.prelude);
        w.b(m.report);
    });
    w.list(&program.params, |w, p| {
        w.s(&p.name);
        w.list(&p.bounds, put_ty);
    });
    w.list(&program.defs, |w, d| {
        w.n(d.module);
        w.s(&d.name);
        w.b(d.public);
        w.numbers(&d.generics);
        match &d.kind {
            DefKind::Struct(fs) => {
                w.tag(0);
                put_fields(w, fs);
            }
            DefKind::Error(fs) => {
                w.tag(1);
                put_fields(w, fs);
            }
            DefKind::Enum(vs) => {
                w.tag(2);
                w.list(vs, |w, v| {
                    w.s(&v.name);
                    w.list(&v.fields, |w, (n, t)| {
                        w.s(n);
                        put_ty(w, t);
                    });
                });
            }
            DefKind::Interface(fs) => {
                w.tag(3);
                w.numbers(fs);
            }
        }
    });
    w.list(&program.funcs, |w, f| {
        w.n(f.module);
        w.s(&f.name);
        w.b(f.public);
        w.numbers(&f.generics);
        w.list(&f.params, |w, p| {
            w.s(&p.name);
            put_ty(w, &p.ty);
            w.b(p.var);
            w.b(p.named_only);
            w.b(p.has_default);
        });
        match f.self_param {
            None => w.tag(0),
            Some(false) => w.tag(1),
            Some(true) => w.tag(2),
        }
        put_ty(w, &f.ret);
        w.list(&f.fails, put_ty);
        w.b(f.body.is_some());
        match f.owner {
            Owner::Module => w.tag(0),
            Owner::Impl(i) => {
                w.tag(1);
                w.n(i);
            }
            Owner::Interface(d) => {
                w.tag(2);
                w.n(d);
            }
        }
    });
    w.list(&program.consts, |w, c| {
        w.n(c.module);
        w.s(&c.name);
        w.b(c.public);
        match &c.annotated {
            Some(t) => {
                w.b(true);
                put_ty(w, t);
            }
            None => w.b(false),
        }
    });
    w.list(&program.impls, |w, i| {
        w.n(i.module);
        w.numbers(&i.generics);
        put_ty(w, &i.target);
        match &i.interface {
            Some(t) => {
                w.b(true);
                put_ty(w, t);
            }
            None => w.b(false),
        }
        w.numbers(&i.methods);
    });
    // The diagnostic metadata: the file of each module and the line of each function.
    let paths: Vec<String> = (0..program.modules.len())
        .map(|m| program.trace_path(m))
        .collect();
    w.texts(&paths);
    w.list(&program.funcs, |w, f| w.n(f.span.start.line));
}

/// The image of a checked and lowered program with its units (lambdas last, as [`super::units::collect`]
/// lists them) and the permissions its manifest declares.
pub(crate) fn encode(
    interp: &Interp,
    units: &[Unit],
    permissions: Option<&Declared>,
    toolchain: &str,
) -> Result<Vec<u8>, String> {
    let mut w = W { out: Vec::new() };
    w.out.extend_from_slice(MAGIC);
    w.out.extend_from_slice(&FORMAT.to_le_bytes());
    w.s(&compiler());
    w.s(toolchain);
    match permissions {
        Some(p) => {
            w.b(true);
            w.texts(&p.read);
            w.texts(&p.write);
            w.texts(&p.env);
            w.texts(&p.process);
        }
        None => w.b(false),
    }
    let program = &interp.program;
    let main = program.modules.iter().position(|m| m.report).unwrap_or(0);
    put_program(&mut w, program);
    w.n(main);
    let primary: Vec<&Unit> = units.iter().filter(|u| u.key != Key::Lambda).collect();
    w.n(primary.len());
    for unit in primary {
        w.n(unit.module);
        let mut c = Put { w: &mut w };
        match (&unit.key, &unit.body) {
            (Key::Func(f), Body::Func(_, code)) => {
                c.w.tag(0);
                c.w.n(*f);
                c.w.n(code.slots);
                c.w.b(code.defaults);
                c.w.n(code.params.len());
                for p in &code.params {
                    c.w.b(p.var);
                    c.opt_node(p.default.as_ref())?;
                }
                c.block(&code.body)?;
            }
            (key, Body::Expr(code)) => {
                match *key {
                    Key::Const(k) => {
                        c.w.tag(1);
                        c.w.n(k);
                    }
                    Key::Field(d, i) => {
                        c.w.tag(2);
                        c.w.n(d);
                        c.w.n(i);
                    }
                    Key::ExternDefault(f, i) => {
                        c.w.tag(3);
                        c.w.n(f);
                        c.w.n(i);
                    }
                    _ => return Err("a unit of an unknown kind".into()),
                }
                c.w.n(code.slots);
                c.node(&code.body)?;
            }
            _ => return Err("a unit of an unknown kind".into()),
        }
    }
    Ok(w.out)
}

// ---------------------------------------------------------------- reading

/// A reader of an image; every read checks the bounds, so a damaged image is refused and
/// never read out of its bytes.
pub(crate) struct R<'a> {
    data: &'a [u8],
    at: usize,
}

type Res<T> = Result<T, Refusal>;

fn damaged<T>(what: &str) -> Res<T> {
    Err(Refusal::Damaged(what.to_string()))
}

impl<'a> R<'a> {
    fn byte(&mut self) -> Res<u8> {
        let b = *self
            .data
            .get(self.at)
            .ok_or_else(|| Refusal::Damaged("it ends early".into()))?;
        self.at += 1;
        Ok(b)
    }
    fn u(&mut self) -> Res<u64> {
        let mut n: u64 = 0;
        for shift in (0..64).step_by(7) {
            let b = self.byte()?;
            n |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(n);
            }
        }
        damaged("a number is too long")
    }
    fn n(&mut self) -> Res<usize> {
        usize::try_from(self.u()?).or_else(|_| damaged("a number is too large"))
    }
    /// A count of items that each take at least one byte: never more than the bytes left.
    fn count(&mut self) -> Res<usize> {
        let n = self.n()?;
        if n > self.data.len() - self.at {
            return damaged("a list is longer than the image");
        }
        Ok(n)
    }
    fn index(&mut self, len: usize, what: &str) -> Res<usize> {
        let n = self.n()?;
        if n >= len {
            return damaged(what);
        }
        Ok(n)
    }
    fn slot(&mut self) -> Res<u32> {
        u32::try_from(self.u()?).or_else(|_| damaged("a slot is too large"))
    }
    fn i(&mut self) -> Res<i64> {
        let u = self.u()?;
        Ok(((u >> 1) as i64) ^ -((u & 1) as i64))
    }
    fn b(&mut self) -> Res<bool> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => damaged("a flag is neither 0 nor 1"),
        }
    }
    fn raw(&mut self, len: usize) -> Res<&'a [u8]> {
        let end = self
            .at
            .checked_add(len)
            .filter(|e| *e <= self.data.len())
            .ok_or_else(|| Refusal::Damaged("a text is longer than the image".into()))?;
        let bytes = &self.data[self.at..end];
        self.at = end;
        Ok(bytes)
    }
    fn s(&mut self) -> Res<String> {
        let len = self.n()?;
        let bytes = self.raw(len)?;
        String::from_utf8(bytes.to_vec()).or_else(|_| damaged("a text is not UTF-8"))
    }
    fn opt_s(&mut self) -> Res<Option<String>> {
        Ok(if self.b()? { Some(self.s()?) } else { None })
    }
    fn list<T>(&mut self, mut each: impl FnMut(&mut R<'a>) -> Res<T>) -> Res<Vec<T>> {
        let n = self.count()?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(each(self)?);
        }
        Ok(out)
    }
    fn texts(&mut self) -> Res<Vec<String>> {
        self.list(|r| r.s())
    }
    fn numbers(&mut self) -> Res<Vec<usize>> {
        self.list(|r| r.n())
    }
}

fn get_ty(r: &mut R) -> Res<Ty> {
    Ok(match r.byte()? {
        0 => Ty::Unknown,
        1 => Ty::Never,
        2 => Ty::Unit,
        3 => Ty::Prim(match r.byte()? {
            0 => Prim::Int,
            1 => Prim::Float,
            2 => Prim::Bool,
            3 => Prim::Text,
            4 => Prim::Bytes,
            5 => Prim::Path,
            _ => return damaged("a type of an unknown kind"),
        }),
        4 => Ty::List(Box::new(get_ty(r)?)),
        5 => Ty::Map(Box::new(get_ty(r)?), Box::new(get_ty(r)?)),
        6 => Ty::Set(Box::new(get_ty(r)?)),
        7 => Ty::Tuple(r.list(get_ty)?),
        8 => {
            let params = r.list(|r| Ok((r.opt_s()?, get_ty(r)?)))?;
            let ret = get_ty(r)?;
            let fails = r.list(get_ty)?;
            Ty::Func(Box::new(FnTy { params, ret, fails }))
        }
        9 => Ty::Named(r.n()?, r.list(get_ty)?),
        10 => Ty::Param(r.n()?),
        11 => Ty::Var(r.n()?),
        12 => Ty::SelfTy,
        _ => return damaged("a type of an unknown kind"),
    })
}

fn get_value(r: &mut R) -> Res<Value> {
    Ok(match r.byte()? {
        0 => Value::Unit,
        1 => Value::Int(r.i()?),
        2 => Value::Float(f64::from_bits(r.u()?)),
        3 => Value::Bool(r.b()?),
        4 => Value::text(r.s()?),
        5 => Value::path(r.s()?),
        6 => {
            let len = r.n()?;
            Value::bytes(r.raw(len)?.to_vec())
        }
        7 => Value::list(r.list(get_value)?),
        8 => Value::tuple(r.list(get_value)?),
        _ => return damaged("a value of an unknown kind"),
    })
}

fn get_fields(r: &mut R) -> Res<Vec<FieldDef>> {
    r.list(|r| {
        Ok(FieldDef {
            name: r.s()?,
            ty: get_ty(r)?,
            public: r.b()?,
            var: r.b()?,
            has_default: r.b()?,
        })
    })
}

fn span(line: usize) -> crate::diag::Span {
    let at = crate::diag::Pos {
        offset: 0,
        line,
        column: 1,
    };
    crate::diag::Span::new(at, at)
}

/// The declarations of an image, read before the interpreter that the lowered code needs.
pub(crate) struct Opened<'a> {
    pub program: Program,
    pub main: usize,
    pub permissions: Option<Declared>,
    pub rest: R<'a>,
}

/// What an image records of the build that made it (C-4): the identity of the compiler and that
/// of the toolchain that linked the executable.
#[derive(Debug, PartialEq, Eq)]
pub struct Made {
    pub compiler: String,
    pub toolchain: String,
}

/// Reads the header of an image of this format: the mark, the format, and the identities.
fn header(data: &[u8]) -> Res<(Made, R<'_>)> {
    if data.get(..MAGIC.len()) != Some(&MAGIC[..]) {
        return damaged("it does not start with the mark of a Unio program");
    }
    let mut r = R {
        data,
        at: MAGIC.len(),
    };
    let format = r.raw(4)?;
    let format = u32::from_le_bytes([format[0], format[1], format[2], format[3]]);
    if format != FORMAT {
        return Err(Refusal::Format(format));
    }
    let compiler = r.s().map_err(|_| Refusal::Compiler("unknown".into()))?;
    let toolchain = r.s()?;
    Ok((
        Made {
            compiler,
            toolchain,
        },
        r,
    ))
}

/// What the image `data` records of its build; an image of another format is refused.
pub fn made_by(data: &[u8]) -> Result<Made, Refusal> {
    header(data).map(|(made, _)| made)
}

/// What the program image inside an executable made by `unio build` records of its build: the
/// first place in `bytes` that holds the header of an image of this format. The mark alone
/// also occurs in the code of the runtime that compares it, which is passed over.
pub fn made_by_executable(bytes: &[u8]) -> Option<Made> {
    (0..bytes.len().saturating_sub(MAGIC.len()))
        .filter(|&i| bytes[i..].starts_with(MAGIC))
        .find_map(|i| made_by(&bytes[i..]).ok())
}

/// Checks the header of an image and reads its declarations.
pub(crate) fn open(data: &[u8]) -> Res<Opened<'_>> {
    let (made, mut r) = header(data)?;
    if made.compiler != compiler() {
        return Err(Refusal::Compiler(made.compiler));
    }
    let permissions = if r.b()? {
        Some(Declared {
            read: r.texts()?,
            write: r.texts()?,
            env: r.texts()?,
            process: r.texts()?,
        })
    } else {
        None
    };
    let modules = r.list(|r| Ok((r.s()?, r.b()?, r.b()?, r.b()?)))?;
    let m = modules.len();
    let params = r.list(|r| {
        Ok(ParamInfo {
            name: r.s()?,
            bounds: r.list(get_ty)?,
        })
    })?;
    let defs = r.list(|r| {
        Ok(Def {
            module: r.index(m, "a type of no module")?,
            name: r.s()?,
            public: r.b()?,
            span: Default::default(),
            generics: r.numbers()?,
            kind: match r.byte()? {
                0 => DefKind::Struct(get_fields(r)?),
                1 => DefKind::Error(get_fields(r)?),
                2 => DefKind::Enum(r.list(|r| {
                    Ok(VariantDef {
                        name: r.s()?,
                        fields: r.list(|r| Ok((r.s()?, get_ty(r)?)))?,
                    })
                })?),
                3 => DefKind::Interface(r.numbers()?),
                _ => return damaged("a type of an unknown kind"),
            },
        })
    })?;
    let mut funcs = r.list(|r| {
        let module = r.index(m, "a function of no module")?;
        Ok(FnDef {
            module,
            name: r.s()?,
            public: r.b()?,
            generics: r.numbers()?,
            params: r.list(|r| {
                Ok(ParamDef {
                    name: r.s()?,
                    ty: get_ty(r)?,
                    var: r.b()?,
                    named_only: r.b()?,
                    has_default: r.b()?,
                    span: Default::default(),
                })
            })?,
            self_param: match r.byte()? {
                0 => None,
                1 => Some(false),
                2 => Some(true),
                _ => return damaged("a parameter of an unknown kind"),
            },
            ret: get_ty(r)?,
            fails: r.list(get_ty)?,
            body: r.b()?.then_some(BodyRef {
                module,
                item: 0,
                member: None,
            }),
            owner: match r.byte()? {
                0 => Owner::Module,
                1 => Owner::Impl(r.n()?),
                2 => Owner::Interface(r.n()?),
                _ => return damaged("a function of an unknown owner"),
            },
            span: Default::default(),
        })
    })?;
    let consts = r.list(|r| {
        Ok(ConstDef {
            module: r.index(m, "a constant of no module")?,
            name: r.s()?,
            public: r.b()?,
            annotated: if r.b()? { Some(get_ty(r)?) } else { None },
            item: 0,
            span: Default::default(),
        })
    })?;
    let impls = r.list(|r| {
        Ok(Impl {
            module: r.index(m, "an implementation of no module")?,
            generics: r.numbers()?,
            target: get_ty(r)?,
            interface: if r.b()? { Some(get_ty(r)?) } else { None },
            methods: r.numbers()?,
            span: Default::default(),
        })
    })?;
    let paths = r.texts()?;
    let lines = r.numbers()?;
    if paths.len() != m || lines.len() != funcs.len() {
        return damaged("the diagnostic metadata does not match the declarations");
    }
    for (f, line) in funcs.iter_mut().zip(lines) {
        f.span = span(line);
    }
    let main = r.index(m, "no main module")?;
    // Every number that refers to a declaration must name one, so that the interpreter never
    // indexes outside its tables.
    let (d, f, i, p) = (defs.len(), funcs.len(), impls.len(), params.len());
    let ty_ok = |t: &Ty| ty_in_range(t, d, p);
    let ok = defs.iter().all(|x| {
        x.generics.iter().all(|g| *g < p)
            && match &x.kind {
                DefKind::Struct(fs) | DefKind::Error(fs) => fs.iter().all(|f| ty_ok(&f.ty)),
                DefKind::Enum(vs) => vs.iter().all(|v| v.fields.iter().all(|(_, t)| ty_ok(t))),
                DefKind::Interface(fs) => fs.iter().all(|x| *x < f),
            }
    }) && funcs.iter().all(|x| {
        x.generics.iter().all(|g| *g < p)
            && x.params.iter().all(|q| ty_ok(&q.ty))
            && ty_ok(&x.ret)
            && x.fails.iter().all(ty_ok)
            && match x.owner {
                Owner::Module => true,
                Owner::Impl(j) => j < i,
                Owner::Interface(j) => j < d,
            }
    }) && consts
        .iter()
        .all(|c| c.annotated.as_ref().is_none_or(ty_ok))
        && impls.iter().all(|x| {
            x.generics.iter().all(|g| *g < p)
                && ty_ok(&x.target)
                && x.interface.as_ref().is_none_or(ty_ok)
                && x.methods.iter().all(|j| *j < f)
        })
        && params.iter().all(|q| q.bounds.iter().all(ty_ok));
    if !ok {
        return damaged("a declaration refers to one that does not exist");
    }
    let modules = modules
        .into_iter()
        .zip(paths)
        .map(|((name, stdlib, prelude, report), path)| PreparedModule {
            name,
            path,
            stdlib,
            prelude,
            report,
        })
        .collect();
    Ok(Opened {
        program: Program::prepared(modules, defs, funcs, consts, impls, params),
        main,
        permissions,
        rest: r,
    })
}

fn ty_in_range(t: &Ty, defs: usize, params: usize) -> bool {
    let all = |ts: &[Ty]| ts.iter().all(|t| ty_in_range(t, defs, params));
    match t {
        Ty::List(t) | Ty::Set(t) => ty_in_range(t, defs, params),
        Ty::Map(k, v) => ty_in_range(k, defs, params) && ty_in_range(v, defs, params),
        Ty::Tuple(ts) => all(ts),
        Ty::Func(f) => {
            f.params.iter().all(|(_, t)| ty_in_range(t, defs, params))
                && ty_in_range(&f.ret, defs, params)
                && all(&f.fails)
        }
        Ty::Named(d, args) => *d < defs && all(args),
        Ty::Param(p) => *p < params,
        _ => true,
    }
}

/// Reads lowered code with the names of the interpreter, checking every number that refers to
/// a declaration of the program.
struct Lowered<'i, 'r, 'a> {
    r: &'r mut R<'a>,
    interp: &'i Interp,
}

impl Lowered<'_, '_, '_> {
    fn name(&mut self) -> Res<Rc<str>> {
        let s = self.r.s()?;
        Ok(self.interp.shared(&s))
    }

    fn func(&mut self) -> Res<usize> {
        self.r
            .index(self.interp.program.funcs.len(), "a call of no function")
    }

    fn def(&mut self) -> Res<usize> {
        self.r
            .index(self.interp.program.defs.len(), "a type that does not exist")
    }

    fn constant(&mut self) -> Res<usize> {
        self.r.index(
            self.interp.program.consts.len(),
            "a constant that does not exist",
        )
    }

    fn slots(&mut self) -> Res<Vec<u32>> {
        self.r.list(|r| r.slot())
    }

    fn opt_slot(&mut self) -> Res<Option<u32>> {
        Ok(if self.r.b()? {
            Some(self.r.slot()?)
        } else {
            None
        })
    }

    fn block(&mut self) -> Res<Block> {
        let n = self.r.count()?;
        let mut stmts = Vec::with_capacity(n);
        for _ in 0..n {
            let line = u32::try_from(self.r.u()?).or_else(|_| damaged("a line is too large"))?;
            stmts.push(Stmt {
                line,
                kind: self.s()?,
            });
        }
        Ok(Block {
            stmts,
            clear: (self.r.slot()?, self.r.slot()?),
        })
    }

    fn opt_node(&mut self) -> Res<Option<Node>> {
        Ok(if self.r.b()? {
            Some(self.node()?)
        } else {
            None
        })
    }

    fn nodes(&mut self) -> Res<Vec<Node>> {
        let n = self.r.count()?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(self.node()?);
        }
        Ok(out)
    }

    fn s(&mut self) -> Res<S> {
        Ok(match self.r.byte()? {
            0 => S::Let(self.r.slot()?, self.node()?),
            1 => S::Assign(self.place()?, self.node()?),
            2 => S::SetIndex(self.place()?, self.node()?, self.node()?),
            3 => S::Concat(self.r.slot()?, self.nodes()?, Box::new(self.node()?)),
            4 => S::Return(self.opt_node()?),
            5 => S::Fail(self.node()?),
            6 => S::Assert(self.node()?, self.r.s()?),
            7 => S::Break,
            8 => S::Continue,
            9 => S::Pass,
            10 => S::Expr(self.node()?),
            11 => S::Handled {
                inner: Box::new(self.s()?),
                ty: self.r.s()?,
                bind: self.opt_slot()?,
                block: self.block()?,
            },
            12 => {
                let n = self.r.count()?;
                let mut branches = Vec::with_capacity(n);
                for _ in 0..n {
                    branches.push((self.node()?, self.block()?));
                }
                let otherwise = if self.r.b()? {
                    Some(self.block()?)
                } else {
                    None
                };
                S::If(branches, otherwise)
            }
            13 => S::While(self.node()?, self.block()?),
            14 => S::For(self.slots()?, self.node()?, self.block()?),
            15 => {
                let n = self.node()?;
                let count = self.r.count()?;
                let mut cases = Vec::with_capacity(count);
                for _ in 0..count {
                    cases.push(CaseCode {
                        pattern: self.pat()?,
                        guard: self.opt_node()?,
                        body: self.block()?,
                    });
                }
                S::Match(n, cases)
            }
            16 => {
                let b = self.block()?;
                let count = self.r.count()?;
                let mut handlers = Vec::with_capacity(count);
                for _ in 0..count {
                    handlers.push(HandlerCode {
                        ty: self.r.s()?,
                        bind: self.opt_slot()?,
                        body: self.block()?,
                    });
                }
                S::Try(b, handlers)
            }
            17 => S::Using(self.r.slot()?, self.node()?, self.block()?),
            _ => return damaged("a statement of an unknown kind"),
        })
    }

    fn place(&mut self) -> Res<Place> {
        let root = self.r.slot()?;
        let n = self.r.count()?;
        let mut steps = Vec::with_capacity(n);
        for _ in 0..n {
            steps.push(match self.r.byte()? {
                0 => PStep::Field(self.name()?),
                1 => PStep::Index(Rc::new(self.node()?)),
                _ => return damaged("a place of an unknown kind"),
            });
        }
        Ok(Place { root, steps })
    }

    fn opt_place(&mut self) -> Res<Option<Place>> {
        Ok(if self.r.b()? {
            Some(self.place()?)
        } else {
            None
        })
    }

    fn pats(&mut self) -> Res<Vec<Pat>> {
        let n = self.r.count()?;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(self.pat()?);
        }
        Ok(out)
    }

    fn pat(&mut self) -> Res<Pat> {
        Ok(match self.r.byte()? {
            0 => Pat::Wildcard,
            1 => Pat::Int(if self.r.b()? { Some(self.r.i()?) } else { None }),
            2 => Pat::Bool(self.r.b()?),
            3 => Pat::Text(self.r.s()?),
            4 => Pat::Name {
                last: self.name()?,
                fallback: match self.r.byte()? {
                    0 => NameFallback::Bind(self.r.slot()?),
                    1 => NameFallback::Const(self.constant()?),
                    2 => NameFallback::No,
                    _ => return damaged("a pattern of an unknown kind"),
                },
                known: Cell::new(0),
                pin: RefCell::new(None),
            },
            5 => {
                let last = self.name()?;
                let n = self.r.count()?;
                let mut args = Vec::with_capacity(n);
                for _ in 0..n {
                    args.push((self.r.opt_s()?, self.pat()?));
                }
                Pat::Constructor { last, args }
            }
            6 => {
                let n = self.r.count()?;
                let mut elements = Vec::with_capacity(n);
                for _ in 0..n {
                    elements.push(match self.r.byte()? {
                        0 => ListPat::Pattern(self.pat()?),
                        1 => ListPat::Rest(self.r.slot()?),
                        _ => return damaged("a pattern of an unknown kind"),
                    });
                }
                Pat::List {
                    elements,
                    fixed: self.r.n()?,
                    rest: self.r.b()?,
                }
            }
            7 => Pat::Tuple(self.pats()?),
            8 => Pat::Bind(Box::new(self.pat()?), self.r.slot()?),
            9 => Pat::Or(self.pats()?),
            _ => return damaged("a pattern of an unknown kind"),
        })
    }

    fn type_code(&mut self) -> Res<TypeCode> {
        let generic = self.r.opt_s()?;
        let base = match self.r.byte()? {
            0 => TypeBase::Int,
            1 => TypeBase::Float,
            2 => TypeBase::Bool,
            3 => TypeBase::Text,
            4 => TypeBase::Bytes,
            5 => TypeBase::Path,
            6 => TypeBase::List,
            7 => TypeBase::Map,
            8 => TypeBase::Set,
            9 => TypeBase::Tuple,
            10 => TypeBase::Def(self.def()?),
            11 => TypeBase::Unknown,
            _ => return damaged("a type of an unknown kind"),
        };
        let n = self.r.count()?;
        let mut args = Vec::with_capacity(n);
        for _ in 0..n {
            args.push(self.type_code()?);
        }
        Ok(TypeCode {
            generic,
            base,
            args,
        })
    }

    fn lambda(&mut self) -> Res<LambdaCode> {
        let slots = self.r.n()?;
        let params = self.r.n()?;
        let n = self.r.count()?;
        let mut captures = Vec::with_capacity(n);
        for _ in 0..n {
            captures.push((self.r.slot()?, self.r.slot()?));
        }
        Ok(LambdaCode {
            slots,
            params,
            captures,
            body: self.node()?,
        })
    }

    fn node(&mut self) -> Res<Node> {
        Ok(match self.r.byte()? {
            0 => Node::Value(get_value(self.r)?),
            1 => Node::Local(self.r.slot()?),
            2 => Node::Const(self.constant()?),
            3 => Node::FnRef(self.func()?),
            4 => Node::Variant(self.def()?, self.name()?),
            5 => {
                let n = self.r.count()?;
                let mut parts = Vec::with_capacity(n);
                for _ in 0..n {
                    parts.push(match self.r.byte()? {
                        0 => Part::Literal(self.r.s()?),
                        1 => Part::Value(self.node()?),
                        _ => return damaged("a part of an unknown kind"),
                    });
                }
                Node::Text(parts)
            }
            6 => Node::Tuple(self.nodes()?),
            7 => Node::List(self.nodes()?),
            8 => {
                let n = self.r.count()?;
                let mut entries = Vec::with_capacity(n);
                for _ in 0..n {
                    entries.push((self.node()?, self.node()?));
                }
                Node::Map(entries)
            }
            9 => Node::Comprehension(Box::new(ComprehensionCode {
                element: self.node()?,
                names: self.slots()?,
                iterable: self.node()?,
                condition: self.opt_node()?,
            })),
            10 => Node::Lambda(Rc::new(self.lambda()?)),
            11 => Node::Is(Box::new(self.node()?), Box::new(self.pat()?)),
            12 => Node::Conditional(Box::new([self.node()?, self.node()?, self.node()?])),
            13 => Node::And(Box::new(self.node()?), Box::new(self.node()?)),
            14 => Node::Or(Box::new(self.node()?), Box::new(self.node()?), self.r.b()?),
            15 => Node::Not(Box::new(self.node()?)),
            16 => Node::Negate(Box::new(self.node()?)),
            17 => {
                let op = *OPS
                    .get(self.r.n()?)
                    .ok_or_else(|| Refusal::Damaged("an operator of an unknown kind".into()))?;
                Node::Binary(op, Box::new(self.node()?), Box::new(self.node()?))
            }
            18 => Node::Try(Box::new(self.node()?), self.r.b()?),
            19 => {
                let f = self.func()?;
                let n = self.r.count()?;
                let mut args = Vec::with_capacity(n);
                for _ in 0..n {
                    args.push(ArgCode {
                        target: self.r.n()?,
                        value: self.node()?,
                        place: self.opt_place()?,
                    });
                }
                let n = self.r.count()?;
                let mut types = Vec::with_capacity(n);
                for _ in 0..n {
                    types.push(self.type_code()?);
                }
                let count = self.r.n()?;
                if args.iter().any(|a| a.target >= count) {
                    return damaged("an argument of no parameter");
                }
                Node::Call(Box::new(CallCode {
                    f,
                    args,
                    types,
                    count,
                }))
            }
            20 => Node::CallValue(Box::new(self.node()?), self.nodes()?),
            21 => {
                let def = self.def()?;
                let variant = match self.r.opt_s()? {
                    Some(v) => Some(self.interp.shared(&v)),
                    None => None,
                };
                let n = self.r.count()?;
                let mut args = Vec::with_capacity(n);
                for _ in 0..n {
                    args.push((self.r.n()?, self.node()?));
                }
                let fields = self.r.n()?;
                if args.iter().any(|(i, _)| *i >= fields) {
                    return damaged("an argument of no field");
                }
                Node::Construct(Box::new(ConstructCode {
                    def,
                    variant,
                    args,
                    fields,
                }))
            }
            22 => {
                let receiver = self.node()?;
                let name = self.r.s()?;
                let n = self.r.count()?;
                let mut args = Vec::with_capacity(n);
                for _ in 0..n {
                    args.push(MethodArg {
                        name: self.r.opt_s()?,
                        value: self.node()?,
                        place: self.opt_place()?,
                    });
                }
                Node::Method(Box::new(MethodCode {
                    receiver,
                    name,
                    args,
                    place: self.opt_place()?,
                    cache: Cell::new((0, 0)),
                    pin: RefCell::new(None),
                    positional: self.r.b()?,
                }))
            }
            23 => Node::Index(Box::new(self.node()?), Box::new(self.node()?)),
            24 => Node::Field(Box::new(self.node()?), self.name()?, Cell::new(0)),
            25 => Node::SafeField(Box::new(self.node()?), self.name()?),
            26 => Node::Fault(self.r.s()?),
            _ => return damaged("an expression of an unknown kind"),
        })
    }
}

/// Reads the units of an image after its declarations, and checks that the image ends there.
pub(crate) fn read_units(mut r: R<'_>, interp: &Interp) -> Res<Vec<Unit>> {
    let program = &interp.program;
    let n = r.count()?;
    let mut units = Vec::with_capacity(n);
    for _ in 0..n {
        let module = r.index(program.modules.len(), "a unit of no module")?;
        let mut l = Lowered { r: &mut r, interp };
        let tag = l.r.byte()?;
        let unit = if tag == 0 {
            let f = l.func()?;
            let slots = l.r.n()?;
            let defaults = l.r.b()?;
            let count = l.r.count()?;
            let mut params = Vec::with_capacity(count);
            for _ in 0..count {
                params.push(ParamCode {
                    var: l.r.b()?,
                    default: l.opt_node()?,
                });
            }
            let body = l.block()?;
            Unit {
                module,
                key: Key::Func(f),
                body: Body::Func(
                    f,
                    Rc::new(Code {
                        slots,
                        defaults,
                        params,
                        body,
                    }),
                ),
            }
        } else {
            let key = match tag {
                1 => Key::Const(l.constant()?),
                2 => Key::Field(l.def()?, l.r.n()?),
                3 => Key::ExternDefault(l.func()?, l.r.n()?),
                _ => return damaged("a unit of an unknown kind"),
            };
            let slots = l.r.n()?;
            let body = l.node()?;
            Unit {
                module,
                key,
                body: Body::Expr(Rc::new(ExprCode { slots, body })),
            }
        };
        units.push(unit);
    }
    if r.at != r.data.len() {
        return damaged("bytes follow the last unit");
    }
    Ok(units)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_header(format: u32, compiler: &str, toolchain: &str) -> Vec<u8> {
        let mut w = W { out: Vec::new() };
        w.out.extend_from_slice(MAGIC);
        w.out.extend_from_slice(&format.to_le_bytes());
        w.s(compiler);
        w.s(toolchain);
        w.out
    }

    /// The header records the toolchain after the compiler; an image of format 1, which had no
    /// toolchain, is refused as of another format (code 104).
    #[test]
    fn the_header_records_the_toolchain() {
        let toolchain = "x86_64-unknown-linux-gnu; cc: gcc (GCC) 14.2.0";
        let data = image_header(FORMAT, &compiler(), toolchain);
        assert_eq!(
            made_by(&data),
            Ok(Made {
                compiler: compiler(),
                toolchain: toolchain.into()
            })
        );
        let mut executable = b"code UNIOPRG\0 compared here; data: ".to_vec();
        executable.extend_from_slice(&data);
        executable.extend_from_slice(b"more of the image");
        assert_eq!(
            made_by_executable(&executable).map(|m| m.toolchain),
            Some(toolchain.to_string())
        );
        assert_eq!(made_by_executable(b"no image"), None);
        let old = image_header(1, &compiler(), toolchain);
        assert_eq!(made_by(&old), Err(Refusal::Format(1)));
        assert!(matches!(open(&old), Err(Refusal::Format(1))));
        let cut = &data[..data.len() - 3];
        assert!(matches!(made_by(cut), Err(Refusal::Damaged(_))));
    }
}
