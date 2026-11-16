//! Wide integers and soft floats as `for` ranges and as `switch` patterns.
//!
//! These numbers live in memory (see `wide.rs` and `softf.rs`), so the counting loop of `for_range` (which
//! keeps its counter in a block parameter) cannot hold them. Here the counter is a slot that the loop steps
//! in place, and each pass gives the body its own copy, which the body cannot change. Patterns compare the
//! scrutinee with the literal or the ends of a range through the same comparisons as the operators.

use super::*;
use crate::types::FloatKind;

/// The two kinds of number kept in memory.
#[derive(Clone, Copy)]
enum MemNum {
    Wide(u32, bool),
    Soft(FloatKind),
}

impl<'a> Lowerer<'a> {
    fn mem_num(&mut self, t: Ty) -> Option<MemNum> {
        if let Some((n, s)) = self.wide_of(t) {
            Some(MemNum::Wide(n, s))
        } else {
            self.soft_of(t).map(MemNum::Soft)
        }
    }

    fn mem_bytes(&self, m: MemNum) -> u32 {
        match m {
            MemNum::Wide(n, _) => 8 * n,
            MemNum::Soft(k) => k.bytes(),
        }
    }

    /// A fresh slot of whole words for the number.
    fn mem_slot(&mut self, m: MemNum) -> Value {
        let words = match m {
            MemNum::Wide(n, _) => n,
            MemNum::Soft(k) => k.words(),
        };
        let s = self.slot_bytes(8 * words, 8);
        for i in 0..words {
            let z = self.i64c(0);
            self.store_at(s, 8 * i, z);
        }
        s
    }

    /// The number at `p` copied to a fresh slot of whole words.
    fn mem_copy(&mut self, p: Value, m: MemNum) -> Value {
        let s = self.mem_slot(m);
        self.copy_bytes(s, p, self.mem_bytes(m));
        s
    }

    /// The value of `e` in a private slot of whole words.
    fn mem_operand(&mut self, e: &'a Expr, m: MemNum) -> Option<Value> {
        let Res::Val(p) = self.expr(e) else {
            return None;
        };
        Some(self.mem_copy(p, m))
    }

    fn mem_cmp(&mut self, op: Punct, a: Value, b: Value, m: MemNum) -> Option<Value> {
        match m {
            MemNum::Wide(n, signed) => self.wide_cmp_ptrs(op, a, b, n, signed),
            MemNum::Soft(k) => self.soft_cmp_ptrs(op, a, b, k),
        }
    }

    // ---- patterns -----------------------------------------------------------------------------------------------

    /// A literal pattern on a wide integer or a soft float: false when `ty` is neither.
    pub(crate) fn test_mem_literal(
        &mut self,
        lit: &'a Expr,
        ptr: Value,
        ty: Ty,
        fail: BlockId,
    ) -> Option<bool> {
        let m = self.mem_num(ty)?;
        let Some(want) = self.mem_operand(lit, m) else {
            return Some(false);
        };
        let have = self.mem_copy(ptr, m);
        if let Some(c) = self.mem_cmp(Punct::EqEq, have, want, m) {
            self.require_at(c, fail);
        }
        Some(true)
    }

    /// A range pattern on a wide integer or a soft float.
    pub(crate) fn test_mem_range(
        &mut self,
        start: Option<&'a Expr>,
        end: Option<&'a Expr>,
        inclusive: bool,
        ptr: Value,
        ty: Ty,
        fail: BlockId,
    ) -> Option<bool> {
        let m = self.mem_num(ty)?;
        let have = self.mem_copy(ptr, m);
        if let Some(lo) = start {
            let Some(lo) = self.mem_operand(lo, m) else {
                return Some(false);
            };
            if let Some(c) = self.mem_cmp(Punct::GtEq, have, lo, m) {
                self.require_at(c, fail);
            }
        }
        if let Some(hi) = end {
            let Some(hi) = self.mem_operand(hi, m) else {
                return Some(false);
            };
            let op = if inclusive { Punct::LtEq } else { Punct::Lt };
            if let Some(c) = self.mem_cmp(op, have, hi, m) {
                self.require_at(c, fail);
            }
        }
        Some(true)
    }

    // ---- `for` ranges -------------------------------------------------------------------------------------------

    /// `for i in a..b` (or `..=`, with `step`) over a wide integer or a soft float; None when the type is neither.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn for_range_mem(
        &mut self,
        label: Option<String>,
        var: Option<DefId>,
        sty: Ty,
        start: &'a Expr,
        end: &'a Expr,
        inclusive: bool,
        step_e: Option<&'a Expr>,
        body: &mut LoopBody<'a, '_>,
        span: Span,
    ) -> Option<Res> {
        let m = self.mem_num(sty)?;
        let cur = self.mem_slot(m);
        let Some(a) = self.mem_operand(start, m) else {
            return Some(Res::Never);
        };
        self.copy_bytes(cur, a, self.mem_bytes(m));
        let Some(b) = self.mem_operand(end, m) else {
            return Some(Res::Never);
        };
        let step = match step_e {
            Some(s) => match self.mem_operand(s, m) {
                Some(p) => Some(p),
                None => return Some(Res::Never),
            },
            None => None,
        };
        // The copy the body sees.
        let iv = self.slot_for(sty);
        let head = self.new_block(&[]);
        let body_b = self.new_block(&[]);
        let step_b = self.new_block(&[]);
        let exit = self.new_block(&[]);
        self.jump(head, vec![]);
        self.start(head);
        let (up_op, down_op) = if inclusive {
            (Punct::LtEq, Punct::GtEq)
        } else {
            (Punct::Lt, Punct::Gt)
        };
        let up = self.mem_cmp(up_op, cur, b, m);
        let cond = match (step, up) {
            (Some(sp), Some(up)) => {
                let neg = match m {
                    MemNum::Wide(n, true) => self.wide_is_neg(sp, n),
                    MemNum::Wide(_, false) => None,
                    MemNum::Soft(k) => {
                        let zero = self.soft_from_int(0, k);
                        self.mem_cmp(Punct::Lt, sp, zero, m)
                    }
                };
                match neg {
                    Some(neg) => {
                        let down = self.mem_cmp(down_op, cur, b, m);
                        match down {
                            Some(down) => match self.select(neg, down, up, IrTy::Bool, span) {
                                Res::Val(v) => Some(v),
                                _ => None,
                            },
                            None => None,
                        }
                    }
                    None => Some(up),
                }
            }
            (_, up) => up,
        };
        if let Some(c) = cond {
            self.branch(c, body_b, exit);
        }
        let depth = self.f.scopes.len();
        self.f.loops.push(LoopCtx {
            label,
            cont: step_b,
            exit,
            break_ty: None,
            break_dst: None,
            scope_depth: depth,
        });
        self.start(body_b);
        self.copy_bytes(iv, cur, self.mem_bytes(m));
        if let Some(d) = var {
            self.f.locals.insert(
                d,
                Local::Slot {
                    ptr: iv,
                    ty: sty,
                    flag: None,
                },
            );
        }
        let void = self.types.void;
        let r = self.run_body(body, void);
        if !matches!(r, Res::Never) {
            self.jump(step_b, vec![]);
        }
        self.f.loops.pop();
        self.start(step_b);
        if inclusive {
            // After the last value, stop without adding (the add could overflow).
            if let Some(at_end) = self.mem_cmp(Punct::EqEq, cur, b, m) {
                let more = self.new_block(&[]);
                self.branch(at_end, exit, more);
                self.start(more);
            }
        }
        match m {
            MemNum::Wide(n, signed) => self.wide_step(cur, step, n, signed),
            MemNum::Soft(k) => self.soft_step(cur, step, k),
        }
        self.jump(head, vec![]);
        self.start(exit);
        Some(Res::Unit)
    }
}
