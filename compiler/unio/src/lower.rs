//! The lowered form of checked code that the interpreter runs (docs/runtime.md 8.1).
//!
//! Each function body, test, lambda, constant, and default value is lowered once, the first
//! time it runs, from the checked tree to [`Node`] and [`Stmt`]: local names become slots of a
//! frame, a call of a named function knows the function and the parameter of each argument, a
//! construction knows the field of each argument, literals are values, and the source text of
//! an assertion is kept for its message. Method calls keep the name of the method, because the
//! method depends on the value; the interpreter finds it once per type at each call site.
//!
//! The lowering follows the scopes of the tree walker it replaced: a name is visible from its
//! binding to the end of the block, the condition of `if` and `while` binds for its branch,
//! `is` in an expression binds only inside that expression, and a lambda captures the values
//! of the outer names it uses when it is created (L-11a).

use crate::ast::*;
use crate::interp::{Global, Interp};
use crate::program::{DefKind, Symbol};
use std::cell::Cell;
use std::rc::Rc;
use unio_runtime::value::Value;

pub(crate) type Slot = u32;

/// A lowered function: parameters occupy the first slots, `self` first.
pub(crate) struct Code {
    pub slots: usize,
    /// Whether a parameter has a default, which a call that omits it evaluates.
    pub defaults: bool,
    pub params: Vec<ParamCode>,
    pub body: Block,
}

pub(crate) struct ParamCode {
    pub var: bool,
    pub default: Option<Node>,
}

/// A lowered expression evaluated in its own frame: a constant, a default of an external or a
/// field, or the body of a lambda.
pub(crate) struct ExprCode {
    pub slots: usize,
    pub body: Node,
}

pub(crate) struct LambdaCode {
    pub slots: usize,
    pub params: usize,
    /// Slots of the creating frame copied into slots of the lambda's frame.
    pub captures: Vec<(Slot, Slot)>,
    pub body: Node,
}

/// Statements and the slots they bind, which are cleared when the block ends so that values
/// are freed and unshared as early as the scopes of the language allow.
pub(crate) struct Block {
    pub stmts: Vec<Stmt>,
    pub clear: (Slot, Slot),
}

pub(crate) struct Stmt {
    pub line: u32,
    pub kind: S,
}

pub(crate) enum S {
    Let(Slot, Node),
    Assign(Place, Node),
    /// `m[k] = v` and `l[i] = v`.
    SetIndex(Place, Node, Node),
    /// `x = x + a + b` on text where the parts do not use `x`: the text grows in place.
    Concat(Slot, Vec<Node>, Box<Node>),
    Return(Option<Node>),
    Fail(Node),
    Assert(Node, String),
    Break,
    Continue,
    Pass,
    Expr(Node),
    /// A simple statement with an inline handler `on Type as name:`.
    Handled {
        inner: Box<S>,
        ty: String,
        bind: Option<Slot>,
        block: Block,
    },
    If(Vec<(Node, Block)>, Option<Block>),
    While(Node, Block),
    For(Vec<Slot>, Node, Block),
    Match(Node, Vec<CaseCode>),
    Try(Block, Vec<HandlerCode>),
    Using(Slot, Node, Block),
}

pub(crate) struct CaseCode {
    pub pattern: Pat,
    pub guard: Option<Node>,
    pub body: Block,
}

pub(crate) struct HandlerCode {
    pub ty: String,
    pub bind: Option<Slot>,
    pub body: Block,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Op {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Power,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    In,
    NotIn,
}

impl Op {
    fn of(op: &str) -> Option<Op> {
        Some(match op {
            "+" => Op::Add,
            "-" => Op::Subtract,
            "*" => Op::Multiply,
            "/" => Op::Divide,
            "%" => Op::Remainder,
            "**" => Op::Power,
            "==" => Op::Equal,
            "!=" => Op::NotEqual,
            "<" => Op::Less,
            "<=" => Op::LessEqual,
            ">" => Op::Greater,
            ">=" => Op::GreaterEqual,
            "in" => Op::In,
            "not in" => Op::NotIn,
            _ => return None,
        })
    }

    pub fn text(self) -> &'static str {
        match self {
            Op::Add => "+",
            Op::Subtract => "-",
            Op::Multiply => "*",
            Op::Divide => "/",
            Op::Remainder => "%",
            Op::Power => "**",
            Op::Equal => "==",
            Op::NotEqual => "!=",
            Op::Less => "<",
            Op::LessEqual => "<=",
            Op::Greater => ">",
            Op::GreaterEqual => ">=",
            Op::In => "in",
            Op::NotIn => "not in",
        }
    }
}

pub(crate) enum Node {
    Value(Value),
    Local(Slot),
    Const(usize),
    FnRef(usize),
    /// A variant without fields written as a name.
    Variant(usize, Rc<str>),
    Text(Vec<Part>),
    Tuple(Vec<Node>),
    List(Vec<Node>),
    Map(Vec<(Node, Node)>),
    Comprehension(Box<ComprehensionCode>),
    Lambda(Rc<LambdaCode>),
    Is(Box<Node>, Box<Pat>),
    Conditional(Box<[Node; 3]>),
    And(Box<Node>, Box<Node>),
    Or(Box<Node>, Box<Node>, bool),
    Not(Box<Node>),
    Negate(Box<Node>),
    Binary(Op, Box<Node>, Box<Node>),
    /// `try` on a value of `Result` (true) or on a call that fails (false).
    Try(Box<Node>, bool),
    Call(Box<CallCode>),
    CallValue(Box<Node>, Vec<Node>),
    Construct(Box<ConstructCode>),
    Method(Box<MethodCode>),
    Index(Box<Node>, Box<Node>),
    /// A field and the position it had in the last value read here.
    Field(Box<Node>, Rc<str>, Cell<u32>),
    SafeField(Box<Node>, Rc<str>),
    /// Code that a checked program cannot reach; running it is an internal fault.
    Fault(String),
}

pub(crate) enum Part {
    Literal(String),
    Value(Node),
}

pub(crate) struct ComprehensionCode {
    pub element: Node,
    pub names: Vec<Slot>,
    pub iterable: Node,
    pub condition: Option<Node>,
}

pub(crate) struct ArgCode {
    /// The position among the callee's parameters, `self` first.
    pub target: usize,
    pub value: Node,
    /// The place of a `var` argument, which is moved into the call and back.
    pub place: Option<Place>,
}

pub(crate) struct CallCode {
    pub f: usize,
    pub args: Vec<ArgCode>,
    /// Explicit type arguments, resolved at the call for `std.json.from_value`.
    pub types: Vec<TypeCode>,
    pub count: usize,
}

/// A written type argument with its names resolved in its module: at the call, a name of one
/// part is first a type parameter of the caller (`generic`), then `base` with `args`.
pub(crate) struct TypeCode {
    pub generic: Option<String>,
    pub base: TypeBase,
    pub args: Vec<TypeCode>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TypeBase {
    Int,
    Float,
    Bool,
    Text,
    Bytes,
    Path,
    List,
    Map,
    Set,
    Tuple,
    Def(usize),
    Unknown,
}

pub(crate) struct ConstructCode {
    pub def: usize,
    /// The variant of an enum; `None` for a struct or an error.
    pub variant: Option<Rc<str>>,
    /// Arguments in written order with the field each sets.
    pub args: Vec<(usize, Node)>,
    pub fields: usize,
}

pub(crate) struct MethodArg {
    pub name: Option<String>,
    pub value: Node,
    pub place: Option<Place>,
}

pub(crate) struct MethodCode {
    pub receiver: Node,
    pub name: String,
    pub args: Vec<MethodArg>,
    /// The receiver as a place, for a `var self` method.
    pub place: Option<Place>,
    /// The last type seen here and its method: the address of the type's identity and the method.
    pub cache: Cell<(usize, usize)>,
    /// The identity whose address `cache` holds, kept alive so that the address cannot be
    /// reused by the name of another type while it is cached.
    pub pin: std::cell::RefCell<Option<Rc<str>>>,
    /// Whether every argument is positional.
    pub positional: bool,
}

#[derive(Clone)]
pub(crate) struct Place {
    pub root: Slot,
    pub steps: Vec<PStep>,
}

#[derive(Clone)]
pub(crate) enum PStep {
    Field(Rc<str>),
    Index(Rc<Node>),
}

pub(crate) enum Pat {
    Wildcard,
    Int(Option<i64>),
    Bool(bool),
    Text(String),
    /// A variant, a struct type, a binding, or a constant: decided by the value first, as the
    /// checker leaves it (semantics 1.4).
    Name {
        last: Rc<str>,
        fallback: NameFallback,
        /// The address of the identity of the last enum type seen here that has this variant.
        known: Cell<usize>,
        /// The identity whose address `known` holds, kept alive while it is cached.
        pin: std::cell::RefCell<Option<Rc<str>>>,
    },
    Constructor {
        last: Rc<str>,
        args: Vec<(Option<String>, Pat)>,
    },
    List {
        elements: Vec<ListPat>,
        fixed: usize,
        rest: bool,
    },
    Tuple(Vec<Pat>),
    Bind(Box<Pat>, Slot),
    Or(Vec<Pat>),
}

pub(crate) enum NameFallback {
    Bind(Slot),
    Const(usize),
    No,
}

pub(crate) enum ListPat {
    Pattern(Pat),
    Rest(Slot),
}

/// One frame being lowered: the visible names and their slots.
struct Frame {
    scope: Vec<(String, Slot)>,
    slots: Slot,
    captures: Vec<(Slot, Slot)>,
}

pub(crate) struct Lowering<'a> {
    interp: &'a Interp,
    module: usize,
    frames: Vec<Frame>,
}

impl<'a> Lowering<'a> {
    pub fn new(interp: &'a Interp, module: usize) -> Lowering<'a> {
        Lowering {
            interp,
            module,
            frames: vec![Frame {
                scope: Vec::new(),
                slots: 0,
                captures: Vec::new(),
            }],
        }
    }

    fn top(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("a frame")
    }

    pub fn slots(&self) -> usize {
        self.frames.last().map_or(0, |f| f.slots as usize)
    }

    /// Binds a name to a new slot of the current frame.
    pub fn bind(&mut self, name: &str) -> Slot {
        let frame = self.top();
        let slot = frame.slots;
        frame.slots += 1;
        frame.scope.push((name.to_string(), slot));
        slot
    }

    fn mark(&self) -> (usize, Slot) {
        let frame = self.frames.last().expect("a frame");
        (frame.scope.len(), frame.slots)
    }

    fn restore(&mut self, mark: (usize, Slot)) -> (Slot, Slot) {
        let frame = self.top();
        frame.scope.truncate(mark.0);
        (mark.1, frame.slots)
    }

    /// The slot of a local name, capturing it from enclosing frames for a lambda.
    fn local(&mut self, name: &str) -> Option<Slot> {
        self.local_in(self.frames.len() - 1, name)
    }

    fn local_in(&mut self, frame: usize, name: &str) -> Option<Slot> {
        if let Some((_, slot)) = self.frames[frame]
            .scope
            .iter()
            .rev()
            .find(|(n, _)| n == name)
        {
            return Some(*slot);
        }
        if frame == 0 {
            return None;
        }
        let outer = self.local_in(frame - 1, name)?;
        let f = &mut self.frames[frame];
        let inner = f.slots;
        f.slots += 1;
        f.scope.insert(0, (name.to_string(), inner));
        f.captures.push((outer, inner));
        Some(inner)
    }

    // ---------------------------------------------------------------- statements

    pub fn block(&mut self, block: &crate::ast::Block) -> Block {
        let mark = self.mark();
        let stmts = block.iter().map(|s| self.stmt(s)).collect();
        let clear = self.restore(mark);
        Block { stmts, clear }
    }

    fn stmt(&mut self, statement: &Statement) -> Stmt {
        let line = statement.span.start.line as u32;
        let kind = match &statement.kind {
            StatementKind::Simple {
                statement: simple,
                handler,
            } => {
                let inner = self.simple(simple);
                match handler {
                    Some(block) => match find_handler(simple) {
                        Some((ty, name)) => {
                            let mark = self.mark();
                            let bind = name.as_ref().map(|n| self.bind(&n.text));
                            let mut body = self.block(block);
                            let (start, _) = self.restore(mark);
                            body.clear.0 = start;
                            S::Handled {
                                inner: Box::new(inner),
                                ty: self.handler_type(ty),
                                bind,
                                block: body,
                            }
                        }
                        None => S::Expr(Node::Fault("internal: inline handler".into())),
                    },
                    None => inner,
                }
            }
            StatementKind::If {
                branches,
                otherwise,
            } => {
                let mut out = Vec::new();
                for (condition, body) in branches {
                    let mark = self.mark();
                    let c = self.condition(condition);
                    let mut b = self.block(body);
                    let (start, _) = self.restore(mark);
                    b.clear.0 = start;
                    out.push((c, b));
                }
                S::If(out, otherwise.as_ref().map(|b| self.block(b)))
            }
            StatementKind::While { condition, body } => {
                let mark = self.mark();
                let c = self.condition(condition);
                let mut b = self.block(body);
                let (start, _) = self.restore(mark);
                b.clear.0 = start;
                S::While(c, b)
            }
            StatementKind::For {
                names,
                iterable,
                body,
            } => {
                let source = self.expr(iterable);
                let mark = self.mark();
                let slots = names.iter().map(|n| self.bind(&n.text)).collect();
                let mut b = self.block(body);
                let (start, _) = self.restore(mark);
                b.clear.0 = start;
                S::For(slots, source, b)
            }
            StatementKind::Match { subject, cases } => {
                let subject = self.expr(subject);
                let cases = cases
                    .iter()
                    .map(|case| {
                        let mark = self.mark();
                        let pattern = self.pattern(&case.pattern);
                        let guard = case.guard.as_ref().map(|g| self.expr(g));
                        let mut body = self.block(&case.body);
                        let (start, _) = self.restore(mark);
                        body.clear.0 = start;
                        CaseCode {
                            pattern,
                            guard,
                            body,
                        }
                    })
                    .collect();
                S::Match(subject, cases)
            }
            StatementKind::Try { body, handlers } => {
                let body = self.block(body);
                let handlers = handlers
                    .iter()
                    .map(|h| {
                        let mark = self.mark();
                        let bind = h.name.as_ref().map(|n| self.bind(&n.text));
                        let mut body = self.block(&h.body);
                        let (start, _) = self.restore(mark);
                        body.clear.0 = start;
                        HandlerCode {
                            ty: self.handler_type(&h.ty),
                            bind,
                            body,
                        }
                    })
                    .collect();
                S::Try(body, handlers)
            }
            StatementKind::Using { name, value, body } => {
                let value = self.expr(value);
                let mark = self.mark();
                let slot = self.bind(&name.text);
                let mut b = self.block(body);
                let (start, _) = self.restore(mark);
                b.clear.0 = start;
                S::Using(slot, value, b)
            }
        };
        Stmt { line, kind }
    }

    fn simple(&mut self, simple: &Simple) -> S {
        match simple {
            Simple::Let { name, value, .. } => {
                let v = self.expr(value);
                S::Let(self.bind(&name.text), v)
            }
            Simple::Assign { target, value } => self.assign(target, value),
            Simple::Return(e) => S::Return(e.as_ref().map(|e| self.expr(e))),
            Simple::Fail(e) => S::Fail(self.expr(e)),
            Simple::Assert(e) => {
                let text = self.interp.source_text(self.module, e.span);
                S::Assert(self.expr(e), text)
            }
            Simple::Break => S::Break,
            Simple::Continue => S::Continue,
            Simple::Pass => S::Pass,
            Simple::Expr(e) => S::Expr(self.expr(e)),
        }
    }

    fn assign(&mut self, target: &Expr, value: &Expr) -> S {
        if let ExprKind::Index { target: t, index } = &target.kind {
            let Some(place) = self.place(t) else {
                return S::Expr(Node::Fault("internal: not a place".into()));
            };
            let key = self.expr(index);
            return S::SetIndex(place, key, self.expr(value));
        }
        if let ExprKind::Name(n) = &target.kind {
            let mut parts = Vec::new();
            if concat_parts(value, n, &mut parts) && !parts.is_empty() {
                if let Some(slot) = self.local(n) {
                    let general = self.expr(value);
                    let parts = parts.into_iter().map(|p| self.expr(p)).collect();
                    return S::Concat(slot, parts, Box::new(general));
                }
            }
        }
        match self.place(target) {
            Some(place) => S::Assign(place, self.expr(value)),
            None => S::Expr(Node::Fault("internal: not a place".into())),
        }
    }

    pub fn place(&mut self, expr: &Expr) -> Option<Place> {
        match &expr.kind {
            ExprKind::Paren(e) => self.place(e),
            ExprKind::Name(n) => Some(Place {
                root: self.local(n)?,
                steps: Vec::new(),
            }),
            ExprKind::SelfValue => Some(Place {
                root: self.local("self")?,
                steps: Vec::new(),
            }),
            ExprKind::Field { target, name } => {
                let mut place = self.place(target)?;
                place
                    .steps
                    .push(PStep::Field(self.interp.shared(&name.text)));
                Some(place)
            }
            ExprKind::Index { target, index } => {
                let mut place = self.place(target)?;
                place.steps.push(PStep::Index(Rc::new(self.expr(index))));
                Some(place)
            }
            _ => None,
        }
    }

    // ---------------------------------------------------------------- expressions

    /// A condition of `if` or `while`: `is` tests and the left operands of `and` bind their
    /// names for the guarded branch (L-33).
    fn condition(&mut self, expr: &Expr) -> Node {
        match &expr.kind {
            ExprKind::Paren(e) => self.condition(e),
            ExprKind::Is { value, pattern } => {
                let v = self.expr(value);
                Node::Is(Box::new(v), Box::new(self.pattern(pattern)))
            }
            ExprKind::Binary {
                op: "and",
                left,
                right,
            } => {
                let l = self.condition(left);
                let r = self.condition(right);
                Node::And(Box::new(l), Box::new(r))
            }
            _ => self.expr(expr),
        }
    }

    pub fn expr(&mut self, expr: &Expr) -> Node {
        match &expr.kind {
            ExprKind::Int(text) => match crate::interp::parse_int(text) {
                Some(i) => Node::Value(Value::Int(i)),
                None => Node::Fault("internal: integer literal".into()),
            },
            ExprKind::Float(text) => match text.replace('_', "").parse::<f64>() {
                Ok(x) => Node::Value(Value::Float(x)),
                Err(_) => Node::Fault("internal: float literal".into()),
            },
            ExprKind::Bool(b) => Node::Value(Value::Bool(*b)),
            ExprKind::Text(parts) => {
                if let [] | [TextPart::Literal(_)] = &parts[..] {
                    let text = match parts.first() {
                        Some(TextPart::Literal(s)) => s.clone(),
                        _ => String::new(),
                    };
                    return Node::Value(Value::text(text));
                }
                Node::Text(
                    parts
                        .iter()
                        .map(|p| match p {
                            TextPart::Literal(s) => Part::Literal(s.clone()),
                            TextPart::Interpolation(e) => Part::Value(self.expr(e)),
                        })
                        .collect(),
                )
            }
            ExprKind::SelfValue => match self.local("self") {
                Some(slot) => Node::Local(slot),
                None => Node::Fault("internal: self".into()),
            },
            ExprKind::Name(n) => self.name(n),
            ExprKind::Paren(e) => self.expr(e),
            ExprKind::Tuple(items) => constant_items(
                items.iter().map(|e| self.expr(e)).collect(),
                Node::Tuple,
                Value::tuple,
            ),
            // A list of literals is one value made once: `c in [" ", "\t"]` allocates nothing,
            // and changing a copy of it copies first, as for any shared value.
            ExprKind::List(items) => constant_items(
                items.iter().map(|e| self.expr(e)).collect(),
                Node::List,
                Value::list,
            ),
            ExprKind::Map(entries) => Node::Map(
                entries
                    .iter()
                    .map(|(k, v)| (self.expr(k), self.expr(v)))
                    .collect(),
            ),
            ExprKind::Comprehension {
                element,
                names,
                iterable,
                condition,
            } => {
                let iterable = self.expr(iterable);
                let mark = self.mark();
                let names = names.iter().map(|n| self.bind(&n.text)).collect();
                let condition = condition.as_ref().map(|c| self.expr(c));
                let element = self.expr(element);
                self.restore(mark);
                Node::Comprehension(Box::new(ComprehensionCode {
                    element,
                    names,
                    iterable,
                    condition,
                }))
            }
            ExprKind::Lambda { params, body, .. } => {
                self.frames.push(Frame {
                    scope: Vec::new(),
                    slots: 0,
                    captures: Vec::new(),
                });
                for p in params {
                    self.bind(&p.name.text);
                }
                let body = self.expr(body);
                let frame = self.frames.pop().expect("the lambda's frame");
                Node::Lambda(Rc::new(LambdaCode {
                    slots: frame.slots as usize,
                    params: params.len(),
                    captures: frame.captures,
                    body,
                }))
            }
            ExprKind::Is { value, pattern } => {
                let v = self.expr(value);
                let mark = self.mark();
                let p = self.pattern(pattern);
                self.restore(mark);
                Node::Is(Box::new(v), Box::new(p))
            }
            ExprKind::Conditional {
                then,
                condition,
                otherwise,
            } => {
                let c = self.expr(condition);
                let t = self.expr(then);
                let o = self.expr(otherwise);
                Node::Conditional(Box::new([c, t, o]))
            }
            ExprKind::Binary { op, left, right } => {
                let l = self.expr(left);
                let r = self.expr(right);
                match *op {
                    "and" => Node::And(Box::new(l), Box::new(r)),
                    "or" => Node::Or(Box::new(l), Box::new(r), yields_option(right)),
                    other => match Op::of(other) {
                        Some(op) => Node::Binary(op, Box::new(l), Box::new(r)),
                        None => Node::Fault(format!("internal: operator {other}")),
                    },
                }
            }
            ExprKind::Unary { op, operand } => {
                let v = self.expr(operand);
                match *op {
                    "not" => Node::Not(Box::new(v)),
                    "-" => Node::Negate(Box::new(v)),
                    other => Node::Fault(format!("internal: unary {other}")),
                }
            }
            ExprKind::Try { operand, .. } => {
                let is_value = !matches!(
                    &operand.kind,
                    ExprKind::Call { .. } | ExprKind::GenericCall { .. }
                );
                Node::Try(Box::new(self.expr(operand)), is_value)
            }
            ExprKind::Call { callee, arguments } => self.call(callee, &[], arguments),
            ExprKind::GenericCall {
                callee,
                types,
                arguments,
            } => self.call(callee, types, arguments),
            ExprKind::Index { target, index } => {
                let t = self.expr(target);
                Node::Index(Box::new(t), Box::new(self.expr(index)))
            }
            ExprKind::Field { target, name } => self.field(target, &name.text),
            ExprKind::SafeField { target, name } => {
                Node::SafeField(Box::new(self.expr(target)), self.interp.shared(&name.text))
            }
            ExprKind::Error => Node::Fault("internal: expression with a syntax error".into()),
        }
    }

    fn name(&mut self, name: &str) -> Node {
        if let Some(slot) = self.local(name) {
            return Node::Local(slot);
        }
        match self.interp.global(self.module, name) {
            Global::Variant(d) => Node::Variant(d, self.interp.shared(name)),
            Global::Const(c) => Node::Const(c),
            Global::Func(f) => Node::FnRef(f),
            Global::Def(_) | Global::Unknown => match self.interp.variant(self.module, name) {
                Some(d) => Node::Variant(d, self.interp.shared(name)),
                None => Node::Fault(format!("internal: unknown name {name}")),
            },
        }
    }

    fn field(&mut self, target: &Expr, name: &str) -> Node {
        if let ExprKind::Name(n) = &target.kind {
            if self.local(n).is_none() {
                let program = &self.interp.program;
                if let Some(m) = program.module_alias(self.module, n) {
                    return match program.lookup(m, name) {
                        Some((_, Symbol::Const(c))) => Node::Const(c),
                        Some((_, Symbol::Func(f))) => Node::FnRef(f),
                        _ => Node::Fault(format!("internal: unknown {n}.{name}")),
                    };
                }
            }
        }
        if let Some(d) = self.type_target(target) {
            return Node::Variant(d, self.interp.shared(name));
        }
        let t = self.expr(target);
        Node::Field(Box::new(t), self.interp.shared(name), Cell::new(0))
    }

    fn call(&mut self, callee: &Expr, types: &[Type], arguments: &[Argument]) -> Node {
        match &callee.kind {
            ExprKind::Name(n) if self.local(n).is_none() => {
                match self.interp.global(self.module, n) {
                    Global::Variant(d) => self.construct(d, Some(n), arguments),
                    Global::Func(f) => self.call_fn(f, types, arguments),
                    Global::Def(d) => self.construct(d, None, arguments),
                    Global::Const(c) => Node::CallValue(
                        Box::new(Node::Const(c)),
                        arguments.iter().map(|a| self.expr(&a.value)).collect(),
                    ),
                    Global::Unknown => Node::Fault(format!("internal: unknown function {n}")),
                }
            }
            ExprKind::Field { target, name } => {
                if let ExprKind::Name(n) = &target.kind {
                    if self.local(n).is_none() {
                        let program = &self.interp.program;
                        if let Some(m) = program.module_alias(self.module, n) {
                            return match program.lookup(m, &name.text) {
                                Some((_, Symbol::Func(f))) => self.call_fn(f, types, arguments),
                                Some((_, Symbol::Def(d))) => self.construct(d, None, arguments),
                                _ => Node::Fault(format!("internal: unknown {n}.{}", name.text)),
                            };
                        }
                    }
                }
                if let Some(d) = self.type_target(target) {
                    if matches!(self.interp.program.defs[d].kind, DefKind::Enum(_)) {
                        return self.construct(d, Some(&name.text), arguments);
                    }
                    return match self.interp.method(&self.interp.def_names[d], &name.text) {
                        Some(f) => self.call_fn(f, &[], arguments),
                        None => Node::Fault(format!(
                            "internal: no function {}.{}",
                            self.interp.program.defs[d].name, name.text
                        )),
                    };
                }
                let receiver = self.expr(target);
                let place = if is_place(target) {
                    self.place(target)
                } else {
                    None
                };
                let args = arguments
                    .iter()
                    .map(|a| MethodArg {
                        name: a.name.as_ref().map(|n| n.text.clone()),
                        value: self.expr(&a.value),
                        place: if is_place(&a.value) {
                            self.place(&a.value)
                        } else {
                            None
                        },
                    })
                    .collect::<Vec<MethodArg>>();
                Node::Method(Box::new(MethodCode {
                    receiver,
                    name: name.text.clone(),
                    positional: args.iter().all(|a| a.name.is_none()),
                    args,
                    place,
                    cache: Cell::new((0, 0)),
                    pin: std::cell::RefCell::new(None),
                }))
            }
            _ => {
                let f = self.expr(callee);
                Node::CallValue(
                    Box::new(f),
                    arguments.iter().map(|a| self.expr(&a.value)).collect(),
                )
            }
        }
    }

    /// A call of a known function: each argument is matched to its parameter here.
    fn call_fn(&mut self, f: usize, types: &[Type], arguments: &[Argument]) -> Node {
        let def = &self.interp.program.funcs[f];
        let offset = usize::from(def.self_param.is_some());
        let count = offset + def.params.len();
        let mut position = offset;
        let mut args = Vec::with_capacity(arguments.len());
        for argument in arguments {
            let target = match &argument.name {
                Some(n) => match def.params.iter().position(|p| p.name == n.text) {
                    Some(i) => i + offset,
                    None => return Node::Fault(format!("internal: no parameter {}", n.text)),
                },
                None => {
                    position += 1;
                    position - 1
                }
            };
            if target >= count {
                return Node::Fault("internal: too many arguments".into());
            }
            let var = def.params[target - offset].var;
            let place = if var && is_place(&argument.value) {
                self.place(&argument.value)
            } else {
                None
            };
            args.push(ArgCode {
                target,
                value: self.expr(&argument.value),
                place,
            });
        }
        Node::Call(Box::new(CallCode {
            f,
            args,
            types: types.iter().map(|t| self.type_code(t)).collect(),
            count,
        }))
    }

    fn construct(&mut self, d: usize, variant: Option<&str>, arguments: &[Argument]) -> Node {
        let def = &self.interp.program.defs[d];
        match (&def.kind, variant) {
            (DefKind::Enum(vs), Some(v)) => {
                let Some(vd) = vs.iter().find(|x| x.name == v) else {
                    return Node::Fault(format!("internal: no variant {v}"));
                };
                let fields = vd.fields.len();
                let mut position = 0;
                let mut args = Vec::new();
                for a in arguments {
                    let i = match &a.name {
                        Some(n) => vd
                            .fields
                            .iter()
                            .position(|(f, _)| *f == n.text)
                            .unwrap_or(0),
                        None => {
                            position += 1;
                            position - 1
                        }
                    };
                    args.push((i, self.expr(&a.value)));
                }
                Node::Construct(Box::new(ConstructCode {
                    def: d,
                    variant: Some(self.interp.shared(v)),
                    args,
                    fields,
                }))
            }
            (DefKind::Struct(fs) | DefKind::Error(fs), _) => {
                let fields = fs.len();
                let mut position = 0;
                let mut args = Vec::new();
                for a in arguments {
                    let i = match &a.name {
                        Some(n) => fs.iter().position(|f| f.name == n.text).unwrap_or(0),
                        None => {
                            position += 1;
                            position - 1
                        }
                    };
                    args.push((i, self.expr(&a.value)));
                }
                Node::Construct(Box::new(ConstructCode {
                    def: d,
                    variant: None,
                    args,
                    fields,
                }))
            }
            _ => Node::Fault(format!("internal: {} cannot be constructed", def.name)),
        }
    }

    // ---------------------------------------------------------------- patterns

    /// Binds a name of a pattern; an alternative of `|` reuses the slot of the same name.
    fn bind_pattern(&mut self, name: &str, start: usize) -> Slot {
        let frame = self.frames.last().expect("a frame");
        if let Some((_, slot)) = frame.scope[start..].iter().find(|(n, _)| n == name) {
            return *slot;
        }
        self.bind(name)
    }

    /// A type argument as the call resolves it (`Interp::rt_type`): the names of the module
    /// are resolved here, the type parameters of the caller at the call.
    fn type_code(&self, ty: &Type) -> TypeCode {
        let name = ty.path.last().map(|n| n.text.as_str()).unwrap_or("");
        let args = ty
            .arguments
            .iter()
            .map(|a| match a {
                TypeArgument::Type(t) => self.type_code(t),
                _ => TypeCode {
                    generic: None,
                    base: TypeBase::Unknown,
                    args: Vec::new(),
                },
            })
            .collect();
        let builtin = match name {
            "Int" => Some(TypeBase::Int),
            "Float" => Some(TypeBase::Float),
            "Bool" => Some(TypeBase::Bool),
            "Text" => Some(TypeBase::Text),
            "Bytes" => Some(TypeBase::Bytes),
            "Path" => Some(TypeBase::Path),
            "List" => Some(TypeBase::List),
            "Map" => Some(TypeBase::Map),
            "Set" => Some(TypeBase::Set),
            "Tuple" => Some(TypeBase::Tuple),
            _ => None,
        };
        let program = &self.interp.program;
        let base = match builtin {
            Some(b) if ty.path.len() == 1 => b,
            _ => {
                let module = if ty.path.len() > 1 {
                    program
                        .module_alias(self.module, &ty.path[0].text)
                        .unwrap_or(self.module)
                } else {
                    self.module
                };
                match program.lookup(module, name) {
                    Some((_, Symbol::Def(d))) => TypeBase::Def(d),
                    _ => TypeBase::Unknown,
                }
            }
        };
        TypeCode {
            generic: (ty.path.len() == 1).then(|| name.to_string()),
            base,
            args,
        }
    }

    /// The declared type a written path names in the module: `Node`, or `trees.Node` through
    /// a module imported under that name.
    fn named_def(&self, path: &[Name]) -> Option<crate::types::DefId> {
        let program = &self.interp.program;
        let found = match path {
            [name] => program.lookup(self.module, &name.text),
            [module, name] => {
                let m = program.module_alias(self.module, &module.text)?;
                program.lookup(m, &name.text)
            }
            _ => None,
        };
        match found {
            Some((_, Symbol::Def(d))) => Some(d),
            _ => None,
        }
    }

    /// The declared type that the target of `target.name` names, when it is not a value:
    /// `Mark` or `trees.Mark`, as in `trees.Mark.Leaf` or `trees.Node.make()`.
    fn type_target(&mut self, target: &Expr) -> Option<crate::types::DefId> {
        let path = match &target.kind {
            ExprKind::Name(n) if self.local(n).is_none() => vec![Name {
                text: n.clone(),
                span: target.span,
            }],
            ExprKind::Field {
                target: inner,
                name,
            } => match &inner.kind {
                ExprKind::Name(n)
                    if self.local(n).is_none()
                        && self.interp.program.module_alias(self.module, n).is_some() =>
                {
                    vec![
                        Name {
                            text: n.clone(),
                            span: inner.span,
                        },
                        name.clone(),
                    ]
                }
                _ => return None,
            },
            _ => return None,
        };
        self.named_def(&path)
    }

    /// The type of a handler (`on trees.Problem`) as the identity its errors carry.
    fn handler_type(&self, ty: &Type) -> String {
        match self.named_def(&ty.path) {
            Some(d) => self.interp.def_names[d].to_string(),
            None => ty.path.last().map(|n| n.text.clone()).unwrap_or_default(),
        }
    }

    /// The name a pattern compares: the identity of a struct or error type it names, otherwise
    /// the variant, constant or binding name as written.
    fn pattern_name(&self, path: &[Name]) -> Rc<str> {
        let last = path.last().map(|n| n.text.as_str()).unwrap_or_default();
        let variant = path.len() == 1 && self.interp.variant(self.module, last).is_some();
        if !variant {
            if let Some(d) = self.named_def(path) {
                let program = &self.interp.program;
                if matches!(program.defs[d].kind, DefKind::Struct(_) | DefKind::Error(_)) {
                    return Rc::clone(&self.interp.def_names[d]);
                }
            }
        }
        self.interp.shared(last)
    }

    fn pattern(&mut self, pattern: &Pattern) -> Pat {
        let start = self.frames.last().map_or(0, |f| f.scope.len());
        self.pattern_from(pattern, start)
    }

    fn pattern_from(&mut self, pattern: &Pattern, start: usize) -> Pat {
        match &pattern.kind {
            PatternKind::Wildcard => Pat::Wildcard,
            PatternKind::Literal(Literal::Int(t)) => Pat::Int(crate::interp::parse_int(t)),
            PatternKind::Literal(Literal::Bool(b)) => Pat::Bool(*b),
            PatternKind::Literal(Literal::Text(parts)) => Pat::Text(
                parts
                    .iter()
                    .filter_map(|p| match p {
                        TextPart::Literal(l) => Some(l.as_str()),
                        TextPart::Interpolation(_) => None,
                    })
                    .collect(),
            ),
            PatternKind::Name(names) => {
                let last = names.last().map(|n| n.text.clone()).unwrap_or_default();
                let program = &self.interp.program;
                let constant = match program.lookup(self.module, &last) {
                    Some((_, Symbol::Const(c))) => Some(c),
                    _ => None,
                };
                let fallback = if names.len() == 1
                    && !last.chars().next().is_some_and(|c| c.is_uppercase())
                    && self.interp.variant(self.module, &last).is_none()
                {
                    match constant {
                        Some(c) => NameFallback::Const(c),
                        None => NameFallback::Bind(self.bind_pattern(&last, start)),
                    }
                } else {
                    match constant {
                        Some(c) => NameFallback::Const(c),
                        None => NameFallback::No,
                    }
                };
                Pat::Name {
                    last: self.pattern_name(names),
                    fallback,
                    known: Cell::new(0),
                    pin: std::cell::RefCell::new(None),
                }
            }
            PatternKind::Constructor { name, arguments } => Pat::Constructor {
                last: self.pattern_name(name),
                args: arguments
                    .iter()
                    .map(|(n, p)| {
                        (
                            n.as_ref().map(|n| n.text.clone()),
                            self.pattern_from(p, start),
                        )
                    })
                    .collect(),
            },
            PatternKind::List(elements) => {
                let rest = elements.iter().any(|e| matches!(e, ListElement::Rest(_)));
                let fixed = elements.len() - usize::from(rest);
                let elements = elements
                    .iter()
                    .map(|e| match e {
                        ListElement::Pattern(p) => ListPat::Pattern(self.pattern_from(p, start)),
                        ListElement::Rest(n) => ListPat::Rest(self.bind_pattern(&n.text, start)),
                    })
                    .collect();
                Pat::List {
                    elements,
                    fixed,
                    rest,
                }
            }
            PatternKind::Tuple(ps) => {
                Pat::Tuple(ps.iter().map(|p| self.pattern_from(p, start)).collect())
            }
            PatternKind::Bind { pattern, name } => {
                let inner = self.pattern_from(pattern, start);
                Pat::Bind(Box::new(inner), self.bind_pattern(&name.text, start))
            }
            PatternKind::Or(ps) => {
                Pat::Or(ps.iter().map(|p| self.pattern_from(p, start)).collect())
            }
        }
    }
}

pub(crate) fn is_place(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Name(_) | ExprKind::SelfValue => true,
        ExprKind::Field { target, .. } | ExprKind::Index { target, .. } => is_place(target),
        ExprKind::Paren(e) => is_place(e),
        _ => false,
    }
}

/// Whether the right operand of `or` is itself an `Option` (`Option or Option`).
fn yields_option(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Name(n) => n == "none",
        ExprKind::Call { callee, .. } => matches!(&callee.kind, ExprKind::Name(n) if n == "some"),
        ExprKind::Paren(e) => yields_option(e),
        _ => false,
    }
}

fn find_handler(simple: &Simple) -> Option<(&Type, &Option<Name>)> {
    fn search(e: &Expr) -> Option<(&Type, &Option<Name>)> {
        match &e.kind {
            ExprKind::Try {
                handler: Some((t, n)),
                ..
            } => Some((t, n)),
            ExprKind::Try { operand, .. } | ExprKind::Paren(operand) => search(operand),
            _ => None,
        }
    }
    match simple {
        Simple::Let { value, .. } | Simple::Assign { value, .. } => search(value),
        Simple::Return(Some(e)) | Simple::Expr(e) => search(e),
        _ => None,
    }
}

/// The parts of `name + a + b`, when `value` has that form and no part uses `name`.
fn concat_parts<'e>(value: &'e Expr, name: &str, parts: &mut Vec<&'e Expr>) -> bool {
    match &value.kind {
        ExprKind::Binary {
            op: "+",
            left,
            right,
        } => {
            if mentions(right, name) {
                return false;
            }
            if !concat_parts(left, name, parts) {
                return false;
            }
            parts.push(right);
            true
        }
        ExprKind::Paren(e) => concat_parts(e, name, parts),
        ExprKind::Name(n) => n == name,
        _ => false,
    }
}

/// Whether an expression uses a local name, anywhere inside it.
fn mentions(e: &Expr, name: &str) -> bool {
    let m = |x: &Expr| mentions(x, name);
    match &e.kind {
        ExprKind::Name(n) => n == name,
        ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) | ExprKind::Error => false,
        ExprKind::SelfValue => name == "self",
        ExprKind::Text(parts) => parts.iter().any(|p| match p {
            TextPart::Literal(_) => false,
            TextPart::Interpolation(x) => m(x),
        }),
        ExprKind::Paren(x) => m(x),
        ExprKind::Tuple(xs) | ExprKind::List(xs) => xs.iter().any(m),
        ExprKind::Map(kv) => kv.iter().any(|(k, v)| m(k) || m(v)),
        ExprKind::Comprehension {
            element,
            iterable,
            condition,
            ..
        } => m(element) || m(iterable) || condition.as_deref().is_some_and(m),
        ExprKind::Lambda { body, .. } => m(body),
        // A pattern can name a constant but not read a local.
        ExprKind::Is { value, .. } => m(value),
        ExprKind::Conditional {
            then,
            condition,
            otherwise,
        } => m(then) || m(condition) || m(otherwise),
        ExprKind::Binary { left, right, .. } => m(left) || m(right),
        ExprKind::Unary { operand, .. } | ExprKind::Try { operand, .. } => m(operand),
        ExprKind::Call { callee, arguments }
        | ExprKind::GenericCall {
            callee, arguments, ..
        } => m(callee) || arguments.iter().any(|a| m(&a.value)),
        ExprKind::Index { target, index } => m(target) || m(index),
        ExprKind::Field { target, .. } | ExprKind::SafeField { target, .. } => m(target),
    }
}

/// Items that are all literal values become one value; otherwise the node that builds them.
fn constant_items(
    items: Vec<Node>,
    node: fn(Vec<Node>) -> Node,
    value: fn(Vec<Value>) -> Value,
) -> Node {
    if items.is_empty() || !items.iter().all(|n| matches!(n, Node::Value(_))) {
        return node(items);
    }
    let values = items
        .into_iter()
        .map(|n| match n {
            Node::Value(v) => v,
            _ => Value::Unit,
        })
        .collect();
    Node::Value(value(values))
}
