//! The syntax tree. Every node that a diagnostic may point at carries a span.

use crate::diag::Span;

#[derive(Clone, Debug, PartialEq)]
pub struct Name {
    pub text: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct File {
    pub unsafe_module: bool,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Import(Import),
    Declaration(Declaration),
    Extend(Extend),
    Test(Test),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Import {
    pub module: Vec<Name>,
    /// `import a.b as c`
    pub alias: Option<Name>,
    /// `from a.b import x, y`; empty for `import`.
    pub names: Vec<Name>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Declaration {
    pub public: bool,
    pub kind: DeclarationKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DeclarationKind {
    Function(Function),
    External(Signature),
    Struct(Struct),
    Enum(Enum),
    Error(ErrorDecl),
    Interface(Interface),
    Constant(Constant),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Signature {
    pub name: Name,
    pub generics: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub returns: Option<Type>,
    pub fails: Vec<Type>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    pub signature: Signature,
    pub body: Block,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GenericParam {
    pub name: Name,
    pub bounds: Vec<Type>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Param {
    SelfParam {
        var: bool,
        span: Span,
    },
    /// `named port as Int = 8080` (RFC 0001)
    Named {
        name: Name,
        ty: Type,
        default: Option<Expr>,
    },
    Plain {
        var: bool,
        name: Name,
        ty: Type,
        default: Option<Expr>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub public: bool,
    pub var: bool,
    pub name: Name,
    pub ty: Type,
    pub default: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Struct {
    pub name: Name,
    pub generics: Vec<GenericParam>,
    /// None for `struct Name` without a body.
    pub fields: Option<Vec<Field>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ErrorDecl {
    pub name: Name,
    pub fields: Option<Vec<Field>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Enum {
    pub name: Name,
    pub generics: Vec<GenericParam>,
    pub variants: Vec<Variant>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub name: Name,
    pub fields: Vec<(Name, Type)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Interface {
    pub name: Name,
    pub generics: Vec<GenericParam>,
    /// Methods; a block is a default implementation.
    pub members: Vec<(Signature, Option<Block>)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Constant {
    pub name: Name,
    pub ty: Option<Type>,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Extend {
    pub generics: Vec<GenericParam>,
    pub target: Type,
    pub interface: Option<Type>,
    pub members: Vec<Member>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    pub public: bool,
    pub external: bool,
    pub signature: Signature,
    /// None for an external member.
    pub body: Option<Block>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Test {
    pub description: Vec<TextPart>,
    pub body: Block,
    pub span: Span,
}

pub type Block = Vec<Statement>;

#[derive(Clone, Debug, PartialEq)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StatementKind {
    /// A simple statement; `handler` is the block of its inline handler.
    Simple {
        statement: Simple,
        handler: Option<Block>,
    },
    If {
        branches: Vec<(Expr, Block)>,
        otherwise: Option<Block>,
    },
    For {
        names: Vec<Name>,
        iterable: Expr,
        body: Block,
    },
    While {
        condition: Expr,
        body: Block,
    },
    Match {
        subject: Expr,
        cases: Vec<Case>,
    },
    Try {
        body: Block,
        handlers: Vec<Handler>,
    },
    Using {
        name: Name,
        value: Expr,
        body: Block,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Simple {
    Let {
        var: bool,
        name: Name,
        ty: Option<Type>,
        value: Expr,
    },
    Assign {
        target: Expr,
        value: Expr,
    },
    Return(Option<Expr>),
    Fail(Expr),
    Assert(Expr),
    Break,
    Continue,
    Pass,
    Expr(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Case {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Block,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Handler {
    pub ty: Type,
    pub name: Option<Name>,
    pub body: Block,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TextPart {
    Literal(String),
    Interpolation(Expr),
}

/// A lambda parameter; the type may be left out where an expected function type gives it.
#[derive(Clone, Debug, PartialEq)]
pub struct LambdaParam {
    pub named: bool,
    pub name: Name,
    pub ty: Option<Type>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Argument {
    pub name: Option<Name>,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    Int(String),
    Float(String),
    Text(Vec<TextPart>),
    Bool(bool),
    SelfValue,
    Name(String),
    Paren(Box<Expr>),
    Tuple(Vec<Expr>),
    List(Vec<Expr>),
    Comprehension {
        element: Box<Expr>,
        names: Vec<Name>,
        iterable: Box<Expr>,
        condition: Option<Box<Expr>>,
    },
    Map(Vec<(Expr, Expr)>),
    Lambda {
        params: Vec<LambdaParam>,
        returns: Option<Type>,
        fails: Vec<Type>,
        body: Box<Expr>,
    },
    /// `value is pattern` (RFC 0002)
    Is {
        value: Box<Expr>,
        pattern: Box<Pattern>,
    },
    Conditional {
        then: Box<Expr>,
        condition: Box<Expr>,
        otherwise: Box<Expr>,
    },
    Binary {
        op: &'static str,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Unary {
        op: &'static str,
        operand: Box<Expr>,
    },
    /// `try X` with an optional inline handler `on Type as name`.
    Try {
        operand: Box<Expr>,
        handler: Option<(Type, Option<Name>)>,
    },
    Call {
        callee: Box<Expr>,
        arguments: Vec<Argument>,
    },
    GenericCall {
        callee: Box<Expr>,
        types: Vec<Type>,
        arguments: Vec<Argument>,
    },
    Index {
        target: Box<Expr>,
        index: Box<Expr>,
    },
    Field {
        target: Box<Expr>,
        name: Name,
    },
    SafeField {
        target: Box<Expr>,
        name: Name,
    },
    /// Produced only after a syntax error, so that parsing can continue.
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    Int(String),
    Text(Vec<TextPart>),
    Bool(bool),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ListElement {
    Rest(Name),
    Pattern(Pattern),
}

#[derive(Clone, Debug, PartialEq)]
pub enum PatternKind {
    Wildcard,
    Literal(Literal),
    /// A binding, a variant, or `Enum.Variant`: resolved by the checker.
    Name(Vec<Name>),
    Constructor {
        name: Vec<Name>,
        arguments: Vec<(Option<Name>, Pattern)>,
    },
    List(Vec<ListElement>),
    Tuple(Vec<Pattern>),
    Bind {
        pattern: Box<Pattern>,
        name: Name,
    },
    Or(Vec<Pattern>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Type {
    pub path: Vec<Name>,
    pub arguments: Vec<TypeArgument>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeArgument {
    Type(Type),
    /// `(A, named b as B) -> R fails with E`
    Shape {
        params: Vec<(Option<Name>, Type)>,
        returns: Option<Box<Type>>,
        fails: Vec<Type>,
    },
}
