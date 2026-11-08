//! Types of the static semantics (docs/semantics.md, section 2).
//!
//! `Unknown` is the type of an expression the checker could not determine, after an error has
//! been reported or where inference is not complete. It is compatible with every type, so that
//! one mistake is reported once and not again at every use.

use std::collections::HashMap;

/// A declared type: struct, enum, error or interface. Index into `Program::defs`.
pub type DefId = usize;
/// A generic parameter. Index into `Program::params`.
pub type ParamId = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Prim {
    Int,
    Float,
    Bool,
    Text,
    Bytes,
    Path,
}

impl Prim {
    pub fn name(self) -> &'static str {
        match self {
            Prim::Int => "Int",
            Prim::Float => "Float",
            Prim::Bool => "Bool",
            Prim::Text => "Text",
            Prim::Bytes => "Bytes",
            Prim::Path => "Path",
        }
    }

    pub fn from_name(name: &str) -> Option<Prim> {
        Some(match name {
            "Int" => Prim::Int,
            "Float" => Prim::Float,
            "Bool" => Prim::Bool,
            "Text" => Prim::Text,
            "Bytes" => Prim::Bytes,
            "Path" => Prim::Path,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FnTy {
    /// Parameters with their labels: a label is required at calls (RFC 0001).
    pub params: Vec<(Option<String>, Ty)>,
    pub ret: Ty,
    pub fails: Vec<Ty>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Unknown,
    Never,
    /// The result of a function without `returns`.
    Unit,
    Prim(Prim),
    List(Box<Ty>),
    Map(Box<Ty>, Box<Ty>),
    Set(Box<Ty>),
    Tuple(Vec<Ty>),
    Func(Box<FnTy>),
    Named(DefId, Vec<Ty>),
    Param(ParamId),
    /// An inference variable of one call.
    Var(usize),
    /// `Self` inside an interface.
    SelfTy,
}

impl Ty {
    /// Whether the type is IEEE `Float` or is built from it through type arguments, as
    /// `Tuple[Float, Int]` or `Option[Float]`: such a type has neither `Hash` nor a total
    /// `Order` (L-9, L-32), and a key or a bound that needs one is E0045. A struct with a
    /// `Float` field is not counted: the struct itself lacks the interface (E0062).
    pub fn float_inside(&self) -> bool {
        match self {
            Ty::Prim(Prim::Float) => true,
            Ty::List(t) | Ty::Set(t) => t.float_inside(),
            Ty::Map(k, v) => k.float_inside() || v.float_inside(),
            Ty::Tuple(ts) | Ty::Named(_, ts) => ts.iter().any(Ty::float_inside),
            _ => false,
        }
    }

    pub fn int() -> Ty {
        Ty::Prim(Prim::Int)
    }
    pub fn float() -> Ty {
        Ty::Prim(Prim::Float)
    }
    pub fn bool() -> Ty {
        Ty::Prim(Prim::Bool)
    }
    pub fn text() -> Ty {
        Ty::Prim(Prim::Text)
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Ty::Unknown)
    }

    /// Whether the type contains Unknown or an unbound variable anywhere.
    pub fn is_partial(&self) -> bool {
        match self {
            Ty::Unknown | Ty::Var(_) => true,
            Ty::List(t) | Ty::Set(t) => t.is_partial(),
            Ty::Map(k, v) => k.is_partial() || v.is_partial(),
            Ty::Tuple(ts) | Ty::Named(_, ts) => ts.iter().any(Ty::is_partial),
            Ty::Func(f) => f.params.iter().any(|(_, t)| t.is_partial()) || f.ret.is_partial(),
            _ => false,
        }
    }

    /// Replaces generic parameters and Self.
    pub fn subst(&self, params: &HashMap<ParamId, Ty>, self_ty: Option<&Ty>) -> Ty {
        match self {
            Ty::Param(p) => params.get(p).cloned().unwrap_or(Ty::Param(*p)),
            Ty::SelfTy => self_ty.cloned().unwrap_or(Ty::SelfTy),
            Ty::List(t) => Ty::List(Box::new(t.subst(params, self_ty))),
            Ty::Set(t) => Ty::Set(Box::new(t.subst(params, self_ty))),
            Ty::Map(k, v) => Ty::Map(
                Box::new(k.subst(params, self_ty)),
                Box::new(v.subst(params, self_ty)),
            ),
            Ty::Tuple(ts) => Ty::Tuple(ts.iter().map(|t| t.subst(params, self_ty)).collect()),
            Ty::Named(d, ts) => {
                Ty::Named(*d, ts.iter().map(|t| t.subst(params, self_ty)).collect())
            }
            Ty::Func(f) => Ty::Func(Box::new(FnTy {
                params: f
                    .params
                    .iter()
                    .map(|(l, t)| (l.clone(), t.subst(params, self_ty)))
                    .collect(),
                ret: f.ret.subst(params, self_ty),
                fails: f.fails.iter().map(|t| t.subst(params, self_ty)).collect(),
            })),
            other => other.clone(),
        }
    }

    /// Replaces inference variables by their bindings; unbound ones become `unbound`.
    pub fn resolve_vars(&self, vars: &[Option<Ty>], unbound: &Ty) -> Ty {
        match self {
            Ty::Var(v) => match vars.get(*v).cloned().flatten() {
                Some(t) => t.resolve_vars(vars, unbound),
                None => unbound.clone(),
            },
            Ty::List(t) => Ty::List(Box::new(t.resolve_vars(vars, unbound))),
            Ty::Set(t) => Ty::Set(Box::new(t.resolve_vars(vars, unbound))),
            Ty::Map(k, v) => Ty::Map(
                Box::new(k.resolve_vars(vars, unbound)),
                Box::new(v.resolve_vars(vars, unbound)),
            ),
            Ty::Tuple(ts) => Ty::Tuple(ts.iter().map(|t| t.resolve_vars(vars, unbound)).collect()),
            Ty::Named(d, ts) => Ty::Named(
                *d,
                ts.iter().map(|t| t.resolve_vars(vars, unbound)).collect(),
            ),
            Ty::Func(f) => Ty::Func(Box::new(FnTy {
                params: f
                    .params
                    .iter()
                    .map(|(l, t)| (l.clone(), t.resolve_vars(vars, unbound)))
                    .collect(),
                ret: f.ret.resolve_vars(vars, unbound),
                fails: f
                    .fails
                    .iter()
                    .map(|t| t.resolve_vars(vars, unbound))
                    .collect(),
            })),
            other => other.clone(),
        }
    }

    pub fn mentions_self(&self) -> bool {
        match self {
            Ty::SelfTy => true,
            Ty::List(t) | Ty::Set(t) => t.mentions_self(),
            Ty::Map(k, v) => k.mentions_self() || v.mentions_self(),
            Ty::Tuple(ts) | Ty::Named(_, ts) => ts.iter().any(Ty::mentions_self),
            Ty::Func(f) => f.params.iter().any(|(_, t)| t.mentions_self()) || f.ret.mentions_self(),
            _ => false,
        }
    }
}
