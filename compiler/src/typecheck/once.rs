//! Which closures can be called only once (`docs/design/functions.md`, section 19).
//!
//! A closure that consumes a `[sink x]` capture runs its body on a value that is gone afterwards,
//! so its type is `once fn`. The checker looks for the consuming forms that are written out:
//! a `sink x` argument, a `!!` call on `x` (or on something inside it), and `x` as the value the
//! closure returns. A move it misses (`let t = x`) is caught later by the value pass, which then
//! asks for a `once fn` type from the context.

use super::*;
use crate::visit::{self, Visitor};

impl<'p> Checker<'p> {
    /// Does the body of `c` consume one of its `[sink]` captures?
    pub(crate) fn closure_consumes(&self, c: &Closure) -> bool {
        let caps: Vec<DefId> = c
            .captures
            .iter()
            .filter(|cap| cap.mode == Some(Mode::Sink))
            .filter_map(|cap| self.res.use_at(self.file, cap.name.span.start))
            .collect();
        if caps.is_empty() {
            return false;
        }
        let mut f = Finder {
            ck: self,
            caps: &caps,
            found: false,
        };
        if f.rooted(&c.body) {
            return true; // `[sink s] () ~> s`
        }
        f.visit_expr(&c.body);
        f.found
    }
}

struct Finder<'a, 'p> {
    ck: &'a Checker<'p>,
    caps: &'a [DefId],
    found: bool,
}

impl Finder<'_, '_> {
    /// Is `e` a captured name, or a place inside one, or a block whose value is one?
    fn rooted(&self, e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Paren(i) => self.rooted(i),
            ExprKind::Name(n) => self
                .ck
                .res
                .use_at(self.ck.file, n.span.start)
                .is_some_and(|d| self.caps.contains(&d)),
            ExprKind::Field { base, .. }
            | ExprKind::TupleIndex { base, .. }
            | ExprKind::Index { base, .. } => self.rooted(base),
            ExprKind::Block(b) => b.tail.as_deref().is_some_and(|t| self.rooted(t)),
            _ => false,
        }
    }
}

impl Visitor for Finder<'_, '_> {
    fn visit_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Call { callee, args, mark } => {
                if args
                    .iter()
                    .any(|a| a.mode == Some(Mode::Sink) && self.rooted(&a.value))
                {
                    self.found = true;
                }
                if *mark == CallMark::Consumes {
                    let recv = match &callee.kind {
                        ExprKind::Field { base, .. } => base,
                        _ => callee,
                    };
                    if self.rooted(recv) {
                        self.found = true;
                    }
                }
            }
            ExprKind::Return(Some(v)) if self.rooted(v) => self.found = true,
            _ => {}
        }
        visit::walk_expr(self, e);
    }
}
