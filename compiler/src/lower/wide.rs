//! Wide integers: `int128`, `uint128`, `int256`, `uint256`.
//!
//! A wide integer is a memory value of `n` 64-bit words (2 for 128 bits, 4 for 256), least significant word
//! first, in two's complement. Every operation is written once for any `n`, so adding a width is a line in
//! `types.rs`. Nothing here is a new IR instruction: addition, subtraction, comparison and the bit operations
//! are chains of 64-bit instructions (the carry is worked out with bit operations, so there are no branches),
//! and multiplication, division, shifts and printing are calls to loops in `core.wide`
//! (`compiler/core/core/wide.pel`). So the interpreter, x86-64 and AArch64 all run them with no backend work.
//! The checks keep the language's rule: an overflow, a division by zero and a shift by the width or more stop
//! the program.

use super::*;
use crate::typecheck::literal_words;

const SP: Span = Span::new(0, 0);

impl<'a> Lowerer<'a> {
    /// `(words, signed)` when `t` is a wide integer.
    pub(crate) fn wide_of(&mut self, t: Ty) -> Option<(u32, bool)> {
        let t = self.types.shallow(t);
        match self.kind(t) {
            TyKind::Int(k) if k.words() > 0 => Some((k.words(), k.signed())),
            _ => None,
        }
    }

    fn wbin(&mut self, op: BinOp, a: Value, b: Value) -> Value {
        self.emit(Op::Bin(op, a, b), Some(IrTy::I64), SP)
            .unwrap_or(a)
    }

    fn wnot(&mut self, a: Value) -> Value {
        self.emit(Op::Not(a), Some(IrTy::I64), SP).unwrap_or(a)
    }

    /// The top bit of a word, as 0 or 1.
    fn wmsb(&mut self, a: Value) -> Value {
        let k = self.i64c(63);
        self.wbin(BinOp::UShr, a, k)
    }

    fn wflag(&mut self, v: Value) -> Value {
        let z = self.i64c(0);
        self.emit(Op::Cmp(CmpOp::Ne, v, z), Some(IrTy::Bool), SP)
            .unwrap_or(v)
    }

    fn wload(&mut self, p: Value, n: u32) -> Vec<Value> {
        (0..n).map(|i| self.load_at(p, 8 * i, IrTy::I64)).collect()
    }

    fn wstore(&mut self, p: Value, ws: &[Value]) {
        for (i, w) in ws.iter().enumerate() {
            self.store_at(p, 8 * i as u32, *w);
        }
    }

    /// `a + b + carry_in` for one word: the sum and the carry out (0 or 1).
    fn wadd1(&mut self, a: Value, b: Value, cin: Option<Value>) -> (Value, Value) {
        let s1 = self.wbin(BinOp::Add, a, b);
        let ab = self.wbin(BinOp::And, a, b);
        let ao = self.wbin(BinOp::Or, a, b);
        let ns = self.wnot(s1);
        let t = self.wbin(BinOp::And, ao, ns);
        let c = self.wbin(BinOp::Or, ab, t);
        let c1 = self.wmsb(c);
        let Some(cin) = cin else {
            return (s1, c1);
        };
        let s2 = self.wbin(BinOp::Add, s1, cin);
        let ab = self.wbin(BinOp::And, s1, cin);
        let ao = self.wbin(BinOp::Or, s1, cin);
        let ns = self.wnot(s2);
        let t = self.wbin(BinOp::And, ao, ns);
        let c = self.wbin(BinOp::Or, ab, t);
        let c2 = self.wmsb(c);
        let co = self.wbin(BinOp::Or, c1, c2);
        (s2, co)
    }

    /// `a - b - borrow_in` for one word: the difference and the borrow out.
    fn wsub1(&mut self, a: Value, b: Value, bin: Option<Value>) -> (Value, Value) {
        let d1 = self.wbin(BinOp::Sub, a, b);
        let na = self.wnot(a);
        let x = self.wbin(BinOp::Xor, a, b);
        let nx = self.wnot(x);
        let l = self.wbin(BinOp::And, na, b);
        let r = self.wbin(BinOp::And, nx, d1);
        let c = self.wbin(BinOp::Or, l, r);
        let b1 = self.wmsb(c);
        let Some(bin) = bin else {
            return (d1, b1);
        };
        let d2 = self.wbin(BinOp::Sub, d1, bin);
        let nd = self.wnot(d1);
        let x = self.wbin(BinOp::Xor, d1, bin);
        let nx = self.wnot(x);
        let l = self.wbin(BinOp::And, nd, bin);
        let r = self.wbin(BinOp::And, nx, d2);
        let c = self.wbin(BinOp::Or, l, r);
        let b2 = self.wmsb(c);
        let bo = self.wbin(BinOp::Or, b1, b2);
        (d2, bo)
    }

    /// `a + b` or `a - b` over all the words. With `checked`, an overflow stops the program.
    fn wide_addsub(
        &mut self,
        sub: bool,
        signed: bool,
        checked: bool,
        a: &[Value],
        b: &[Value],
    ) -> Vec<Value> {
        let mut out = Vec::new();
        let mut c: Option<Value> = None;
        for (&x, &y) in a.iter().zip(b) {
            let (r, co) = if sub {
                self.wsub1(x, y, c)
            } else {
                self.wadd1(x, y, c)
            };
            out.push(r);
            c = Some(co);
        }
        if checked {
            let top = a.len() - 1;
            let over = if signed {
                // Add: both operands' signs differ from the result's. Subtract: the operands differ in sign and
                // the result's sign is not the first operand's.
                let (x, y) = if sub {
                    (
                        self.wbin(BinOp::Xor, a[top], b[top]),
                        self.wbin(BinOp::Xor, a[top], out[top]),
                    )
                } else {
                    (
                        self.wbin(BinOp::Xor, a[top], out[top]),
                        self.wbin(BinOp::Xor, b[top], out[top]),
                    )
                };
                let m = self.wbin(BinOp::And, x, y);
                let z = self.i64c(0);
                self.emit(Op::Cmp(CmpOp::Lt, m, z), Some(IrTy::Bool), SP)
            } else {
                let f = self.wflag(c.unwrap_or(a[top]));
                Some(f)
            };
            if let Some(o) = over {
                self.stop_if(o, "arithmetic overflow");
            }
        }
        out
    }

    /// An operand in a private slot (the right-hand side may change the variable the left one names).
    pub(crate) fn wide_operand(&mut self, e: &'a Expr, t: Ty) -> Option<Value> {
        let Res::Val(p) = self.expr(e) else {
            return None;
        };
        let size = self.layout(t).size;
        let tmp = self.slot_bytes(size, 8);
        self.copy_bytes(tmp, p, size);
        Some(tmp)
    }

    /// The low 64-bit value of a shift count of any integer type, stopping when it is out of range.
    fn wide_shift_count(&mut self, e: &'a Expr, bits: u32) -> Option<Value> {
        let rt = self.ty_of(e)?;
        let amt = if let Some((rn, rs)) = self.wide_of(rt) {
            let p = self.wide_operand(e, rt)?;
            let ws = self.wload(p, rn);
            let mut hi = self.i64c(0);
            for w in &ws[1..] {
                hi = self.wbin(BinOp::Or, hi, *w);
            }
            let _ = rs;
            let bad = self.wflag(hi);
            self.stop_if(bad, "shifted by the width or more");
            ws[0]
        } else {
            let s = self.is_signed(rt);
            let v = self.scalar(e)?;
            self.resize(v, IrTy::I64, s, e.span)
        };
        let limit = self.i64c(bits as i64);
        let big = self.emit(Op::Cmp(CmpOp::UGe, amt, limit), Some(IrTy::Bool), SP)?;
        self.stop_if(big, "shifted by the width or more");
        Some(amt)
    }

    /// The words of a literal as two's complement, in `words` words.
    fn wide_literal_words(&self, text: &str, neg: bool, words: u32) -> Vec<i64> {
        let src = literal_words(text).unwrap_or([0; 4]);
        let mut w: Vec<u64> = (0..words as usize)
            .map(|i| src.get(i).copied().unwrap_or(0))
            .collect();
        if neg {
            let mut carry = true;
            for x in w.iter_mut() {
                let (s, c) = (!*x).overflowing_add(carry as u64);
                *x = s;
                carry = c;
            }
        }
        w.into_iter().map(|x| x as i64).collect()
    }

    fn wide_store_literal(&mut self, dst: Value, span: Span, neg: bool, words: u32) {
        let text = self.src_text(span).to_string();
        for (i, w) in self
            .wide_literal_words(&text, neg, words)
            .into_iter()
            .enumerate()
        {
            let k = self.i64c(w);
            self.store_at(dst, 8 * i as u32, k);
        }
    }

    /// An integer literal of a wide type, written to `dst`.
    pub(crate) fn wide_int_literal(&mut self, e: &Expr, dst: Value, words: u32) -> Res {
        self.wide_store_literal(dst, e.span, false, words);
        Res::Unit
    }

    /// `-a`, `~a` and `!a` on a wide integer, written to `dst`.
    pub(crate) fn wide_unary(
        &mut self,
        e: &'a Expr,
        op: Punct,
        operand: &'a Expr,
        dst: Value,
        t: Ty,
    ) -> Res {
        let Some((n, signed)) = self.wide_of(t) else {
            return self.unsupported(e.span, "this operand type");
        };
        match op {
            Punct::Minus => {
                if matches!(operand.kind, ExprKind::Int) {
                    self.wide_store_literal(dst, operand.span, true, n);
                    return Res::Unit;
                }
                let Some(p) = self.wide_operand(operand, t) else {
                    return Res::Never;
                };
                let b = self.wload(p, n);
                let z = self.i64c(0);
                let a = vec![z; n as usize];
                let r = self.wide_addsub(true, signed, true, &a, &b);
                self.wstore(dst, &r);
                Res::Unit
            }
            Punct::Tilde | Punct::Bang => {
                let Some(p) = self.wide_operand(operand, t) else {
                    return Res::Never;
                };
                let a = self.wload(p, n);
                let r: Vec<Value> = a.into_iter().map(|w| self.wnot(w)).collect();
                self.wstore(dst, &r);
                Res::Unit
            }
            _ => self.unsupported(e.span, "this operator"),
        }
    }

    /// `a op b` on two wide integers, written to `dst`.
    pub(crate) fn wide_binary(
        &mut self,
        e: &'a Expr,
        op: Punct,
        lhs: &'a Expr,
        rhs: &'a Expr,
        dst: Value,
        t: Ty,
    ) -> Res {
        let Some((n, signed)) = self.wide_of(t) else {
            return self.unsupported(e.span, "this operand type");
        };
        let Some(pa) = self.wide_operand(lhs, t) else {
            return Res::Never;
        };
        if matches!(op, Punct::Shl | Punct::Shr) {
            let Some(amt) = self.wide_shift_count(rhs, n * 64) else {
                return Res::Never;
            };
            return self.wide_shift(op, signed, n, dst, pa, amt);
        }
        let Some(pb) = self.wide_operand(rhs, t) else {
            return Res::Never;
        };
        self.wide_arith(e.span, op, n, signed, dst, pa, pb)
    }

    fn wide_shift(
        &mut self,
        op: Punct,
        signed: bool,
        n: u32,
        dst: Value,
        pa: Value,
        amt: Value,
    ) -> Res {
        let kind = match (op, signed) {
            (Punct::Shl, _) => 0,
            (_, false) => 1,
            (_, true) => 2,
        };
        let nn = self.i64c(n as i64);
        let k = self.i64c(kind);
        self.call_core("rtWideShift", vec![dst, pa, amt, nn, k]);
        Res::Unit
    }

    /// The operators that take two wide operands in memory.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn wide_arith(
        &mut self,
        span: Span,
        op: Punct,
        n: u32,
        signed: bool,
        dst: Value,
        pa: Value,
        pb: Value,
    ) -> Res {
        match op {
            Punct::Plus | Punct::Minus | Punct::PlusPercent | Punct::MinusPercent => {
                let a = self.wload(pa, n);
                let b = self.wload(pb, n);
                let sub = matches!(op, Punct::Minus | Punct::MinusPercent);
                let checked = matches!(op, Punct::Plus | Punct::Minus);
                let r = self.wide_addsub(sub, signed, checked, &a, &b);
                self.wstore(dst, &r);
                Res::Unit
            }
            Punct::Amp | Punct::Pipe | Punct::Caret => {
                let a = self.wload(pa, n);
                let b = self.wload(pb, n);
                let o = match op {
                    Punct::Amp => BinOp::And,
                    Punct::Pipe => BinOp::Or,
                    _ => BinOp::Xor,
                };
                let r: Vec<Value> = a
                    .iter()
                    .zip(&b)
                    .map(|(&x, &y)| self.wbin(o, x, y))
                    .collect();
                self.wstore(dst, &r);
                Res::Unit
            }
            Punct::Star | Punct::StarPercent => {
                let nn = self.i64c(n as i64);
                let s = self.konst(IrTy::Bool, Const::Bool(signed), span);
                let w = self.konst(IrTy::Bool, Const::Bool(op == Punct::StarPercent), span);
                if let Some(over) = self.call_core("rtWideMul", vec![dst, pa, pb, nn, s, w]) {
                    self.stop_if(over, "arithmetic overflow");
                }
                Res::Unit
            }
            Punct::Slash | Punct::Percent => {
                let b = self.wload(pb, n);
                let mut any = b[0];
                for w in &b[1..] {
                    any = self.wbin(BinOp::Or, any, *w);
                }
                let z = self.i64c(0);
                if let Some(is_zero) = self.emit(Op::Cmp(CmpOp::Eq, any, z), Some(IrTy::Bool), span)
                {
                    self.stop_if(is_zero, "division by zero");
                }
                let size = 8 * n;
                let scratch = self.slot_bytes(size, 8);
                let (q, r) = if op == Punct::Slash {
                    (dst, scratch)
                } else {
                    (scratch, dst)
                };
                let nn = self.i64c(n as i64);
                let s = self.konst(IrTy::Bool, Const::Bool(signed), span);
                if let Some(over) = self.call_core("rtWideDiv", vec![q, r, pa, pb, nn, s]) {
                    self.stop_if(over, "arithmetic overflow");
                }
                Res::Unit
            }
            _ => self.unsupported(span, "this operator on this type"),
        }
    }

    /// `a op= b` on a wide place `p`.
    pub(crate) fn wide_assign(
        &mut self,
        e: &'a Expr,
        base: Punct,
        p: Value,
        lt: Ty,
        rhs: &'a Expr,
    ) -> Res {
        let Some((n, signed)) = self.wide_of(lt) else {
            return self.unsupported(e.span, "this assignment");
        };
        let size = 8 * n;
        let pa = self.slot_bytes(size, 8);
        let tmp = self.slot_bytes(size, 8);
        let amt = if matches!(base, Punct::Shl | Punct::Shr) {
            match self.wide_shift_count(rhs, n * 64) {
                Some(a) => Some(a),
                None => return Res::Never,
            }
        } else {
            None
        };
        let pb = if amt.is_none() {
            match self.wide_operand(rhs, lt) {
                Some(b) => Some(b),
                None => return Res::Never,
            }
        } else {
            None
        };
        self.copy_bytes(pa, p, size);
        let r = if let Some(amt) = amt {
            self.wide_shift(base, signed, n, tmp, pa, amt)
        } else {
            self.wide_arith(e.span, base, n, signed, tmp, pa, pb.unwrap_or(pa))
        };
        if matches!(r, Res::Never) {
            return r;
        }
        self.copy_bytes(p, tmp, size);
        Res::Unit
    }

    /// `a++` and `a--` on a wide place.
    pub(crate) fn wide_incdec(&mut self, e: &Expr, op: Punct, p: Value, t: Ty) -> Res {
        let Some((n, signed)) = self.wide_of(t) else {
            return self.unsupported(e.span, "`++` on this type");
        };
        let a = self.wload(p, n);
        let z = self.i64c(0);
        let mut b = vec![z; n as usize];
        b[0] = self.i64c(1);
        let r = self.wide_addsub(op == Punct::MinusMinus, signed, true, &a, &b);
        self.wstore(p, &r);
        Res::Unit
    }

    /// `a < b` and the other comparisons on two wide integers: a Bool.
    pub(crate) fn wide_compare(
        &mut self,
        e: &'a Expr,
        op: Punct,
        lhs: &'a Expr,
        rhs: &'a Expr,
        t: Ty,
    ) -> Res {
        let Some((n, signed)) = self.wide_of(t) else {
            return self.unsupported(e.span, "comparing these values");
        };
        let Some(pa) = self.wide_operand(lhs, t) else {
            return Res::Never;
        };
        let Some(pb) = self.wide_operand(rhs, t) else {
            return Res::Never;
        };
        let a = self.wload(pa, n);
        let b = self.wload(pb, n);
        if matches!(op, Punct::EqEq | Punct::NotEq) {
            let mut acc = self.i64c(0);
            for (&p, &q) in a.iter().zip(&b) {
                let x = self.wbin(BinOp::Xor, p, q);
                acc = self.wbin(BinOp::Or, acc, x);
            }
            let z = self.i64c(0);
            let c = if op == Punct::EqEq {
                CmpOp::Eq
            } else {
                CmpOp::Ne
            };
            return self.wrap(Op::Cmp(c, acc, z), IrTy::Bool, e.span);
        }
        // `x < y` is the borrow out of `x - y`; a signed compare flips the top bit of both first.
        let (first, second, negate) = match op {
            Punct::Lt => (&a, &b, false),
            Punct::Gt => (&b, &a, false),
            Punct::LtEq => (&b, &a, true),
            _ => (&a, &b, true),
        };
        let (x, y) = (first.clone(), second.clone());
        let lt = self.wide_less(signed, &x, &y);
        let z = self.i64c(0);
        let c = if negate { CmpOp::Eq } else { CmpOp::Ne };
        self.wrap(Op::Cmp(c, lt, z), IrTy::Bool, e.span)
    }

    /// 1 when `a < b`, else 0.
    fn wide_less(&mut self, signed: bool, a: &[Value], b: &[Value]) -> Value {
        let mut a = a.to_vec();
        let mut b = b.to_vec();
        if signed {
            let top = a.len() - 1;
            let flip = self.i64c(i64::MIN);
            a[top] = self.wbin(BinOp::Xor, a[top], flip);
            b[top] = self.wbin(BinOp::Xor, b[top], flip);
        }
        let mut borrow: Option<Value> = None;
        for (&x, &y) in a.iter().zip(&b) {
            let (_, bo) = self.wsub1(x, y, borrow);
            borrow = Some(bo);
        }
        borrow.unwrap_or_else(|| self.i64c(0))
    }

    /// Eq for the glue: jump to `fail` when the wide integers at `a` and `b` differ.
    pub(crate) fn wide_eq_glue(&mut self, a: Value, b: Value, n: u32) -> Option<Value> {
        let x = self.wload(a, n);
        let y = self.wload(b, n);
        let mut acc = self.i64c(0);
        for (&p, &q) in x.iter().zip(&y) {
            let d = self.wbin(BinOp::Xor, p, q);
            acc = self.wbin(BinOp::Or, acc, d);
        }
        let z = self.i64c(0);
        self.emit(Op::Cmp(CmpOp::Eq, acc, z), Some(IrTy::Bool), SP)
    }

    /// -1, 0 or 1 for the wide integers at `a` and `b`.
    pub(crate) fn wide_cmp_glue(&mut self, a: Value, b: Value, n: u32, signed: bool) -> Value {
        let x = self.wload(a, n);
        let y = self.wload(b, n);
        let lt = self.wide_less(signed, &x, &y);
        let gt = self.wide_less(signed, &y, &x);
        self.wbin(BinOp::Sub, gt, lt)
    }

    /// Mixes the words of the wide integer at `a` into the hash.
    pub(crate) fn wide_words(&mut self, a: Value, n: u32) -> Vec<Value> {
        self.wload(a, n)
    }

    /// `x to T` where one side is a wide integer and the other a number or a `bool`.
    pub(crate) fn wide_convert(
        &mut self,
        e: &'a Expr,
        value: &'a Expr,
        from: Ty,
        to: Ty,
        dst: Value,
    ) -> Res {
        let Some((tn, _)) = self.wide_of(to) else {
            return self.unsupported(e.span, "this conversion");
        };
        let words: Vec<Value> = if let Some((fnn, fsigned)) = self.wide_of(from) {
            let Some(p) = self.wide_operand(value, from) else {
                return Res::Never;
            };
            let mut ws = self.wload(p, fnn);
            let fill = if fsigned {
                let k = self.i64c(63);
                let top = ws[ws.len() - 1];
                self.wbin(BinOp::Shr, top, k)
            } else {
                self.i64c(0)
            };
            while (ws.len() as u32) < tn {
                ws.push(fill);
            }
            ws.truncate(tn as usize);
            ws
        } else {
            let Some(v) = self.scalar(value) else {
                return Res::Never;
            };
            let signed = self.is_signed(from);
            let w = match self.vty(v) {
                IrTy::Bool => {
                    let one = self.i64c(1);
                    let zero = self.i64c(0);
                    match self.select(v, one, zero, IrTy::I64, e.span) {
                        Res::Val(x) => x,
                        other => return other,
                    }
                }
                _ => self.resize(v, IrTy::I64, signed, e.span),
            };
            let fill = if signed {
                let k = self.i64c(63);
                self.wbin(BinOp::Shr, w, k)
            } else {
                self.i64c(0)
            };
            let mut ws = vec![w];
            while (ws.len() as u32) < tn {
                ws.push(fill);
            }
            ws
        };
        self.wstore(dst, &words);
        Res::Unit
    }

    /// `x to float` for a wide integer `x`.
    pub(crate) fn wide_to_float(
        &mut self,
        e: &'a Expr,
        value: &'a Expr,
        from: Ty,
        to: IrTy,
    ) -> Res {
        let Some((n, signed)) = self.wide_of(from) else {
            return self.unsupported(e.span, "this conversion");
        };
        let Some(p) = self.wide_operand(value, from) else {
            return Res::Never;
        };
        let nn = self.i64c(n as i64);
        let s = self.konst(IrTy::Bool, Const::Bool(signed), e.span);
        let Some(f) = self.call_core("rtWideToFloat", vec![p, nn, s]) else {
            return Res::Never;
        };
        if to == IrTy::F32 {
            return self.wrap(
                Op::Cast(CastKind::FloatResize, f, IrTy::F32),
                IrTy::F32,
                e.span,
            );
        }
        Res::Val(f)
    }

    /// Appends the decimal text of the wide integer at `p` to the string at `out`.
    pub(crate) fn wide_show(&mut self, p: Value, n: u32, signed: bool, out: Value) -> bool {
        let nn = self.i64c(n as i64);
        let s = self.konst(IrTy::Bool, Const::Bool(signed), SP);
        self.call_core("rtWideShow", vec![out, p, nn, s]);
        true
    }
}
