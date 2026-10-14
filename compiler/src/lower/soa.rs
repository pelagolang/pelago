//! `soa<T>`: a struct-of-arrays collection (card A-06, `docs/design/memory.md`).
//!
//! Layout: one list header (24 bytes: pointer, length, capacity) per field of `T`, one after the
//! other; all columns have the same length. The element is never stored whole: `points[i].x` is
//! the address `x_data + i * size`, and a whole element is gathered from the columns when needed.
//! Drop, clone and equality are those of the tuple of columns.

use super::*;

const HEADER: u32 = 24;

impl<'a> Lowerer<'a> {
    /// The fields of the element struct: offset and type of each.
    pub(crate) fn soa_fields(&mut self, elem: Ty) -> Vec<(u32, Ty)> {
        self.struct_fields(elem)
            .into_iter()
            .map(|f| (f.off, f.ty))
            .collect()
    }

    /// The column types (one list per field) of `soa<elem>`.
    pub(crate) fn soa_column_types(&mut self, elem: Ty) -> Vec<Ty> {
        self.soa_fields(elem)
            .into_iter()
            .map(|(_, t)| self.types.list(t))
            .collect()
    }

    pub(crate) fn soa_col(&mut self, h: Value, k: usize) -> Value {
        self.addr(h, k as u32 * HEADER)
    }

    pub(crate) fn soa_len(&mut self, h: Value) -> Value {
        self.load_at(h, 8, IrTy::I64)
    }

    /// The address of field `k` of element `i` (checked against the length).
    pub(crate) fn soa_field_addr(&mut self, h: Value, i: Value, k: usize, ft: Ty) -> Option<Value> {
        let len = self.soa_len(h);
        let bad = self.emit(
            Op::Cmp(CmpOp::UGe, i, len),
            Some(IrTy::Bool),
            Span::new(0, 0),
        )?;
        self.stop_if(bad, "index out of range");
        self.soa_field_addr_unchecked(h, i, k, ft)
    }

    pub(crate) fn soa_field_addr_unchecked(
        &mut self,
        h: Value,
        i: Value,
        k: usize,
        ft: Ty,
    ) -> Option<Value> {
        let col = self.soa_col(h, k);
        let data = self.load_at(col, 0, IrTy::Ptr);
        let size = self.layout(ft).size as i64;
        let sz = self.i64c(size);
        let off = self.emit(Op::Bin(BinOp::Mul, i, sz), Some(IrTy::I64), Span::new(0, 0))?;
        self.emit(Op::PtrAdd(data, off), Some(IrTy::Ptr), Span::new(0, 0))
    }

    /// Copies element `i` out of the columns into `dst` (a plain copy of the bits: the columns keep
    /// owning what is inside).
    pub(crate) fn soa_gather(&mut self, h: Value, i: Value, dst: Value, elem: Ty) -> Option<()> {
        let fields = self.soa_fields(elem);
        for (k, (off, ft)) in fields.iter().enumerate() {
            let src = if k == 0 {
                self.soa_field_addr(h, i, k, *ft)?
            } else {
                self.soa_field_addr_unchecked(h, i, k, *ft)?
            };
            let d = self.addr(dst, *off);
            self.copy_mem_or_scalar(d, src, *ft);
        }
        Some(())
    }

    /// Writes the bits of `src` back into element `i` (no drops: the bits came from there).
    pub(crate) fn soa_scatter(&mut self, h: Value, i: Value, src: Value, elem: Ty) -> Option<()> {
        let fields = self.soa_fields(elem);
        for (k, (off, ft)) in fields.iter().enumerate() {
            let dst = self.soa_field_addr_unchecked(h, i, k, *ft)?;
            let s = self.addr(src, *off);
            self.copy_mem_or_scalar(dst, s, *ft);
        }
        Some(())
    }

    /// `points[i] = value`: the old fields are dropped, the new ones move in.
    pub(crate) fn soa_set(&mut self, h: Value, i: Value, src: Value, elem: Ty) -> Option<()> {
        let fields = self.soa_fields(elem);
        for (k, (off, ft)) in fields.iter().enumerate() {
            let dst = if k == 0 {
                self.soa_field_addr(h, i, k, *ft)?
            } else {
                self.soa_field_addr_unchecked(h, i, k, *ft)?
            };
            self.drop_at(dst, *ft);
            let s = self.addr(src, *off);
            self.copy_mem_or_scalar(dst, s, *ft);
        }
        Some(())
    }

    /// Appends the element held at `src`; its fields move into the columns.
    pub(crate) fn soa_push(&mut self, h: Value, src: Value, elem: Ty) {
        let fields = self.soa_fields(elem);
        for (k, (off, ft)) in fields.iter().enumerate() {
            let col = self.soa_col(h, k);
            let s = self.addr(src, *off);
            self.call_core_g("rt_col_push", &[*ft], vec![col, s]);
        }
    }

    /// `soa [a, b]` and `soa []`.
    pub(crate) fn soa_lit(&mut self, e: &'a Expr, items: &'a [ListItem], dst: Value, t: Ty) -> Res {
        let TyKind::Soa(elem, _) = self.kind(t) else {
            return self.unsupported(e.span, "a `soa` literal of this type");
        };
        if items.iter().any(|i| i.spread) {
            return self.unsupported(e.span, "`...` in a `soa` literal");
        }
        let n = self.soa_fields(elem).len() as u32;
        let zero = self.i64c(0);
        for w in 0..n * 3 {
            self.store_at(dst, w * 8, zero);
        }
        for it in items {
            let tmp = self.slot_for(elem);
            if let Res::Never = self.into(&it.value, tmp, elem) {
                return Res::Never;
            }
            self.soa_push(dst, tmp, elem);
        }
        Res::Unit
    }

    /// The `rt_soa_*` library intrinsics: field loops over the columns.
    pub(crate) fn intr_soa(
        &mut self,
        name: &str,
        e: &'a Expr,
        elem: Ty,
        args: &'a [Arg],
        dst: Option<Value>,
    ) -> Res {
        let sp = e.span;
        let Some(h) = args.first().and_then(|a| self.scalar(&a.value)) else {
            return Res::Never;
        };
        let fields = self.soa_fields(elem);
        match name {
            "rt_soa_len" => Res::Val(self.soa_len(h)),
            "rt_soa_push" => {
                let Some(a) = args.get(1) else {
                    return Res::Never;
                };
                let tmp = self.slot_for(elem);
                if let Res::Never = self.into(&a.value, tmp, elem) {
                    return Res::Never;
                }
                self.soa_push(h, tmp, elem);
                Res::Unit
            }
            "rt_soa_insert" => {
                let (Some(i), Some(a)) = (self.arg_value(args, 1), args.get(2)) else {
                    return Res::Never;
                };
                let tmp = self.slot_for(elem);
                if let Res::Never = self.into(&a.value, tmp, elem) {
                    return Res::Never;
                }
                for (k, (off, ft)) in fields.iter().enumerate() {
                    let col = self.soa_col(h, k);
                    let s = self.addr(tmp, *off);
                    self.call_core_g("rt_col_insert", &[*ft], vec![col, i, s]);
                }
                Res::Unit
            }
            "rt_soa_remove" => {
                let Some(i) = self.arg_value(args, 1) else {
                    return Res::Never;
                };
                let d = match dst {
                    Some(d) => d,
                    None => self.slot_for(elem),
                };
                for (k, (off, ft)) in fields.iter().enumerate() {
                    let col = self.soa_col(h, k);
                    let o = self.addr(d, *off);
                    self.call_core_g("rt_col_remove", &[*ft], vec![col, i, o]);
                }
                if dst.is_none() {
                    self.own(d, elem, false);
                    return Res::Val(d);
                }
                Res::Unit
            }
            "rt_soa_clear" => {
                for (k, (_, ft)) in fields.iter().enumerate() {
                    let col = self.soa_col(h, k);
                    self.call_core_g("rt_col_clear", &[*ft], vec![col]);
                }
                Res::Unit
            }
            _ => self.unsupported(sp, "this `soa` operation"),
        }
    }
}
