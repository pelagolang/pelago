//! What the tree knows about indentation that brackets do not show: a `switch` arm's body is
//! not inside brackets, so its statements sit one level deeper than the `case` line.
//!
//! [`arm_levels`] maps the start offset of every statement, block tail and `switch` arm to the
//! number of `switch` arm bodies around it. The layout adds that to the bracket depth, which
//! replaces the levels the author gave such lines and so corrects a line indented too far.

use std::collections::HashMap;

use crate::ast::*;
use crate::visit::*;

pub fn arm_levels(file: &File) -> HashMap<u32, usize> {
    let mut v = Levels {
        arms: 0,
        map: HashMap::new(),
    };
    v.visit_file(file);
    v.map
}

struct Levels {
    arms: usize,
    map: HashMap<u32, usize>,
}

impl Levels {
    fn body(&mut self, b: &Block) {
        for s in &b.stmts {
            self.map.insert(s.span.start, self.arms);
            self.visit_stmt(s);
        }
        if let Some(t) = &b.tail {
            self.map.insert(t.span.start, self.arms);
            self.visit_expr(t);
        }
    }
}

impl Visitor for Levels {
    fn visit_file(&mut self, f: &File) {
        for s in &f.stmts {
            self.map.insert(s.span.start, 0);
            self.visit_stmt(s);
        }
    }

    fn visit_block(&mut self, b: &Block) {
        self.body(b);
    }

    fn visit_expr(&mut self, e: &Expr) {
        let ExprKind::Switch(s) = &e.kind else {
            walk_expr(self, e);
            return;
        };
        self.visit_expr(&s.scrutinee);
        for arm in &s.arms {
            self.map.insert(arm.span.start, self.arms);
            for p in &arm.patterns {
                self.visit_pattern(p);
            }
            if let Some(g) = &arm.guard {
                self.visit_expr(g);
            }
            // `case x: { ... }`: the braces already show the level.
            let braced = match (arm.body.stmts.as_slice(), &arm.body.tail) {
                ([], Some(t)) => matches!(t.kind, ExprKind::Block(_)),
                ([s], None) => {
                    matches!(&s.kind, StmtKind::Expr(e) if matches!(e.kind, ExprKind::Block(_)))
                }
                _ => false,
            };
            let step = usize::from(!braced);
            self.arms += step;
            self.body(&arm.body);
            self.arms -= step;
        }
    }
}
