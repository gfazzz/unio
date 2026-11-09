//! Derived effects and layers (docs/semantics.md, section 8): the effects of every function
//! from the call graph (L-9b, C-11), the lowest library layer it needs (S-11, C-21), the
//! check of `unio check --profile=core` (E0084, E0087), and the permissions of the manifest
//! (E0085).
//!
//! The graph has a holder for every function, every constant (its initializer), every struct
//! or error (the defaults of its fields) and every lambda (its body). Its edges are the uses
//! the checks record: calls of named functions, implicit calls through interfaces, uses of
//! constants and field defaults, calls through function values, and an edge from every
//! interface method to each of its implementations (semantics 8.4). Allocating expressions
//! are sources of `allocates` and of the layer `alloc`.
//!
//! Function values follow RFC 0005: making a value has no effect of its body, a call carries
//! them. A call through a parameter is a latent effect of its function, replaced at each call
//! of the function by the values the argument can be. A value whose origin is not known
//! exactly (read from a field, a collection, a pattern, or a parameter that escaped) is any
//! value the program stores there, or any value of its type, and is called with unknown
//! arguments.
//!
//! The tests of a program count only when they run (RFC 0005, "Tests in the analysis"): the
//! checks record the values, calls and stores of the tests that `unio test` selects, each
//! test a holder of its own, and nothing of the other tests, so that `unio check`, `unio run`
//! and the tests of imported modules and of the standard library leave the program as it is.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use crate::catalog;
use crate::diag::{Diagnostic, Span};
use crate::program::{DefKind, FnId, ModId, Owner, Program, LAYER_NAMES};
use crate::symbol::Node;
use crate::types::{DefId, Ty};

/// An expression that allocates by itself, without a call (semantics 8.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Allocation {
    List,
    Map,
    Comprehension,
    Interpolation,
    Concatenation,
    Closure,
}

impl Allocation {
    /// What a diagnostic calls it, in English and in Russian.
    pub fn describe(self) -> (&'static str, &'static str) {
        match self {
            Allocation::List => ("a list literal", "литерал списка"),
            Allocation::Map => ("a map literal", "литерал словаря"),
            Allocation::Comprehension => ("a list comprehension", "генератор списка"),
            Allocation::Interpolation => ("a text with interpolation", "текст с интерполяцией"),
            Allocation::Concatenation => ("joining texts with +", "соединение текстов через +"),
            Allocation::Closure => (
                "a lambda that captures values",
                "лямбда, захватывающая значения",
            ),
        }
    }
}

/// What has effects: a function, the initializer of a constant, the field defaults of a type,
/// the body of a lambda (an index into `Facts::lambdas`), or the body of a test that runs (an
/// index into `Facts::tests`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Holder {
    Func(FnId),
    Const(usize),
    Def(DefId),
    Lambda(usize),
    Test(usize),
}

impl From<Node> for Holder {
    fn from(n: Node) -> Holder {
        match n {
            Node::Func(f) => Holder::Func(f),
            Node::Const(c) => Holder::Const(c),
            Node::Def(d) => Holder::Def(d),
        }
    }
}

/// Where a function value can come from (RFC 0005). A flow is the union of its atoms.
#[derive(Clone, Debug, PartialEq)]
pub enum Atom {
    /// A named function used as a value.
    Fn(FnId),
    /// A lambda, an index into `Facts::lambdas`.
    Lambda(usize),
    /// The value of a parameter of a function or a lambda.
    Param(Holder, usize),
    /// A local name, an index into `Facts::locals`.
    Local(usize),
    Const(usize),
    /// What is stored in a field of a struct or error anywhere in the program.
    Field(DefId, String),
    /// What a named function returns.
    Ret(FnId),
    /// Not known: any function value of the program of a type that may be this one.
    Top(Ty),
}

pub type Flow = Vec<Atom>;

/// What a use reaches: a holder by name, or the values a function value can be.
pub enum Callee {
    Holder(Holder),
    Value(Flow),
}

/// A use at a place: `from` needs what the callee needs. `args` are the flows of the
/// arguments that may be function values, by the index of the parameter.
pub struct Use {
    pub from: Holder,
    pub callee: Callee,
    pub args: Vec<(usize, Flow)>,
    pub module: ModId,
    pub span: Span,
}

/// An allocating expression inside a function, a lambda, a constant or a field default.
pub struct Site {
    pub at: Holder,
    pub what: Allocation,
    pub module: ModId,
    pub span: Span,
}

/// A lambda: the holder that makes it, its parameters and its type.
pub struct LambdaFact {
    pub parent: Option<Holder>,
    pub module: ModId,
    pub span: Span,
    pub params: Vec<(String, Ty)>,
    pub ty: Ty,
}

/// A test block that runs: its module, its place and its description.
pub struct TestFact {
    pub module: ModId,
    pub span: Span,
    pub name: String,
}

/// What the checks find for the effects.
#[derive(Default)]
pub struct Facts {
    pub uses: Vec<Use>,
    /// The tests that run, each a holder (RFC 0005, tests in the analysis).
    pub tests: Vec<TestFact>,
    pub sites: Vec<Site>,
    pub lambdas: Vec<LambdaFact>,
    /// Named functions used as values.
    pub named: Vec<FnId>,
    /// The values assigned to each local name.
    pub locals: Vec<Flow>,
    pub consts: HashMap<usize, Flow>,
    /// The values stored in each field: construction, field defaults and assignments.
    pub stores: HashMap<(DefId, String), Flow>,
    pub returns: HashMap<FnId, Flow>,
    /// The defaults of parameters that may hold function values.
    pub defaults: HashMap<(FnId, usize), Flow>,
}

/// How much the checks had recorded, to forget what a test that does not run adds.
pub struct Mark {
    uses: usize,
    sites: usize,
    lambdas: usize,
    named: usize,
    locals: usize,
    stores: HashMap<(DefId, String), usize>,
}

impl Facts {
    pub fn mark(&self) -> Mark {
        Mark {
            uses: self.uses.len(),
            sites: self.sites.len(),
            lambdas: self.lambdas.len(),
            named: self.named.len(),
            locals: self.locals.len(),
            stores: self
                .stores
                .iter()
                .map(|(k, v)| (k.clone(), v.len()))
                .collect(),
        }
    }

    /// Forgets what was recorded after the mark: the values, calls and stores of a test that
    /// does not run are not possible values of the program (RFC 0005). A test has no function,
    /// constant or parameter default of its own, so nothing else can have grown.
    pub fn rewind(&mut self, mark: Mark) {
        self.uses.truncate(mark.uses);
        self.sites.truncate(mark.sites);
        self.lambdas.truncate(mark.lambdas);
        self.named.truncate(mark.named);
        self.locals.truncate(mark.locals);
        self.stores.retain(|k, v| match mark.stores.get(k) {
            Some(&n) => {
                v.truncate(n);
                true
            }
            None => false,
        });
    }
}

/// Whether a value of the type may be a function value, so that its flow matters.
pub fn may_hold_function(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Func(_) | Ty::Param(_) | Ty::Unknown | Ty::Var(_) | Ty::SelfTy
    )
}

/// A value whose set of possible function values is not known exactly (RFC 0005).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Through {
    Const(usize),
    /// An index into the fields of `Derived`.
    Field(usize),
    /// An index into the types of unknown values of `Derived`.
    Top(usize),
    /// The values passed as a parameter of a holder that keeps or passes it on.
    Escaped(usize, usize),
    Ret(FnId),
}

/// Why a holder has an effect or a layer.
#[derive(Clone, Copy, Debug)]
pub enum Via {
    /// A holder it uses by name.
    Node(usize, Span),
    /// An allocating expression of its own.
    Site(usize),
    /// A call of a function value `maker`: passed as argument `arg` of a call, reached
    /// through `through`, and then with its own effects or, when `closed`, with the effects
    /// it has for unknown arguments.
    Value {
        maker: usize,
        span: Span,
        arg: Option<(usize, usize)>,
        through: Option<Through>,
        closed: bool,
    },
}

/// One step of a chain: a holder, an allocating expression, the argument of a call, or the
/// place an unknown value comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Node(usize),
    Site(usize),
    Arg(usize, usize),
    Through(Through),
}

/// The derived effects, in the order `unio symbol` prints them. `files.read` and `files.write`
/// are the scopes of the manifest of the same names (T-6).
pub const EFFECTS: [&str; 6] = [
    "files.read",
    "files.write",
    "network",
    "process",
    "env",
    "allocates",
];
const FILES_READ: usize = 0;
const FILES_WRITE: usize = 1;

/// A set of function values: the holders of named functions and lambdas.
type Set = BTreeSet<usize>;

/// What each holder needs, and for each need the use that brings it in, so that a diagnostic
/// can show the chain of calls down to the function or expression that has it itself.
/// Holders are numbered functions first, so `effects[f]` is the function `f`; then constants,
/// then the field defaults of each declaration, then lambdas, then the tests that run.
pub struct Derived {
    /// A bit per entry of EFFECTS: what a call of the holder needs, besides its latent effects.
    pub effects: Vec<u8>,
    pub effect_via: Vec<[Option<Via>; EFFECTS.len()]>,
    /// The index in LAYER_NAMES of the lowest layer the holder needs.
    pub layer: Vec<usize>,
    pub layer_via: Vec<Option<Via>>,
    /// The parameters whose values a call of the holder may call, as (holder, index): its
    /// own, and for a lambda those of the functions around it (RFC 0005).
    pub latent: Vec<BTreeSet<(usize, usize)>>,
    /// What a call with arguments that are not known needs: the latent effects replaced by
    /// every value the parameters can get.
    closed_effects: Vec<u8>,
    closed_via: Vec<[Option<Via>; EFFECTS.len()]>,
    closed_layer: Vec<usize>,
    closed_layer_via: Vec<Option<Via>>,
    functions: usize,
    constants: usize,
    defs: usize,
    lambdas: usize,
    /// The holder around each lambda.
    parents: Vec<Option<usize>>,
    // The values that may reach each place (RFC 0005).
    locals: Vec<Set>,
    consts: HashMap<usize, Set>,
    fields: Vec<Set>,
    field_ids: HashMap<(DefId, String), usize>,
    field_keys: Vec<(DefId, String)>,
    returns: HashMap<FnId, Set>,
    escaped: HashMap<(usize, usize), Set>,
    tops: Vec<Set>,
    top_ids: HashMap<String, usize>,
    top_types: Vec<Ty>,
    /// Every function value of the program with its type.
    makers: Vec<(usize, Ty)>,
    /// The locals, constants and returns whose values are only estimated: they come through
    /// a parameter, a field, a collection or a pattern.
    inexact_locals: Vec<bool>,
    inexact_consts: HashSet<usize>,
    inexact_returns: HashSet<FnId>,
}

/// Whether a value of type `a` may be seen where the type is `b`: generic parameters, unknown
/// types and interfaces match anything; results and errors are not compared.
fn may_match(p: &Program, a: &Ty, b: &Ty) -> bool {
    let open = |t: &Ty| match t {
        Ty::Unknown | Ty::Never | Ty::Var(_) | Ty::Param(_) | Ty::SelfTy => true,
        Ty::Named(d, _) => matches!(p.defs[*d].kind, DefKind::Interface(_)),
        _ => false,
    };
    if open(a) || open(b) {
        return true;
    }
    match (a, b) {
        (Ty::Prim(x), Ty::Prim(y)) => x == y,
        (Ty::Unit, Ty::Unit) => true,
        (Ty::List(x), Ty::List(y)) | (Ty::Set(x), Ty::Set(y)) => may_match(p, x, y),
        (Ty::Map(k1, v1), Ty::Map(k2, v2)) => may_match(p, k1, k2) && may_match(p, v1, v2),
        (Ty::Tuple(xs), Ty::Tuple(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| may_match(p, x, y))
        }
        (Ty::Named(d1, _), Ty::Named(d2, _)) => d1 == d2,
        (Ty::Func(f), Ty::Func(g)) => {
            f.params.len() == g.params.len()
                && f.params
                    .iter()
                    .zip(&g.params)
                    .all(|((_, x), (_, y))| may_match(p, x, y))
        }
        _ => false,
    }
}

/// The type of a named function used as a value.
fn function_type(p: &Program, f: FnId) -> Ty {
    let def = &p.funcs[f];
    Ty::Func(Box::new(crate::types::FnTy {
        params: def
            .params
            .iter()
            .map(|q| (q.named_only.then(|| q.name.clone()), q.ty.clone()))
            .collect(),
        ret: def.ret.clone(),
        fails: def.fails.clone(),
    }))
}

/// Whether the values a call of `f` returns are known from its `return` statements.
fn returns_known(p: &Program, f: FnId) -> bool {
    let def = &p.funcs[f];
    def.body.is_some()
        && !matches!(def.owner, Owner::Interface(_))
        && matches!(def.ret, Ty::Func(_))
}

/// What one use or one call of a value needs, with the reasons.
#[derive(Default)]
struct Acc {
    bits: u8,
    via: [Option<Via>; EFFECTS.len()],
    layer: usize,
    layer_via: Option<Via>,
    latent: BTreeSet<(usize, usize)>,
}

impl Acc {
    fn add(&mut self, bits: u8, layer: usize, via: Via) {
        let new = bits & !self.bits;
        for e in 0..EFFECTS.len() {
            if new & (1 << e) != 0 {
                self.via[e] = Some(via);
            }
        }
        self.bits |= bits;
        if layer > self.layer {
            self.layer = layer;
            self.layer_via = Some(via);
        }
    }
}

impl Derived {
    fn idx(&self, h: Holder) -> usize {
        match h {
            Holder::Func(f) => f,
            Holder::Const(c) => self.functions + c,
            Holder::Def(d) => self.functions + self.constants + d,
            Holder::Lambda(l) => self.functions + self.constants + self.defs + l,
            Holder::Test(t) => self.functions + self.constants + self.defs + self.lambdas + t,
        }
    }

    fn holder(&self, i: usize) -> Holder {
        let (f, c, d) = (self.functions, self.constants, self.defs);
        if i < f {
            Holder::Func(i)
        } else if i < f + c {
            Holder::Const(i - f)
        } else if i < f + c + d {
            Holder::Def(i - f - c)
        } else if i < f + c + d + self.lambdas {
            Holder::Lambda(i - f - c - d)
        } else {
            Holder::Test(i - f - c - d - self.lambdas)
        }
    }

    /// Whether code of holder `ctx` runs inside holder `scope`: it is the scope or a lambda in it.
    fn within(&self, ctx: usize, scope: usize) -> bool {
        let mut at = Some(ctx);
        while let Some(x) = at {
            if x == scope {
                return true;
            }
            at = self.parents.get(x).copied().flatten();
        }
        false
    }

    /// The holder that is not a lambda around `h`, or None for a test or a lambda in one.
    fn root(&self, h: usize) -> Option<usize> {
        let mut at = h;
        while matches!(self.holder(at), Holder::Lambda(_)) {
            at = self.parents[at]?;
        }
        (!matches!(self.holder(at), Holder::Test(_))).then_some(at)
    }

    fn top(&mut self, p: &Program, t: &Ty) -> usize {
        let key = format!("{t:?}");
        if let Some(&id) = self.top_ids.get(&key) {
            return id;
        }
        let set: Set = self
            .makers
            .iter()
            .filter(|(_, ty)| may_match(p, ty, t))
            .map(|(m, _)| *m)
            .collect();
        self.tops.push(set);
        self.top_types.push(t.clone());
        self.top_ids.insert(key, self.tops.len() - 1);
        self.tops.len() - 1
    }

    fn field(&mut self, d: DefId, name: &str) -> usize {
        let key = (d, name.to_string());
        if let Some(&id) = self.field_ids.get(&key) {
            return id;
        }
        self.fields.push(Set::new());
        self.field_keys.push(key.clone());
        self.field_ids.insert(key, self.fields.len() - 1);
        self.fields.len() - 1
    }

    /// The values an atom may be, where it is not known in which call it is evaluated.
    fn closed(&mut self, p: &Program, a: &Atom) -> (Option<Through>, Set) {
        match a {
            Atom::Fn(f) => (None, Set::from([*f])),
            Atom::Lambda(l) => (None, Set::from([self.idx(Holder::Lambda(*l))])),
            Atom::Param(h, i) => {
                let s = self.idx(*h);
                let set = self.escaped.get(&(s, *i)).cloned().unwrap_or_default();
                (Some(Through::Escaped(s, *i)), set)
            }
            Atom::Local(id) => (None, self.locals[*id].clone()),
            Atom::Const(c) => (
                Some(Through::Const(*c)),
                self.consts.get(c).cloned().unwrap_or_default(),
            ),
            Atom::Field(d, name) => {
                let id = self.field(*d, name);
                (Some(Through::Field(id)), self.fields[id].clone())
            }
            Atom::Ret(f) if returns_known(p, *f) => (
                Some(Through::Ret(*f)),
                self.returns.get(f).cloned().unwrap_or_default(),
            ),
            Atom::Ret(f) => {
                let id = self.top(p, &p.funcs[*f].ret);
                (Some(Through::Top(id)), self.tops[id].clone())
            }
            Atom::Top(t) => {
                let id = self.top(p, t);
                (Some(Through::Top(id)), self.tops[id].clone())
            }
        }
    }

    /// Whether the values of an atom are only estimated (RFC 0005).
    fn inexact(&self, p: &Program, a: &Atom) -> bool {
        match a {
            Atom::Fn(_) | Atom::Lambda(_) => false,
            Atom::Local(id) => self.inexact_locals[*id],
            Atom::Const(c) => self.inexact_consts.contains(c),
            Atom::Ret(f) => !returns_known(p, *f) || self.inexact_returns.contains(f),
            Atom::Param(..) | Atom::Field(..) | Atom::Top(_) => true,
        }
    }

    /// Whether a place a value comes through gives only an estimate of its values.
    fn uncertain(&self, t: Through) -> bool {
        match t {
            Through::Const(c) => self.inexact_consts.contains(&c),
            Through::Ret(f) => self.inexact_returns.contains(&f),
            Through::Field(_) | Through::Top(_) | Through::Escaped(..) => true,
        }
    }

    fn closed_flow(&mut self, p: &Program, flow: &[Atom]) -> Set {
        let mut out = Set::new();
        for a in flow {
            out.extend(self.closed(p, a).1);
        }
        out
    }

    /// The same lookup after the sets are complete: every place has been seen by then.
    fn closed_ref(&self, p: &Program, a: &Atom) -> (Option<Through>, Option<&Set>) {
        match a {
            Atom::Param(h, i) => {
                let s = self.idx(*h);
                (Some(Through::Escaped(s, *i)), self.escaped.get(&(s, *i)))
            }
            Atom::Const(c) => (Some(Through::Const(*c)), self.consts.get(c)),
            Atom::Field(d, name) => match self.field_ids.get(&(*d, name.clone())) {
                Some(&id) => (Some(Through::Field(id)), Some(&self.fields[id])),
                None => (None, None),
            },
            Atom::Ret(f) if returns_known(p, *f) => (Some(Through::Ret(*f)), self.returns.get(f)),
            Atom::Ret(f) => self.top_ref(&p.funcs[*f].ret),
            Atom::Top(t) => self.top_ref(t),
            Atom::Fn(_) | Atom::Lambda(_) | Atom::Local(_) => (None, None),
        }
    }

    fn top_ref(&self, t: &Ty) -> (Option<Through>, Option<&Set>) {
        match self.top_ids.get(&format!("{t:?}")) {
            Some(&id) => (Some(Through::Top(id)), Some(&self.tops[id])),
            None => (None, None),
        }
    }

    /// Calls every value of a set with arguments that are not known.
    fn closed_calls(
        &self,
        set: Option<&Set>,
        through: Through,
        span: Span,
        arg: Option<(usize, usize)>,
        acc: &mut Acc,
    ) {
        for &m in set.into_iter().flatten() {
            let via = Via::Value {
                maker: m,
                span,
                arg,
                through: Some(through),
                closed: true,
            };
            acc.add(self.closed_effects[m], self.closed_layer[m], via);
        }
    }

    /// A latent effect (scope, i) of a call made in `ctx`: still latent when the call runs
    /// inside the scope, replaced by the argument when it is given, else by every value.
    #[allow(clippy::too_many_arguments)]
    fn latent_of(
        &self,
        p: &Program,
        facts: &Facts,
        callee: usize,
        (scope, i): (usize, usize),
        ctx: usize,
        args: &[(usize, Flow)],
        span: Span,
        acc: &mut Acc,
        stack: &mut Vec<usize>,
    ) {
        if scope == callee {
            let given = args.iter().find(|(j, _)| *j == i).map(|(_, f)| f);
            let default = if callee < self.functions {
                facts.defaults.get(&(callee, i))
            } else {
                None
            };
            match given.or(default) {
                Some(flow) => self.eval(
                    p,
                    facts,
                    flow,
                    ctx,
                    &[],
                    Some((callee, i)),
                    span,
                    acc,
                    stack,
                ),
                None => self.closed_calls(
                    self.escaped.get(&(scope, i)),
                    Through::Escaped(scope, i),
                    span,
                    Some((callee, i)),
                    acc,
                ),
            }
        } else if self.within(ctx, scope) {
            acc.latent.insert((scope, i));
        } else {
            self.closed_calls(
                self.escaped.get(&(scope, i)),
                Through::Escaped(scope, i),
                span,
                None,
                acc,
            );
        }
    }

    /// A call, made in holder `ctx`, of the values of a flow with the given arguments.
    #[allow(clippy::too_many_arguments)]
    fn eval(
        &self,
        p: &Program,
        facts: &Facts,
        flow: &[Atom],
        ctx: usize,
        args: &[(usize, Flow)],
        arg: Option<(usize, usize)>,
        span: Span,
        acc: &mut Acc,
        stack: &mut Vec<usize>,
    ) {
        for a in flow {
            let maker = match a {
                Atom::Fn(f) => *f,
                Atom::Lambda(l) => self.idx(Holder::Lambda(*l)),
                Atom::Param(h, i) if self.within(ctx, self.idx(*h)) => {
                    acc.latent.insert((self.idx(*h), *i));
                    continue;
                }
                Atom::Local(id) => {
                    if !stack.contains(id) {
                        stack.push(*id);
                        self.eval(
                            p,
                            facts,
                            &facts.locals[*id],
                            ctx,
                            args,
                            arg,
                            span,
                            acc,
                            stack,
                        );
                        stack.pop();
                    }
                    continue;
                }
                other => {
                    let (through, set) = self.closed_ref(p, other);
                    if let Some(through) = through {
                        self.closed_calls(set, through, span, arg, acc);
                    }
                    continue;
                }
            };
            let via = Via::Value {
                maker,
                span,
                arg,
                through: None,
                closed: false,
            };
            acc.add(self.effects[maker], self.layer[maker], via);
            for &token in &self.latent[maker] {
                self.latent_of(p, facts, maker, token, ctx, args, span, acc, stack);
            }
        }
    }

    /// What a use needs in the holder that makes it.
    fn use_needs(&self, p: &Program, facts: &Facts, u: &Use) -> Acc {
        let mut acc = Acc::default();
        let from = self.idx(u.from);
        let mut stack = Vec::new();
        match &u.callee {
            Callee::Holder(h) => {
                let g = self.idx(*h);
                acc.add(self.effects[g], self.layer[g], Via::Node(g, u.span));
                for &token in &self.latent[g] {
                    self.latent_of(
                        p, facts, g, token, from, &u.args, u.span, &mut acc, &mut stack,
                    );
                }
            }
            Callee::Value(flow) => {
                self.eval(
                    p, facts, flow, from, &u.args, None, u.span, &mut acc, &mut stack,
                );
            }
        }
        acc
    }

    /// Adds what a use needs to its holder; whether anything changed.
    fn merge(&mut self, h: usize, acc: &Acc) -> bool {
        let mut changed = false;
        let added = acc.bits & !self.effects[h];
        if added != 0 {
            self.effects[h] |= added;
            for e in 0..EFFECTS.len() {
                if added & (1 << e) != 0 {
                    self.effect_via[h][e] = acc.via[e];
                }
            }
            changed = true;
        }
        if acc.layer > self.layer[h] {
            self.layer[h] = acc.layer;
            self.layer_via[h] = acc.layer_via;
            changed = true;
        }
        for t in &acc.latent {
            changed |= self.latent[h].insert(*t);
        }
        changed
    }
}

fn extend(set: &mut Set, more: Set) -> bool {
    let before = set.len();
    set.extend(more);
    set.len() != before
}

/// Effects and layers of every holder: what it has itself, joined with what the holders and
/// values it uses have, until nothing changes.
pub fn derive(p: &Program) -> Derived {
    let facts = &p.effect_facts;
    let functions = p.funcs.len();
    let constants = p.consts.len();
    let defs = p.defs.len();
    let n = functions + constants + defs + facts.lambdas.len() + facts.tests.len();
    let mut d = Derived {
        effects: vec![0; n],
        effect_via: vec![[None; EFFECTS.len()]; n],
        layer: vec![0; n],
        layer_via: vec![None; n],
        latent: vec![BTreeSet::new(); n],
        closed_effects: vec![0; n],
        closed_via: vec![[None; EFFECTS.len()]; n],
        closed_layer: vec![0; n],
        closed_layer_via: vec![None; n],
        functions,
        constants,
        defs,
        lambdas: facts.lambdas.len(),
        parents: vec![None; n],
        locals: vec![Set::new(); facts.locals.len()],
        consts: HashMap::new(),
        fields: Vec::new(),
        field_ids: HashMap::new(),
        field_keys: Vec::new(),
        returns: HashMap::new(),
        escaped: HashMap::new(),
        tops: Vec::new(),
        top_ids: HashMap::new(),
        top_types: Vec::new(),
        makers: Vec::new(),
        inexact_locals: vec![false; facts.locals.len()],
        inexact_consts: HashSet::new(),
        inexact_returns: HashSet::new(),
    };
    let mut named: Vec<FnId> = facts.named.clone();
    named.sort_unstable();
    named.dedup();
    for &f in &named {
        d.makers.push((f, function_type(p, f)));
    }
    for (l, lambda) in facts.lambdas.iter().enumerate() {
        let at = d.idx(Holder::Lambda(l));
        d.parents[at] = lambda.parent.map(|h| d.idx(h));
        d.makers.push((at, lambda.ty.clone()));
    }
    for (f, def) in p.funcs.iter().enumerate() {
        let layer = p.module_layer(def.module);
        d.layer[f] = layer.unwrap_or(0);
        if layer == Some(1) {
            d.effects[f] |= 1 << ALLOCATES;
        }
        let external = def.body.is_none() && !matches!(def.owner, Owner::Interface(_));
        if external && p.modules[def.module].stdlib {
            d.effects[f] |= external_effects(p, f);
            // The runtime calls the function values an external function gets.
            for (i, q) in def.params.iter().enumerate() {
                if matches!(q.ty, Ty::Func(_)) {
                    d.latent[f].insert((f, i));
                }
            }
        }
    }
    for (l, lambda) in facts.lambdas.iter().enumerate() {
        let at = d.idx(Holder::Lambda(l));
        let layer = p.module_layer(lambda.module);
        d.layer[at] = layer.unwrap_or(0);
        if layer == Some(1) {
            d.effects[at] |= 1 << ALLOCATES;
        }
    }
    for (i, site) in facts.sites.iter().enumerate() {
        let at = d.idx(site.at);
        if d.effects[at] & (1 << ALLOCATES) == 0 {
            d.effects[at] |= 1 << ALLOCATES;
            d.effect_via[at][ALLOCATES] = Some(Via::Site(i));
        }
        if d.layer[at] < 1 {
            d.layer[at] = 1;
            d.layer_via[at] = Some(Via::Site(i));
        }
    }
    let impls = implementations(p);

    // The values that may reach each place (RFC 0005). A parameter of a named function used
    // as a value, or of a lambda, may get any value of its type.
    for &f in &named {
        for (i, q) in p.funcs[f].params.iter().enumerate() {
            if may_hold_function(&q.ty) {
                let id = d.top(p, &q.ty);
                let set = d.tops[id].clone();
                d.escaped.entry((f, i)).or_default().extend(set);
            }
        }
    }
    for (l, lambda) in facts.lambdas.iter().enumerate() {
        let at = d.idx(Holder::Lambda(l));
        for (i, (_, t)) in lambda.params.iter().enumerate() {
            let id = d.top(p, t);
            let set = d.tops[id].clone();
            d.escaped.entry((at, i)).or_default().extend(set);
        }
    }
    loop {
        let mut changed = false;
        for (id, flow) in facts.locals.iter().enumerate() {
            let set = d.closed_flow(p, flow);
            changed |= extend(&mut d.locals[id], set);
            if !d.inexact_locals[id] && flow.iter().any(|a| d.inexact(p, a)) {
                d.inexact_locals[id] = true;
                changed = true;
            }
        }
        for (c, flow) in &facts.consts {
            let set = d.closed_flow(p, flow);
            changed |= extend(d.consts.entry(*c).or_default(), set);
            if !d.inexact_consts.contains(c) && flow.iter().any(|a| d.inexact(p, a)) {
                changed |= d.inexact_consts.insert(*c);
            }
        }
        for ((def, name), flow) in &facts.stores {
            let set = d.closed_flow(p, flow);
            let id = d.field(*def, name);
            changed |= extend(&mut d.fields[id], set);
        }
        for (f, flow) in &facts.returns {
            let set = d.closed_flow(p, flow);
            changed |= extend(d.returns.entry(*f).or_default(), set);
            if !d.inexact_returns.contains(f) && flow.iter().any(|a| d.inexact(p, a)) {
                changed |= d.inexact_returns.insert(*f);
            }
        }
        for u in &facts.uses {
            if let Callee::Value(flow) = &u.callee {
                d.closed_flow(p, flow);
            }
            let callee = match u.callee {
                Callee::Holder(Holder::Func(f)) => Some(f),
                _ => None,
            };
            for (i, flow) in &u.args {
                let set = d.closed_flow(p, flow);
                if let Some(f) = callee {
                    changed |= extend(d.escaped.entry((f, *i)).or_default(), set);
                }
            }
        }
        for ((f, i), flow) in &facts.defaults {
            let set = d.closed_flow(p, flow);
            changed |= extend(d.escaped.entry((*f, *i)).or_default(), set);
        }
        for &(m, f, _) in &impls {
            for i in 0..p.funcs[m].params.len() {
                let set = d.escaped.get(&(m, i)).cloned().unwrap_or_default();
                changed |= extend(d.escaped.entry((f, i)).or_default(), set);
            }
        }
        if !changed {
            break;
        }
    }
    for f in 0..functions {
        let ret = p.funcs[f].ret.clone();
        if matches!(ret, Ty::Func(_)) {
            d.top(p, &ret);
        }
    }

    loop {
        let mut changed = false;
        for u in &facts.uses {
            let acc = d.use_needs(p, facts, u);
            changed |= d.merge(d.idx(u.from), &acc);
        }
        for &(m, f, span) in &impls {
            let mut acc = Acc::default();
            acc.add(d.effects[f], d.layer[f], Via::Node(f, span));
            acc.latent = d.latent[f]
                .iter()
                .map(|&(s, i)| if s == f { (m, i) } else { (s, i) })
                .collect();
            changed |= d.merge(m, &acc);
        }
        for h in 0..n {
            let mut acc = Acc {
                bits: d.effects[h],
                via: d.effect_via[h],
                layer: d.layer[h],
                layer_via: d.layer_via[h],
                ..Default::default()
            };
            for &(s, i) in &d.latent[h] {
                d.closed_calls(
                    d.escaped.get(&(s, i)),
                    Through::Escaped(s, i),
                    Span::default(),
                    None,
                    &mut acc,
                );
            }
            let added = acc.bits & !d.closed_effects[h];
            if added != 0 {
                d.closed_effects[h] |= added;
                for e in 0..EFFECTS.len() {
                    if added & (1 << e) != 0 {
                        d.closed_via[h][e] = acc.via[e];
                    }
                }
                changed = true;
            }
            if acc.layer > d.closed_layer[h] {
                d.closed_layer[h] = acc.layer;
                d.closed_layer_via[h] = acc.layer_via;
                changed = true;
            }
        }
        if !changed {
            return d;
        }
    }
}

impl Derived {
    /// The steps from `f` down to the function or expression that has the effect itself.
    pub fn effect_chain(&self, f: FnId, effect: usize) -> Vec<Step> {
        let mut chain = vec![Step::Node(f)];
        chain.extend(self.follow(self.effect_via[f][effect], Some(effect)));
        chain
    }

    /// The steps from `f` down to the first function or expression of the layer it needs.
    pub fn layer_chain(&self, f: FnId) -> Vec<Step> {
        let mut chain = vec![Step::Node(f)];
        chain.extend(self.follow(self.layer_via[f], None));
        chain
    }

    pub fn effect_names(&self, f: FnId) -> Vec<&'static str> {
        (0..EFFECTS.len())
            .filter(|e| self.effects[f] & (1 << e) != 0)
            .map(|e| EFFECTS[e])
            .collect()
    }

    /// The parameters of `f` whose calls count for the caller of `f`, with the effects of the
    /// argument it passes (RFC 0005).
    pub fn argument_effects(&self, p: &Program, f: FnId) -> Vec<String> {
        self.latent[f]
            .iter()
            .filter(|(s, _)| *s == f)
            .filter_map(|(_, i)| p.funcs[f].params.get(*i).map(|q| q.name.clone()))
            .collect()
    }

    /// The chain from a reason: of an effect, or of the layer when `effect` is None.
    fn follow(&self, first: Option<Via>, effect: Option<usize>) -> Vec<Step> {
        let mut chain = Vec::new();
        let mut seen: HashSet<(usize, bool)> = HashSet::new();
        let mut via = first;
        while let Some(v) = via {
            let (node, closed) = match v {
                Via::Site(s) => {
                    chain.push(Step::Site(s));
                    break;
                }
                Via::Node(n, _) => (n, false),
                Via::Value {
                    maker,
                    arg,
                    through,
                    closed,
                    ..
                } => {
                    if let Some((g, i)) = arg {
                        chain.push(Step::Arg(g, i));
                    }
                    if let Some(t) = through {
                        chain.push(Step::Through(t));
                    }
                    (maker, closed)
                }
            };
            if !seen.insert((node, closed)) {
                break;
            }
            chain.push(Step::Node(node));
            via = match (effect, closed) {
                (Some(e), false) => self.effect_via[node][e],
                (Some(e), true) => self.closed_via[node][e],
                (None, false) => self.layer_via[node],
                (None, true) => self.closed_layer_via[node],
            };
        }
        chain
    }

    /// The first reason in a chain from `start` that is an estimate: the holder that makes
    /// the call and its place.
    fn first_uncertain(
        &self,
        start: usize,
        first: Option<Via>,
        effect: Option<usize>,
    ) -> Option<(usize, Span)> {
        let mut via = first;
        let mut at = start;
        let mut seen: HashSet<(usize, bool)> = HashSet::new();
        while let Some(v) = via {
            let (node, closed) = match v {
                Via::Site(_) => return None,
                Via::Node(n, _) => (n, false),
                Via::Value {
                    maker,
                    through,
                    closed,
                    span,
                    ..
                } => {
                    if through.is_some_and(|t| self.uncertain(t)) {
                        return Some((at, span));
                    }
                    (maker, closed)
                }
            };
            if !seen.insert((node, closed)) {
                return None;
            }
            at = node;
            via = match (effect, closed) {
                (Some(e), false) => self.effect_via[node][e],
                (Some(e), true) => self.closed_via[node][e],
                (None, false) => self.layer_via[node],
                (None, true) => self.closed_layer_via[node],
            };
        }
        None
    }

    /// The name of a holder as a diagnostic shows it, in English and in Russian.
    fn holder_name(&self, p: &Program, h: usize) -> (String, String) {
        match self.holder(h) {
            Holder::Func(f) => {
                let name = crate::symbol::node_name(p, Node::Func(f));
                (name.clone(), name)
            }
            Holder::Const(c) => {
                let name = crate::symbol::node_name(p, Node::Const(c));
                (name.clone(), name)
            }
            Holder::Def(d) => {
                let name = crate::symbol::node_name(p, Node::Def(d));
                (name.clone(), name)
            }
            Holder::Test(t) => {
                let name = &p.effect_facts.tests[t].name;
                (format!("the test \"{name}\""), format!("тест \"{name}\""))
            }
            Holder::Lambda(l) => {
                let line = p.effect_facts.lambdas[l].span.start.line;
                match self.parents[h] {
                    Some(parent) => {
                        let (en, ru) = self.holder_name(p, parent);
                        (
                            format!("the lambda on line {line} in {en}"),
                            format!("лямбда в строке {line} в {ru}"),
                        )
                    }
                    None => (
                        format!("the lambda on line {line}"),
                        format!("лямбда в строке {line}"),
                    ),
                }
            }
        }
    }

    fn param_name(&self, p: &Program, h: usize, i: usize) -> String {
        match self.holder(h) {
            Holder::Func(f) => p.funcs[f].params.get(i).map(|q| q.name.clone()),
            Holder::Lambda(l) => p.effect_facts.lambdas[l]
                .params
                .get(i)
                .map(|(n, _)| n.clone()),
            _ => None,
        }
        .unwrap_or_else(|| format!("{}", i + 1))
    }

    /// The chain as a diagnostic shows it, in English and in Russian.
    fn show_chain(&self, p: &Program, chain: &[Step]) -> (String, String) {
        let mut en = Vec::new();
        let mut ru = Vec::new();
        for step in chain {
            let (e, r) = match *step {
                Step::Node(n) => self.holder_name(p, n),
                Step::Site(s) => {
                    let site = &p.effect_facts.sites[s];
                    let (what_en, what_ru) = site.what.describe();
                    let line = site.span.start.line;
                    (
                        format!("{what_en} on line {line}"),
                        format!("{what_ru} в строке {line}"),
                    )
                }
                Step::Arg(g, i) => {
                    let (name_en, name_ru) = self.holder_name(p, g);
                    let param = self.param_name(p, g, i);
                    (
                        format!("{name_en} (argument {param})"),
                        format!("{name_ru} (аргумент {param})"),
                    )
                }
                Step::Through(t) => self.show_through(p, t),
            };
            en.push(e);
            ru.push(r);
        }
        (en.join(" -> "), ru.join(" -> "))
    }

    fn show_through(&self, p: &Program, t: Through) -> (String, String) {
        match t {
            Through::Const(c) => {
                let name = crate::symbol::node_name(p, Node::Const(c));
                (name.clone(), name)
            }
            Through::Field(id) => {
                let (d, field) = &self.field_keys[id];
                let name = format!("{}.{field}", crate::symbol::node_name(p, Node::Def(*d)));
                (
                    format!("the values stored in the field {name}"),
                    format!("значения, записанные в поле {name}"),
                )
            }
            Through::Top(id) => {
                let shown = p.show(&self.top_types[id]);
                (
                    format!("any function value of type {shown}"),
                    format!("любое значение-функция типа {shown}"),
                )
            }
            Through::Escaped(h, i) => {
                let (en, ru) = self.holder_name(p, h);
                let param = self.param_name(p, h, i);
                (
                    format!("the values passed to {en} as {param}"),
                    format!("значения, переданные в {ru} как {param}"),
                )
            }
            Through::Ret(f) => {
                let name = crate::symbol::node_name(p, Node::Func(f));
                (
                    format!("the values {name} returns"),
                    format!("значения, которые возвращает {name}"),
                )
            }
        }
    }
}

/// The effects an external function of a standard-library module has itself, as bits. An
/// external of `std.files` has the scopes its "Permission:" line names (semantics 8.2).
fn external_effects(p: &Program, f: FnId) -> u8 {
    let def = &p.funcs[f];
    let module = &p.modules[def.module];
    let effect = match module.name.as_str() {
        "std.files" => return file_scopes(&module.analysis.source, def.span.start.line),
        "std.net" => "network",
        "std.process" => "process",
        "std.env" => "env",
        _ => return 0,
    };
    EFFECTS
        .iter()
        .position(|e| *e == effect)
        .map_or(0, |e| 1 << e)
}

/// The file scopes named by the "Permission:" sentence of the documentation comment that ends
/// above the line; both scopes when there is none, so that a missing line is never permissive.
fn file_scopes(source: &str, line: usize) -> u8 {
    let lines: Vec<&str> = source.lines().collect();
    let mut doc = String::new();
    let mut at = line.saturating_sub(1).min(lines.len());
    while at > 0 && lines[at - 1].trim_start().starts_with("##") {
        at -= 1;
        doc.insert_str(0, lines[at]);
    }
    let Some((_, permission)) = doc.split_once("Permission:") else {
        return (1 << FILES_READ) | (1 << FILES_WRITE);
    };
    let mut scopes = 0;
    if permission.contains("files.read") {
        scopes |= 1 << FILES_READ;
    }
    if permission.contains("files.write") {
        scopes |= 1 << FILES_WRITE;
    }
    if permission.trim_start().starts_with("none") {
        scopes = 0;
    }
    scopes
}

const ALLOCATES: usize = 5;

/// The implementations of an interface method: the method of the same name in every
/// `extend ... with` of its interface in the program. A type implements an interface only in
/// its own module or in the interface's, and every module of the program is loaded, so the
/// set is complete for the program (semantics 8.4).
fn implementations(p: &Program) -> Vec<(FnId, FnId, Span)> {
    let mut found = Vec::new();
    for imp in &p.impls {
        let Some(Ty::Named(i, _)) = &imp.interface else {
            continue;
        };
        let DefKind::Interface(members) = &p.defs[*i].kind else {
            continue;
        };
        for &m in members {
            for &f in &imp.methods {
                if p.funcs[f].name == p.funcs[m].name {
                    found.push((m, f, p.funcs[f].span));
                }
            }
        }
    }
    found
}

/// The build profile of `unio check` (C-21).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Profile {
    #[default]
    Application,
    Core,
}

/// The effects a manifest permits, or None when it has no `[permissions]` table.
pub fn manifest_permissions(package: &Path) -> Option<HashSet<&'static str>> {
    let text = std::fs::read_to_string(package.join("unio.toml")).ok()?;
    let mut section = String::new();
    let mut found = false;
    let mut permitted = HashSet::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            section = line
                .trim_matches(|c| c == '[' || c == ']')
                .trim()
                .to_string();
            found |= section == "permissions";
            continue;
        }
        if section != "permissions" {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        let granted = match value {
            "false" | "[]" | "\"\"" => false,
            _ => !value.is_empty(),
        };
        if !granted {
            continue;
        }
        let effect = match key {
            "files.read" => "files.read",
            "files.write" => "files.write",
            _ => match key.split('.').next().unwrap_or("") {
                "network" | "net" => "network",
                "process" => "process",
                "env" => "env",
                _ => continue,
            },
        };
        permitted.insert(effect);
    }
    found.then_some(permitted)
}

/// E0084 and E0087 for the core profile and E0085 for the manifest, on the modules that are
/// reported.
pub fn report(p: &mut Program, profile: Profile) {
    let d = derive(p);
    let mut found: Vec<(ModId, Diagnostic)> = Vec::new();
    {
        let p: &Program = p;
        let facts = &p.effect_facts;
        let reported = |m: ModId| p.modules[m].report && !p.modules[m].stdlib;
        if profile == Profile::Core {
            for (f, def) in p.funcs.iter().enumerate() {
                if !reported(def.module) || d.layer[f] == 0 {
                    continue;
                }
                let chain = d.layer_chain(f);
                let (en, ru) = d.show_chain(p, &chain);
                let layer = LAYER_NAMES[d.layer[f]];
                // A chain through a value whose possible values are only estimated cannot show
                // that the call stays in core (RFC 0005): E0087 at that call when the function
                // makes it, else at the call of the function that leads to it, so that every
                // function is reported at a place of its own.
                if let Some((at, span)) = d.first_uncertain(f, d.layer_via[f], None) {
                    let span = if d.root(at) == Some(f) {
                        span
                    } else {
                        match d.layer_via[f] {
                            Some(Via::Node(_, s) | Via::Value { span: s, .. }) => s,
                            _ => def.span,
                        }
                    };
                    let message = catalog::unproven_value_call(&def.name, layer, (&en, &ru));
                    found.push((def.module, Diagnostic::error("E0087", span, message)));
                    continue;
                }
                let message = catalog::profile_violation(&def.name, layer, (&en, &ru));
                found.push((def.module, Diagnostic::error("E0084", def.span, message)));
            }
        }
        // A call through a function value in a module of the `core` layer of the library must
        // stay in core (S-11, RFC 0005).
        for u in &facts.uses {
            let module = &p.modules[u.module];
            if !module.report || p.module_layer(u.module) != Some(0) {
                continue;
            }
            if !matches!(u.callee, Callee::Value(_)) {
                continue;
            }
            let acc = d.use_needs(p, facts, u);
            if acc.layer == 0 {
                continue;
            }
            let from = d.idx(u.from);
            let mut chain = vec![Step::Node(from)];
            chain.extend(d.follow(acc.layer_via, None));
            let (en, ru) = d.show_chain(p, &chain);
            let (name, _) = d.holder_name(p, from);
            let message = catalog::unproven_value_call(&name, LAYER_NAMES[acc.layer], (&en, &ru));
            found.push((u.module, Diagnostic::error("E0087", u.span, message)));
        }
        let mut permissions: HashMap<ModId, Option<HashSet<&'static str>>> = HashMap::new();
        let mut reported_effects: HashSet<(ModId, usize)> = HashSet::new();
        // Every use at a place: calls, implicit calls, uses of constants and field defaults,
        // and calls through function values (semantics 8.4).
        let mut uses: Vec<&Use> = facts.uses.iter().collect();
        uses.sort_by_key(|u| (u.module, u.span.start.offset));
        for u in uses {
            if !reported(u.module) {
                continue;
            }
            let from = d.idx(u.from);
            // A test and its lambdas are checked as the test runs: by the sandbox, not
            // against the manifest. What their values bring into the program is checked
            // where the program calls them.
            if d.root(from).is_none() {
                continue;
            }
            let permitted = permissions
                .entry(u.module)
                .or_insert_with(|| manifest_permissions(&p.modules[u.module].package));
            let Some(permitted) = permitted else {
                continue;
            };
            let mut acc = None;
            for (e, effect) in EFFECTS.iter().enumerate() {
                if e == ALLOCATES || permitted.contains(effect) {
                    continue;
                }
                if reported_effects.contains(&(u.module, e)) {
                    continue;
                }
                let acc = acc.get_or_insert_with(|| d.use_needs(p, facts, u));
                if acc.bits & (1 << e) == 0 {
                    continue;
                }
                reported_effects.insert((u.module, e));
                let mut chain = vec![Step::Node(from)];
                chain.extend(d.follow(acc.via[e], Some(e)));
                let (en, ru) = d.show_chain(p, &chain);
                let message = catalog::effect_not_permitted(effect, (&en, &ru));
                found.push((u.module, Diagnostic::error("E0085", u.span, message)));
            }
        }
    }
    for (m, diagnostic) in found {
        p.error(m, diagnostic);
    }
}
