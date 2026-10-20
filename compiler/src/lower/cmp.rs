//! Glue for comparing, ordering, hashing and printing values of any type.

use super::*;

impl<'a> Lowerer<'a> {
    /// A method a type defines for itself (`eq`, `cmp`, `show`, ...), when there is one.
    fn own_method(&mut self, ty: Ty, name: &str) -> Option<(DefId, HashMap<DefId, Ty>)> {
        if !matches!(self.kind(ty), TyKind::Adt(..)) {
            return None;
        }
        let (m, map) = self.find_impl_method(ty, name)?;
        // Only code written in the program counts, not an interface's promise.
        let body = self
            .fns
            .get(&m)
            .is_some_and(|e| !matches!(e.decl.body, FnBody::None));
        body.then_some((m, map))
    }

    fn require_true(&mut self, c: Value, fail: BlockId) {
        let ok = self.new_block(&[]);
        self.branch(c, ok, fail);
        self.start(ok);
    }

    fn three_way(&mut self, a: Value, b: Value, signed: bool) -> Value {
        let float = self.vty(a).is_float();
        let (lt, gt) = if signed || float {
            (CmpOp::Lt, CmpOp::Gt)
        } else {
            (CmpOp::ULt, CmpOp::UGt)
        };
        let sp = Span::new(0, 0);
        let l = self
            .emit(Op::Cmp(lt, a, b), Some(IrTy::Bool), sp)
            .unwrap_or(a);
        let g = self
            .emit(Op::Cmp(gt, a, b), Some(IrTy::Bool), sp)
            .unwrap_or(a);
        let one = self.i64c(1);
        let neg = self.i64c(-1);
        let zero = self.i64c(0);
        let inner = match self.select(g, one, zero, IrTy::I64, sp) {
            Res::Val(v) => v,
            _ => zero,
        };
        match self.select(l, neg, inner, IrTy::I64, sp) {
            Res::Val(v) => v,
            _ => zero,
        }
    }

    // ---- Equality -------------------------------------------------------------------------------------------------------

    pub(crate) fn eq_fn_body(&mut self, a: Value, b: Value, ty: Ty) -> bool {
        let fail = self.new_block(&[]);
        if !self.eq_part(a, b, ty, fail) {
            return false;
        }
        let t = self.konst(IrTy::Bool, Const::Bool(true), Span::new(0, 0));
        self.terminate(Term::Return(Some(t)));
        self.start(fail);
        let f = self.konst(IrTy::Bool, Const::Bool(false), Span::new(0, 0));
        self.terminate(Term::Return(Some(f)));
        true
    }

    /// Compares the values at `a` and `b`; jumps to `fail` when they differ.
    fn eq_part(&mut self, a: Value, b: Value, ty: Ty, fail: BlockId) -> bool {
        let sp = Span::new(0, 0);
        if let Some((m, map)) = self.own_method(ty, "eq") {
            let Some(f) = self.request_fn(m, &map) else {
                return false;
            };
            let Some(r) = self.emit(Op::Call(f, vec![a, b]), Some(IrTy::Bool), sp) else {
                return false;
            };
            self.require_true(r, fail);
            return true;
        }
        match self.kind(ty) {
            TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char | TyKind::Ptr => {
                let s = self.scalar_of(ty).unwrap_or(IrTy::I64);
                let x = self.load_at(a, 0, s);
                let y = self.load_at(b, 0, s);
                if let Some(c) = self.emit(Op::Cmp(CmpOp::Eq, x, y), Some(IrTy::Bool), sp) {
                    self.require_true(c, fail);
                }
                true
            }
            TyKind::String | TyKind::Str => {
                if let Some(c) = self.call_core("rt_str_eq", vec![a, b]) {
                    self.require_true(c, fail);
                }
                true
            }
            TyKind::Void | TyKind::Never => true,
            TyKind::Tuple(_) | TyKind::Range(_) => self.eq_fields(a, b, ty, fail),
            TyKind::Adt(d, _) => match self.res.def(d).kind {
                DefKind::Enum => self.eq_enum(a, b, ty, fail),
                DefKind::Obj => false,
                _ => self.eq_fields(a, b, ty, fail),
            },
            TyKind::Option(_) | TyKind::Result(..) => self.eq_enum(a, b, ty, fail),
            TyKind::Box(inner) => {
                let x = self.load_at(a, 0, IrTy::Ptr);
                let y = self.load_at(b, 0, IrTy::Ptr);
                self.eq_part(x, y, inner, fail)
            }
            TyKind::List(e) | TyKind::Iter(e) => {
                let la = self.load_at(a, 8, IrTy::I64);
                let lb = self.load_at(b, 8, IrTy::I64);
                if let Some(c) = self.emit(Op::Cmp(CmpOp::Eq, la, lb), Some(IrTy::Bool), sp) {
                    self.require_true(c, fail);
                }
                let da = self.load_at(a, 0, IrTy::Ptr);
                let db = self.load_at(b, 0, IrTy::Ptr);
                let esize = self.layout(e).size;
                let mut ok = true;
                self.each_elem(da, la, esize, |s, p, off| {
                    let q = s
                        .emit(Op::PtrAdd(db, off), Some(IrTy::Ptr), sp)
                        .unwrap_or(db);
                    ok &= s.eq_part(p, q, e, fail);
                });
                ok
            }
            TyKind::Array(e, len) => {
                let n = self.array_len(len);
                let nv = self.i64c(n);
                let esize = self.layout(e).size;
                let mut ok = true;
                self.each_elem(a, nv, esize, |s, p, off| {
                    let q = s.emit(Op::PtrAdd(b, off), Some(IrTy::Ptr), sp).unwrap_or(b);
                    ok &= s.eq_part(p, q, e, fail);
                });
                ok
            }
            TyKind::Set(_) | TyKind::Map(..) => self.eq_collection(a, b, ty, fail),
            TyKind::Any(_) => self.eq_any(a, b, fail),
            _ => false,
        }
    }

    fn eq_fields(&mut self, a: Value, b: Value, ty: Ty, fail: BlockId) -> bool {
        for f in self.struct_fields(ty) {
            let x = self.addr(a, f.off);
            let y = self.addr(b, f.off);
            if !self.eq_part(x, y, f.ty, fail) {
                return false;
            }
        }
        true
    }

    fn eq_enum(&mut self, a: Value, b: Value, ty: Ty, fail: BlockId) -> bool {
        let Some(info) = self.enum_info(ty) else {
            return false;
        };
        let sp = Span::new(0, 0);
        let ta = self.load_at(a, 0, info.tag);
        let tb = self.load_at(b, 0, info.tag);
        if let Some(c) = self.emit(Op::Cmp(CmpOp::Eq, ta, tb), Some(IrTy::Bool), sp) {
            self.require_true(c, fail);
        }
        let done = self.new_block(&[]);
        for (i, fields) in info.variants.iter().enumerate() {
            if fields.is_empty() {
                continue;
            }
            let want = self.konst(info.tag, Const::Int(i as i128), sp);
            let Some(c) = self.emit(Op::Cmp(CmpOp::Eq, ta, want), Some(IrTy::Bool), sp) else {
                continue;
            };
            let yes = self.new_block(&[]);
            let no = self.new_block(&[]);
            self.branch(c, yes, no);
            self.start(yes);
            for f in fields {
                let x = self.addr(a, f.off);
                let y = self.addr(b, f.off);
                if !self.eq_part(x, y, f.ty, fail) {
                    return false;
                }
            }
            self.jump(done, vec![]);
            self.start(no);
        }
        self.jump(done, vec![]);
        self.start(done);
        true
    }

    // ---- Ordering ----------------------------------------------------------------------------------------------------------

    pub(crate) fn cmp_fn_body(&mut self, a: Value, b: Value, ty: Ty) -> bool {
        if !self.cmp_part(a, b, ty) {
            return false;
        }
        let z = self.i64c(0);
        self.terminate(Term::Return(Some(z)));
        true
    }

    /// Returns early with the first nonzero comparison; falls through when the values are equal.
    fn cmp_part(&mut self, a: Value, b: Value, ty: Ty) -> bool {
        let sp = Span::new(0, 0);
        if let Some((m, map)) = self.own_method(ty, "cmp") {
            let Some(f) = self.request_fn(m, &map) else {
                return false;
            };
            let Some(r) = self.emit(Op::Call(f, vec![a, b]), Some(IrTy::I64), sp) else {
                return false;
            };
            self.return_if_nonzero(r);
            return true;
        }
        match self.kind(ty) {
            TyKind::Int(_) | TyKind::Float(_) | TyKind::Char | TyKind::Bool | TyKind::Ptr => {
                let s = self.scalar_of(ty).unwrap_or(IrTy::I64);
                let signed = self.is_signed(ty);
                let x = self.load_at(a, 0, s);
                let y = self.load_at(b, 0, s);
                let r = self.three_way(x, y, signed);
                self.return_if_nonzero(r);
                true
            }
            TyKind::String | TyKind::Str => {
                if let Some(r) = self.call_core("rt_str_cmp", vec![a, b]) {
                    self.return_if_nonzero(r);
                }
                true
            }
            TyKind::Void | TyKind::Never => true,
            TyKind::Tuple(_) => self.cmp_fields(a, b, ty),
            TyKind::Adt(d, _) => match self.res.def(d).kind {
                DefKind::Enum => self.cmp_enum(a, b, ty),
                DefKind::Obj => false,
                _ => self.cmp_fields(a, b, ty),
            },
            TyKind::Option(_) | TyKind::Result(..) => self.cmp_enum(a, b, ty),
            TyKind::Box(inner) => {
                let x = self.load_at(a, 0, IrTy::Ptr);
                let y = self.load_at(b, 0, IrTy::Ptr);
                self.cmp_part(x, y, inner)
            }
            TyKind::List(e) | TyKind::Iter(e) => {
                let la = self.load_at(a, 8, IrTy::I64);
                let lb = self.load_at(b, 8, IrTy::I64);
                // The shorter length bounds the element-wise part.
                let short = match self.emit(Op::Cmp(CmpOp::Lt, la, lb), Some(IrTy::Bool), sp) {
                    Some(c) => match self.select(c, la, lb, IrTy::I64, sp) {
                        Res::Val(v) => v,
                        _ => la,
                    },
                    None => la,
                };
                let da = self.load_at(a, 0, IrTy::Ptr);
                let db = self.load_at(b, 0, IrTy::Ptr);
                let esize = self.layout(e).size;
                let mut ok = true;
                self.each_elem(da, short, esize, |s, p, off| {
                    let q = s
                        .emit(Op::PtrAdd(db, off), Some(IrTy::Ptr), sp)
                        .unwrap_or(db);
                    ok &= s.cmp_part(p, q, e);
                });
                let r = self.three_way(la, lb, true);
                self.return_if_nonzero(r);
                ok
            }
            _ => false,
        }
    }

    fn return_if_nonzero(&mut self, r: Value) {
        let sp = Span::new(0, 0);
        let zero = self.i64c(0);
        let Some(c) = self.emit(Op::Cmp(CmpOp::Ne, r, zero), Some(IrTy::Bool), sp) else {
            return;
        };
        let ret = self.new_block(&[]);
        let go = self.new_block(&[]);
        self.branch(c, ret, go);
        self.start(ret);
        self.terminate(Term::Return(Some(r)));
        self.start(go);
    }

    fn cmp_fields(&mut self, a: Value, b: Value, ty: Ty) -> bool {
        for f in self.struct_fields(ty) {
            let x = self.addr(a, f.off);
            let y = self.addr(b, f.off);
            if !self.cmp_part(x, y, f.ty) {
                return false;
            }
        }
        true
    }

    fn cmp_enum(&mut self, a: Value, b: Value, ty: Ty) -> bool {
        let Some(info) = self.enum_info(ty) else {
            return false;
        };
        let sp = Span::new(0, 0);
        let ta = self.load_at(a, 0, info.tag);
        let tb = self.load_at(b, 0, info.tag);
        let ia = self.resize(ta, IrTy::I64, false, sp);
        let ib = self.resize(tb, IrTy::I64, false, sp);
        let r = self.three_way(ia, ib, true);
        self.return_if_nonzero(r);
        let done = self.new_block(&[]);
        for (i, fields) in info.variants.iter().enumerate() {
            if fields.is_empty() {
                continue;
            }
            let want = self.konst(info.tag, Const::Int(i as i128), sp);
            let Some(c) = self.emit(Op::Cmp(CmpOp::Eq, ta, want), Some(IrTy::Bool), sp) else {
                continue;
            };
            let yes = self.new_block(&[]);
            let no = self.new_block(&[]);
            self.branch(c, yes, no);
            self.start(yes);
            for f in fields {
                let x = self.addr(a, f.off);
                let y = self.addr(b, f.off);
                if !self.cmp_part(x, y, f.ty) {
                    return false;
                }
            }
            self.jump(done, vec![]);
            self.start(no);
        }
        self.jump(done, vec![]);
        self.start(done);
        true
    }

    // ---- Hashing -----------------------------------------------------------------------------------------------------------

    pub(crate) fn hash_fn_body(&mut self, a: Value, ty: Ty) -> bool {
        let acc = self.slot_bytes(8, 8);
        let seed = self.i64c(1469598103934665603);
        self.store_scalar_ty(acc, seed);
        if !self.hash_part(a, ty, acc) {
            return false;
        }
        let h = self.load_at(acc, 0, IrTy::I64);
        self.terminate(Term::Return(Some(h)));
        true
    }

    fn mix(&mut self, acc: Value, x: Value) {
        let sp = Span::new(0, 0);
        let cur = self.load_at(acc, 0, IrTy::I64);
        let k = self.i64c(1099511628211);
        let m = self
            .emit(Op::Bin(BinOp::Mul, cur, k), Some(IrTy::I64), sp)
            .unwrap_or(cur);
        let n = self
            .emit(Op::Bin(BinOp::Xor, m, x), Some(IrTy::I64), sp)
            .unwrap_or(m);
        self.store_scalar_ty(acc, n);
    }

    fn hash_part(&mut self, a: Value, ty: Ty, acc: Value) -> bool {
        let sp = Span::new(0, 0);
        if let Some((m, map)) = self.own_method(ty, "hash") {
            let Some(f) = self.request_fn(m, &map) else {
                return false;
            };
            let Some(r) = self.emit(Op::Call(f, vec![a]), Some(IrTy::I64), sp) else {
                return false;
            };
            self.mix(acc, r);
            return true;
        }
        match self.kind(ty) {
            TyKind::Int(_) | TyKind::Bool | TyKind::Char => {
                let s = self.scalar_of(ty).unwrap_or(IrTy::I64);
                let v = self.load_at(a, 0, s);
                let w = match s {
                    IrTy::Bool => {
                        let one = self.i64c(1);
                        let zero = self.i64c(0);
                        match self.select(v, one, zero, IrTy::I64, sp) {
                            Res::Val(x) => x,
                            _ => zero,
                        }
                    }
                    _ => {
                        let signed = self.is_signed(ty);
                        self.resize(v, IrTy::I64, signed, sp)
                    }
                };
                self.mix(acc, w);
                true
            }
            TyKind::String | TyKind::Str => {
                if let Some(h) = self.call_core("rt_str_hash", vec![a]) {
                    self.mix(acc, h);
                }
                true
            }
            TyKind::Tuple(_) => {
                for f in self.struct_fields(ty) {
                    let p = self.addr(a, f.off);
                    if !self.hash_part(p, f.ty, acc) {
                        return false;
                    }
                }
                true
            }
            TyKind::Adt(d, _) => match self.res.def(d).kind {
                DefKind::Enum => self.hash_enum(a, ty, acc),
                DefKind::Obj => false,
                _ => {
                    for f in self.struct_fields(ty) {
                        let p = self.addr(a, f.off);
                        if !self.hash_part(p, f.ty, acc) {
                            return false;
                        }
                    }
                    true
                }
            },
            TyKind::Option(_) | TyKind::Result(..) => self.hash_enum(a, ty, acc),
            TyKind::Box(inner) => {
                let p = self.load_at(a, 0, IrTy::Ptr);
                self.hash_part(p, inner, acc)
            }
            TyKind::Any(_) => match self.hash_any(a) {
                Some(h) => {
                    self.mix(acc, h);
                    true
                }
                None => false,
            },
            _ => false,
        }
    }

    fn hash_enum(&mut self, a: Value, ty: Ty, acc: Value) -> bool {
        let Some(info) = self.enum_info(ty) else {
            return false;
        };
        let sp = Span::new(0, 0);
        let tag = self.load_at(a, 0, info.tag);
        let t = self.resize(tag, IrTy::I64, false, sp);
        self.mix(acc, t);
        let done = self.new_block(&[]);
        for (i, fields) in info.variants.iter().enumerate() {
            if fields.is_empty() {
                continue;
            }
            let want = self.konst(info.tag, Const::Int(i as i128), sp);
            let Some(c) = self.emit(Op::Cmp(CmpOp::Eq, tag, want), Some(IrTy::Bool), sp) else {
                continue;
            };
            let yes = self.new_block(&[]);
            let no = self.new_block(&[]);
            self.branch(c, yes, no);
            self.start(yes);
            for f in fields {
                let p = self.addr(a, f.off);
                if !self.hash_part(p, f.ty, acc) {
                    return false;
                }
            }
            self.jump(done, vec![]);
            self.start(no);
        }
        self.jump(done, vec![]);
        self.start(done);
        true
    }

    // ---- Printing ------------------------------------------------------------------------------------------------------------

    pub(crate) fn show_body(&mut self, v: Value, out: Value, ty: Ty) {
        let sp = Span::new(0, 0);
        if let Some((m, map)) = self.own_method(ty, "show") {
            // The type's own `show` makes a string, which is appended and dropped.
            let Some(f) = self.request_fn(m, &map) else {
                return;
            };
            let string = self.types.string;
            let tmp = self.slot_for(string);
            let arg = if self.obj_def(ty).is_some() {
                self.load_at(v, 0, IrTy::Ptr)
            } else {
                v
            };
            self.emit(Op::Call(f, vec![tmp, arg]), None, sp);
            let d = self.load_at(tmp, 0, IrTy::Ptr);
            let n = self.load_at(tmp, 8, IrTy::I64);
            self.call_core("rt_str_push", vec![out, d, n]);
            self.drop_at(tmp, string);
            return;
        }
        match self.kind(ty) {
            TyKind::List(e) | TyKind::Iter(e) => {
                self.push_static(out, b"[");
                let len = self.load_at(v, 8, IrTy::I64);
                let data = self.load_at(v, 0, IrTy::Ptr);
                let esize = self.layout(e).size;
                self.each_elem(data, len, esize, |s, p, off| {
                    let zero = s.i64c(0);
                    if let Some(c) = s.emit(Op::Cmp(CmpOp::Ne, off, zero), Some(IrTy::Bool), sp) {
                        s.if_then_pub(c, |s| s.push_static(out, b", "));
                    }
                    s.show_at(p, e, out, true);
                });
                self.push_static(out, b"]");
            }
            TyKind::Soa(e, _) => {
                // A list of the elements, each gathered from the columns.
                self.push_static(out, b"[");
                let fields = self.soa_fields(e);
                if let Some((_, ft0)) = fields.first().copied() {
                    let len = self.soa_len(v);
                    let data = self.load_at(v, 0, IrTy::Ptr);
                    let esize = self.layout(ft0).size;
                    self.each_elem(data, len, esize, |s, _p, off| {
                        let zero = s.i64c(0);
                        if let Some(c) = s.emit(Op::Cmp(CmpOp::Ne, off, zero), Some(IrTy::Bool), sp)
                        {
                            s.if_then_pub(c, |s| s.push_static(out, b", "));
                        }
                        let isz = s.i64c(esize as i64);
                        if let Some(i) = s.emit(Op::Bin(BinOp::Div, off, isz), Some(IrTy::I64), sp)
                        {
                            let tmp = s.slot_for(e);
                            s.soa_gather(v, i, tmp, e);
                            s.show_at(tmp, e, out, true);
                        }
                    });
                }
                self.push_static(out, b"]");
            }
            TyKind::Array(e, len) => {
                self.push_static(out, b"[");
                let n = self.array_len(len);
                let nv = self.i64c(n);
                let esize = self.layout(e).size;
                self.each_elem(v, nv, esize, |s, p, off| {
                    let zero = s.i64c(0);
                    if let Some(c) = s.emit(Op::Cmp(CmpOp::Ne, off, zero), Some(IrTy::Bool), sp) {
                        s.if_then_pub(c, |s| s.push_static(out, b", "));
                    }
                    s.show_at(p, e, out, true);
                });
                self.push_static(out, b"]");
            }
            TyKind::Option(inner) => {
                let Some(info) = self.enum_info(ty) else {
                    return;
                };
                let tag = self.load_at(v, 0, info.tag);
                let zero = self.konst(info.tag, Const::Int(0), sp);
                if let Some(is_none) =
                    self.emit(Op::Cmp(CmpOp::Eq, tag, zero), Some(IrTy::Bool), sp)
                {
                    let none_b = self.new_block(&[]);
                    let some_b = self.new_block(&[]);
                    let done = self.new_block(&[]);
                    self.branch(is_none, none_b, some_b);
                    self.start(none_b);
                    self.push_static(out, b"none");
                    self.jump(done, vec![]);
                    self.start(some_b);
                    self.push_static(out, b"some(");
                    let p = self.addr(v, info.variants[1][0].off);
                    self.show_at(p, inner, out, true);
                    self.push_static(out, b")");
                    self.jump(done, vec![]);
                    self.start(done);
                }
            }
            TyKind::Result(..) => {
                let Some(info) = self.enum_info(ty) else {
                    return;
                };
                let tag = self.load_at(v, 0, info.tag);
                let zero = self.konst(info.tag, Const::Int(0), sp);
                if let Some(is_ok) = self.emit(Op::Cmp(CmpOp::Eq, tag, zero), Some(IrTy::Bool), sp)
                {
                    let ok_b = self.new_block(&[]);
                    let err_b = self.new_block(&[]);
                    let done = self.new_block(&[]);
                    self.branch(is_ok, ok_b, err_b);
                    self.start(ok_b);
                    self.push_static(out, b"ok(");
                    if let Some(f) = info.variants[0].first() {
                        let p = self.addr(v, f.off);
                        self.show_at(p, f.ty, out, true);
                    }
                    self.push_static(out, b")");
                    self.jump(done, vec![]);
                    self.start(err_b);
                    self.push_static(out, b"err(");
                    if let Some(f) = info.variants[1].first() {
                        let p = self.addr(v, f.off);
                        self.show_at(p, f.ty, out, true);
                    }
                    self.push_static(out, b")");
                    self.jump(done, vec![]);
                    self.start(done);
                }
            }
            TyKind::Tuple(_) => {
                self.push_static(out, b"(");
                for (i, f) in self.struct_fields(ty).into_iter().enumerate() {
                    if i > 0 {
                        self.push_static(out, b", ");
                    }
                    let p = self.addr(v, f.off);
                    self.show_at(p, f.ty, out, true);
                }
                self.push_static(out, b")");
            }
            TyKind::Box(inner) => {
                let p = self.load_at(v, 0, IrTy::Ptr);
                self.show_at(p, inner, out, false);
            }
            TyKind::Error => {
                let string = self.types.string;
                self.show_at(v, string, out, false);
            }
            TyKind::Fn(..) => self.push_static(out, b"<function>"),
            TyKind::Any(i) => self.show_any(v, out, i),
            TyKind::Adt(d, _) => match self.res.def(d).kind {
                DefKind::Enum => self.show_enum(v, out, ty, d),
                DefKind::Struct | DefKind::Obj => {
                    let name = self.res.def(d).name.clone();
                    self.push_static(out, name.as_bytes());
                    let v = if self.res.def(d).kind == DefKind::Obj {
                        let o = self.load_at(v, 0, IrTy::Ptr);
                        self.addr(o, super::obj::OBJ_HEADER)
                    } else {
                        v
                    };
                    let fs = self.struct_fields(ty);
                    if !fs.is_empty() {
                        let decls: Vec<DefId> = self
                            .typed
                            .adts
                            .get(&d)
                            .map(|a| a.fields.iter().map(|(f, _)| *f).collect())
                            .unwrap_or_default();
                        self.push_static(out, b" { ");
                        for (i, (f, fd)) in fs.iter().zip(decls.iter()).enumerate() {
                            if i > 0 {
                                self.push_static(out, b", ");
                            }
                            let fname = self.res.def(*fd).name.clone();
                            self.push_static(out, fname.as_bytes());
                            self.push_static(out, b": ");
                            let p = self.addr(v, f.off);
                            self.show_at(p, f.ty, out, true);
                        }
                        self.push_static(out, b" }");
                    }
                }
                _ => {
                    self.unsupported(sp, "printing an object");
                }
            },
            TyKind::Map(..) | TyKind::Set(_) => self.show_collection(v, out, ty),
            _ => {
                self.unsupported(sp, "printing this type");
            }
        }
    }

    fn show_enum(&mut self, v: Value, out: Value, ty: Ty, d: DefId) {
        let Some(info) = self.enum_info(ty) else {
            return;
        };
        let sp = Span::new(0, 0);
        let tag = self.load_at(v, 0, info.tag);
        let variants: Vec<DefId> = self
            .typed
            .adts
            .get(&d)
            .map(|a| a.variants.iter().map(|(v, _)| *v).collect())
            .unwrap_or_default();
        let done = self.new_block(&[]);
        for (i, vd) in variants.iter().enumerate() {
            let want = self.konst(info.tag, Const::Int(i as i128), sp);
            let Some(c) = self.emit(Op::Cmp(CmpOp::Eq, tag, want), Some(IrTy::Bool), sp) else {
                continue;
            };
            let yes = self.new_block(&[]);
            let no = self.new_block(&[]);
            self.branch(c, yes, no);
            self.start(yes);
            let name = self.res.def(*vd).name.clone();
            self.push_static(out, name.as_bytes());
            let fields = info.variants[i].clone();
            if !fields.is_empty() {
                self.push_static(out, b"(");
                for (k, f) in fields.iter().enumerate() {
                    if k > 0 {
                        self.push_static(out, b", ");
                    }
                    let p = self.addr(v, f.off);
                    self.show_at(p, f.ty, out, true);
                }
                self.push_static(out, b")");
            }
            self.jump(done, vec![]);
            self.start(no);
        }
        self.jump(done, vec![]);
        self.start(done);
    }
}
