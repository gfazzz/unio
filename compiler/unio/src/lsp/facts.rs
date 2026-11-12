//! What the checker records for the language server (T-10): the type of every expression it
//! checks, every local binding with its type, and which binding each local name refers to.
//! Nothing is recorded unless the analysis of the language server asks for it on its thread
//! ([`start`]); `unio check` and the other tools record nothing.

use std::cell::RefCell;

use crate::diag::Span;
use crate::program::ModId;
use crate::types::Ty;

/// A local binding: a `let`, `var`, parameter, loop name, or a name bound by a pattern.
#[derive(Clone, Debug)]
pub struct Local {
    pub module: ModId,
    pub name: String,
    /// The name where it is bound.
    pub span: Span,
    pub ty: Ty,
    pub param: bool,
}

/// The facts of one check of a program.
#[derive(Default, Debug)]
pub struct Facts {
    /// Every checked expression with its type, in the order checked.
    pub types: Vec<(ModId, Span, Ty)>,
    pub locals: Vec<Local>,
    /// A local name used at the first span, bound at the second.
    pub local_refs: Vec<(ModId, Span, Span)>,
}

thread_local! {
    static FACTS: RefCell<Option<Facts>> = const { RefCell::new(None) };
}

/// Starts recording on this thread.
pub fn start() {
    FACTS.with(|f| *f.borrow_mut() = Some(Facts::default()));
}

/// Ends recording on this thread and gives what was recorded.
pub fn take() -> Option<Facts> {
    FACTS.with(|f| f.borrow_mut().take())
}

/// Whether the checker records facts on this thread.
pub(crate) fn recording() -> bool {
    FACTS.with(|f| f.borrow().is_some())
}

/// What one run of the checker noted, with types that may still hold inference variables.
#[derive(Default)]
pub(crate) struct Noted {
    /// Whether the language server records on this thread, taken once per run.
    pub(crate) on: bool,
    types: Vec<(ModId, Span, Ty)>,
    locals: Vec<Local>,
    local_refs: Vec<(ModId, Span, Span)>,
}

impl Noted {
    pub(crate) fn new() -> Noted {
        Noted {
            on: recording(),
            ..Noted::default()
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.types.is_empty() && self.locals.is_empty() && self.local_refs.is_empty()
    }

    pub(crate) fn local(&mut self, module: ModId, name: &str, span: Span, ty: &Ty, param: bool) {
        self.locals.push(Local {
            module,
            name: name.to_string(),
            span,
            ty: ty.clone(),
            param,
        });
    }

    pub(crate) fn expr(&mut self, module: ModId, span: Span, ty: &Ty, local: Option<Span>) {
        self.types.push((module, span, ty.clone()));
        if let Some(bound) = local {
            self.local_refs.push((module, span, bound));
        }
    }

    /// Adds what was noted to the facts of this thread, with the types resolved.
    pub(crate) fn finish(self, resolve: impl Fn(&Ty) -> Ty) {
        FACTS.with(|f| {
            if let Some(facts) = f.borrow_mut().as_mut() {
                facts
                    .types
                    .extend(self.types.into_iter().map(|(m, s, t)| (m, s, resolve(&t))));
                facts.locals.extend(self.locals.into_iter().map(|mut l| {
                    l.ty = resolve(&l.ty);
                    l
                }));
                facts.local_refs.extend(self.local_refs);
            }
        });
    }
}
