//! Floats the machine has no instructions for: `float16`, `float128` and `float256`.
//!
//! Like the wide integers (`wide.rs`) they are memory values, and every operation is a call into
//! `compiler/core/core/softfloat.pel`, written once for any IEEE binary format: it is given the number of
//! 64-bit words and the exponent and fraction widths. The compiler's part is small: it copies each operand
//! into a private zero-filled slot of whole words (a `float16` is 2 bytes in memory, the library reads 8),
//! calls, and copies the result back. A literal is turned into bits here, at compile time, rounded
//! correctly (`softfloat.rs`). Arithmetic follows IEEE 754 (round to nearest even, infinities, NaN,
//! signed zero); it does not stop on overflow or on division by zero, as `float` does not.

use super::*;
use crate::types::FloatKind;

const SP: Span = Span::new(0, 0);

impl<'a> Lowerer<'a> {
    pub(crate) fn soft_of(&mut self, t: Ty) -> Option<FloatKind> {
        let t = self.types.shallow(t);
        match self.kind(t) {
            TyKind::Float(k) if k.soft() => Some(k),
            _ => None,
        }
    }

    fn sf_i(&mut self, n: i64) -> Value {
        self.i64c(n)
    }

    fn sf_zeroed(&mut self, words: u32) -> Value {
        let s = self.slot_bytes(8 * words, 8);
        for i in 0..words {
            let z = self.i64c(0);
            self.store_at(s, 8 * i, z);
        }
        s
    }

    /// A private zero-filled copy, in whole words, of the float of `k` stored at `p`.
    fn sf_copy(&mut self, p: Value, k: FloatKind) -> Value {
        let s = self.sf_zeroed(k.words());
        self.copy_bytes(s, p, k.bytes());
        s
    }

    fn sf_operand(&mut self, e: &'a Expr, k: FloatKind) -> Option<Value> {
        let Res::Val(p) = self.expr(e) else {
            return None;
        };
        Some(self.sf_copy(p, k))
    }

    /// The format arguments `(n, eb, mb)` of a kind as three constants.
    fn sf_fmt(&mut self, k: FloatKind) -> [Value; 3] {
        let (eb, mb) = k.format();
        [
            self.sf_i(k.words() as i64),
            self.sf_i(eb as i64),
            self.sf_i(mb as i64),
        ]
    }

    /// The bits of a float literal (a decimal or integer text), stored to `dst`.
    pub(crate) fn soft_literal(&mut self, e: &Expr, dst: Value, k: FloatKind) -> Res {
        let text = self.src_text(e.span).replace('_', "");
        let text = if matches!(e.kind, ExprKind::Int) {
            // Hexadecimal, binary and octal integers go through their value.
            match crate::typecheck::literal_value(&text) {
                Some(v) => v.to_string(),
                None => text,
            }
        } else {
            text
        };
        let (eb, mb) = k.format();
        let fmt = crate::softfloat::Format { eb, mb };
        let Some(words) = crate::softfloat::parse(&text, fmt) else {
            return self.unsupported(e.span, "a float literal of this form");
        };
        let tmp = self.sf_zeroed(k.words());
        for (i, w) in words.iter().enumerate() {
            let c = self.i64c(*w as i64);
            self.store_at(tmp, 8 * i as u32, c);
        }
        self.copy_bytes(dst, tmp, k.bytes());
        Res::Unit
    }

    /// `-a` on a soft float, written to `dst`.
    pub(crate) fn soft_unary(
        &mut self,
        e: &'a Expr,
        op: Punct,
        operand: &'a Expr,
        dst: Value,
        k: FloatKind,
    ) -> Res {
        if op != Punct::Minus {
            return self.unsupported(e.span, "this operator on this type");
        }
        let Some(a) = self.sf_operand(operand, k) else {
            return Res::Never;
        };
        let tmp = self.sf_zeroed(k.words());
        let [n, eb, mb] = self.sf_fmt(k);
        self.call_core("rtFloatNeg", vec![tmp, a, n, eb, mb]);
        self.copy_bytes(dst, tmp, k.bytes());
        Res::Unit
    }

    fn sf_op_code(&self, op: Punct) -> Option<i64> {
        match op {
            Punct::Plus => Some(0),
            Punct::Minus => Some(1),
            Punct::Star => Some(2),
            Punct::Slash => Some(3),
            Punct::Percent => Some(4),
            _ => None,
        }
    }

    /// `a op b` on two soft floats, written to `dst`.
    pub(crate) fn soft_binary(
        &mut self,
        e: &'a Expr,
        op: Punct,
        lhs: &'a Expr,
        rhs: &'a Expr,
        dst: Value,
        k: FloatKind,
    ) -> Res {
        let Some(code) = self.sf_op_code(op) else {
            return self.unsupported(e.span, "this operator on this type");
        };
        let Some(a) = self.sf_operand(lhs, k) else {
            return Res::Never;
        };
        let Some(b) = self.sf_operand(rhs, k) else {
            return Res::Never;
        };
        self.sf_arith(code, dst, a, b, k);
        Res::Unit
    }

    fn sf_arith(&mut self, code: i64, dst: Value, a: Value, b: Value, k: FloatKind) {
        let tmp = self.sf_zeroed(k.words());
        let [n, eb, mb] = self.sf_fmt(k);
        let c = self.sf_i(code);
        self.call_core("rtFloatBin", vec![tmp, a, b, n, eb, mb, c]);
        self.copy_bytes(dst, tmp, k.bytes());
    }

    /// `a op= b` on a soft float place.
    pub(crate) fn soft_assign(
        &mut self,
        e: &'a Expr,
        base: Punct,
        p: Value,
        lt: Ty,
        rhs: &'a Expr,
    ) -> Res {
        let (Some(k), Some(code)) = (self.soft_of(lt), self.sf_op_code(base)) else {
            return self.unsupported(e.span, "this assignment operator");
        };
        let Some(b) = self.sf_operand(rhs, k) else {
            return Res::Never;
        };
        let a = self.sf_copy(p, k);
        self.sf_arith(code, p, a, b, k);
        Res::Unit
    }

    /// The comparison operators on two soft floats: a Bool.
    pub(crate) fn soft_compare(
        &mut self,
        e: &'a Expr,
        op: Punct,
        lhs: &'a Expr,
        rhs: &'a Expr,
        k: FloatKind,
    ) -> Res {
        let Some(a) = self.sf_operand(lhs, k) else {
            return Res::Never;
        };
        let Some(b) = self.sf_operand(rhs, k) else {
            return Res::Never;
        };
        let [n, eb, mb] = self.sf_fmt(k);
        let Some(r) = self.call_core("rtFloatCmp", vec![a, b, n, eb, mb]) else {
            return Res::Never;
        };
        let c = |s: &mut Self, v: i64| s.i64c(v);
        // The answer is -1, 0, 1, or 2 for a NaN.
        let (cmp, rv, lhs_v) = match op {
            Punct::EqEq => (CmpOp::Eq, c(self, 0), r),
            Punct::NotEq => (CmpOp::Ne, c(self, 0), r),
            Punct::Lt => (CmpOp::Eq, c(self, -1), r),
            Punct::Gt => (CmpOp::Eq, c(self, 1), r),
            Punct::LtEq => {
                let one = c(self, 1);
                let s = self.wrap_bin(BinOp::Add, r, one);
                (CmpOp::ULt, c(self, 2), s)
            }
            _ => (CmpOp::ULt, c(self, 2), r),
        };
        self.wrap(Op::Cmp(cmp, lhs_v, rv), IrTy::Bool, e.span)
    }

    fn wrap_bin(&mut self, op: BinOp, a: Value, b: Value) -> Value {
        self.emit(Op::Bin(op, a, b), Some(IrTy::I64), SP)
            .unwrap_or(a)
    }

    /// `x to T` where `T` is a soft float, written to `dst`.
    pub(crate) fn soft_convert(
        &mut self,
        e: &'a Expr,
        value: &'a Expr,
        from: Ty,
        dst: Value,
        k: FloatKind,
    ) -> Res {
        let tmp = self.sf_zeroed(k.words());
        let [n, eb, mb] = self.sf_fmt(k);
        if let Some((fnn, fsigned)) = self.wide_of(from) {
            let Some(p) = self.wide_operand(value, from) else {
                return Res::Never;
            };
            let ni = self.sf_i(fnn as i64);
            let s = self.konst(IrTy::Bool, Const::Bool(fsigned), e.span);
            self.call_core("rtFloatFromInt", vec![tmp, p, ni, s, n, eb, mb]);
        } else if let Some(fk) = self.soft_of(from) {
            let Some(p) = self.sf_operand(value, fk) else {
                return Res::Never;
            };
            let (feb, fmb) = fk.format();
            let (fn_, feb, fmb) = (
                self.sf_i(fk.words() as i64),
                self.sf_i(feb as i64),
                self.sf_i(fmb as i64),
            );
            self.call_core("rtFloatConvert", vec![tmp, n, eb, mb, p, fn_, feb, fmb]);
        } else {
            let Some(v) = self.scalar(value) else {
                return Res::Never;
            };
            let src = self.sf_zeroed(1);
            match self.vty(v) {
                IrTy::F32 | IrTy::F64 => {
                    let (fk, ty) = if self.vty(v) == IrTy::F32 {
                        (FloatKind::F32, IrTy::F32)
                    } else {
                        (FloatKind::F64, IrTy::F64)
                    };
                    let _ = ty;
                    self.store_at(src, 0, v);
                    let (feb, fmb) = fk.format();
                    let (one, feb, fmb) =
                        (self.sf_i(1), self.sf_i(feb as i64), self.sf_i(fmb as i64));
                    self.call_core("rtFloatConvert", vec![tmp, n, eb, mb, src, one, feb, fmb]);
                }
                IrTy::Int(_) => {
                    let signed = self.is_signed(from);
                    let w = self.resize(v, IrTy::I64, signed, e.span);
                    self.store_at(src, 0, w);
                    let one = self.sf_i(1);
                    let s = self.konst(IrTy::Bool, Const::Bool(signed), e.span);
                    self.call_core("rtFloatFromInt", vec![tmp, src, one, s, n, eb, mb]);
                }
                _ => return self.unsupported(e.span, "this conversion"),
            }
        }
        self.copy_bytes(dst, tmp, k.bytes());
        Res::Unit
    }

    /// `x to float` or `to float32` where `x` is a soft float: a number the machine has.
    pub(crate) fn soft_to_native(
        &mut self,
        e: &'a Expr,
        value: &'a Expr,
        from: FloatKind,
        to: IrTy,
    ) -> Res {
        let Some(p) = self.sf_operand(value, from) else {
            return Res::Never;
        };
        let tk = if to == IrTy::F32 {
            FloatKind::F32
        } else {
            FloatKind::F64
        };
        let tmp = self.sf_zeroed(1);
        let (teb, tmb) = tk.format();
        let (one, teb, tmb) = (self.sf_i(1), self.sf_i(teb as i64), self.sf_i(tmb as i64));
        let [n, eb, mb] = self.sf_fmt(from);
        self.call_core("rtFloatConvert", vec![tmp, one, teb, tmb, p, n, eb, mb]);
        {
            let _ = e;
            Res::Val(self.load_at(tmp, 0, to))
        }
    }

    /// Appends the text of the soft float at `p` to the string `out`.
    pub(crate) fn soft_show(&mut self, p: Value, k: FloatKind, out: Value) -> bool {
        let a = self.sf_copy(p, k);
        let [n, eb, mb] = self.sf_fmt(k);
        self.call_core("rtFloatShow", vec![out, a, n, eb, mb]);
        true
    }

    /// The comparison for the glue: -1, 0, 1 (2 for a NaN).
    pub(crate) fn soft_cmp_glue(&mut self, a: Value, b: Value, k: FloatKind) -> Option<Value> {
        let x = self.sf_copy(a, k);
        let y = self.sf_copy(b, k);
        let [n, eb, mb] = self.sf_fmt(k);
        self.call_core("rtFloatCmp", vec![x, y, n, eb, mb])
    }

    /// A number for hashing the soft float at `a`.
    pub(crate) fn soft_key_glue(&mut self, a: Value, k: FloatKind) -> Option<Value> {
        let x = self.sf_copy(a, k);
        let [n, eb, mb] = self.sf_fmt(k);
        self.call_core("rtFloatKey", vec![x, n, eb, mb])
    }
}
