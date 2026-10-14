//! Glue functions generated per type: dropping, deep copying, comparing, hashing and printing.
//!
//! Each is an ordinary IR function over addresses, built on demand and shared by every use of the
//! type. Types that own nothing need no drop or clone glue (a clone is a plain copy of bytes).

use super::*;

impl<'a> Lowerer<'a> {
    /// The glue function of this kind for a type, or `None` when the type needs none.
    pub(crate) fn glue_fn(&mut self, kind: GlueKind, ty: Ty) -> Option<FuncId> {
        let ty = self.types.zonk(ty);
        if let Some(r) = self.glue.get(&(kind, ty)) {
            return *r;
        }
        let wanted = match kind {
            GlueKind::Drop | GlueKind::Clone => self.needs_drop(ty),
            _ => true,
        };
        if !wanted {
            self.glue.insert((kind, ty), None);
            return None;
        }
        let (params, ret): (Vec<IrTy>, IrTy) = match kind {
            GlueKind::Drop => (vec![IrTy::Ptr], IrTy::Void),
            GlueKind::Clone => (vec![IrTy::Ptr, IrTy::Ptr], IrTy::Void),
            GlueKind::Eq => (vec![IrTy::Ptr, IrTy::Ptr], IrTy::Bool),
            GlueKind::Cmp => (vec![IrTy::Ptr, IrTy::Ptr], IrTy::I64),
            GlueKind::Hash => (vec![IrTy::Ptr], IrTy::I64),
            GlueKind::Show => (vec![IrTy::Ptr, IrTy::Ptr], IrTy::Void),
        };
        let label = match kind {
            GlueKind::Drop => "drop",
            GlueKind::Clone => "clone",
            GlueKind::Eq => "eq",
            GlueKind::Cmp => "cmp",
            GlueKind::Hash => "hash",
            GlueKind::Show => "show",
        };
        let name = self.unique_name(format!("{label}<{}>", self.show_ty(ty)));
        let mut func = Func::new(&name, &params, ret);
        let e = func.entry();
        func.terminate(e, Term::Stop("not lowered yet".to_string()));
        let id = self.module.add(func);
        self.glue.insert((kind, ty), Some(id));
        self.jobs.push(Job::Glue { id, kind, ty });
        Some(id)
    }

    pub(crate) fn lower_glue_job(&mut self, id: FuncId, kind: GlueKind, ty: Ty) {
        let placeholder = self.module.funcs[id.0 as usize].clone();
        let void = self.types.void;
        let mut state = FnState::empty(void);
        state.func = Func::new(&placeholder.name, &placeholder.params, placeholder.ret);
        state.cur = Some(state.func.entry());
        self.f = state;
        let entry = self.f.func.entry();
        let ps = self.f.func.block_params(entry).to_vec();
        match kind {
            GlueKind::Drop => {
                self.drop_body(ps[0], ty);
                self.terminate(Term::Return(None));
            }
            GlueKind::Clone => {
                self.clone_body(ps[0], ps[1], ty);
                self.terminate(Term::Return(None));
            }
            GlueKind::Eq => {
                if !self.eq_fn_body(ps[0], ps[1], ty) {
                    self.unsupported(Span::new(0, 0), "comparing this type");
                }
            }
            GlueKind::Cmp => {
                if !self.cmp_fn_body(ps[0], ps[1], ty) {
                    self.unsupported(Span::new(0, 0), "ordering this type");
                }
            }
            GlueKind::Hash => {
                if !self.hash_fn_body(ps[0], ty) {
                    self.unsupported(Span::new(0, 0), "hashing this type");
                }
            }
            GlueKind::Show => {
                self.show_body(ps[0], ps[1], ty);
                self.terminate(Term::Return(None));
            }
        }
        let st = std::mem::replace(&mut self.f, FnState::empty(void));
        if st.fail.is_none() {
            let mut func = st.func;
            func.compact();
            self.module.funcs[id.0 as usize] = func;
        } else if let Some((span, msg)) = st.fail {
            self.diags.push(Diagnostic {
                module: ModuleId(0),
                span,
                message: format!("not lowered yet: {msg} (in `{}`)", placeholder.name),
                warning: true,
            });
        }
    }

    // ---- Small IR helpers for glue ---------------------------------------------------------------------------------

    /// Calls the drop glue of `ty` on the value at `p` (nothing when the type owns nothing).
    pub(crate) fn drop_at(&mut self, p: Value, ty: Ty) {
        if let Some(g) = self.glue_fn(GlueKind::Drop, ty) {
            self.emit(Op::Call(g, vec![p]), None, Span::new(0, 0));
        }
    }

    /// Runs `body(i_address)` for each element of a buffer of `len` elements of `esize` bytes.
    pub(crate) fn each_elem(
        &mut self,
        data: Value,
        len: Value,
        esize: u32,
        body: impl FnMut(&mut Self, Value, Value),
    ) {
        self.each_element(data, len, esize, body)
    }

    fn each_element(
        &mut self,
        data: Value,
        len: Value,
        esize: u32,
        mut body: impl FnMut(&mut Self, Value, Value),
    ) {
        let head = self.new_block(&[IrTy::I64]);
        let step = self.new_block(&[]);
        let done = self.new_block(&[]);
        let zero = self.i64c(0);
        self.jump(head, vec![zero]);
        self.start(head);
        let i = self.f.func.block_params(head)[0];
        let more = self.emit(
            Op::Cmp(CmpOp::Lt, i, len),
            Some(IrTy::Bool),
            Span::new(0, 0),
        );
        let go = self.new_block(&[]);
        if let Some(m) = more {
            self.branch(m, go, done);
        }
        self.start(go);
        let sz = self.i64c(esize as i64);
        let off = self
            .emit(Op::Bin(BinOp::Mul, i, sz), Some(IrTy::I64), Span::new(0, 0))
            .unwrap_or(i);
        let p = self
            .emit(Op::PtrAdd(data, off), Some(IrTy::Ptr), Span::new(0, 0))
            .unwrap_or(data);
        body(self, p, off);
        self.jump(step, vec![]);
        self.start(step);
        let one = self.i64c(1);
        let next = self
            .emit(
                Op::Bin(BinOp::Add, i, one),
                Some(IrTy::I64),
                Span::new(0, 0),
            )
            .unwrap_or(i);
        self.jump(head, vec![next]);
        self.start(done);
    }

    /// `if cond { ... }` around code emitted by `f`.
    pub(crate) fn if_then_pub(&mut self, cond: Value, f: impl FnOnce(&mut Self)) {
        self.if_then(cond, f)
    }

    fn if_then(&mut self, cond: Value, f: impl FnOnce(&mut Self)) {
        let then = self.new_block(&[]);
        let after = self.new_block(&[]);
        self.branch(cond, then, after);
        self.start(then);
        f(self);
        if self.f.cur.is_some() {
            self.jump(after, vec![]);
        }
        self.start(after);
    }

    fn cmp_i64_gt0(&mut self, v: Value) -> Value {
        let zero = self.i64c(0);
        self.emit(
            Op::Cmp(CmpOp::Gt, v, zero),
            Some(IrTy::Bool),
            Span::new(0, 0),
        )
        .unwrap_or(v)
    }

    /// Calls the user's `deinit` for a type that has one.
    fn call_deinit(&mut self, p: Value, d: DefId, args: &[Ty]) {
        if let Some(m) = self.res.member(d, "deinit") {
            if self.fns.contains_key(&m) {
                let map = self.adt_subst(d, args);
                if let Some(f) = self.request_fn(m, &map) {
                    self.emit(Op::Call(f, vec![p]), None, Span::new(0, 0));
                }
            }
        }
    }

    // ---- Drop -----------------------------------------------------------------------------------------------------------

    fn drop_body(&mut self, p: Value, ty: Ty) {
        match self.kind(ty) {
            TyKind::String => {
                let cap = self.load_at(p, 16, IrTy::I64);
                let c = self.cmp_i64_gt0(cap);
                self.if_then(c, |s| {
                    let data = s.load_at(p, 0, IrTy::Ptr);
                    s.emit(Op::Free(data), None, Span::new(0, 0));
                });
            }
            TyKind::List(e) | TyKind::Iter(e) => {
                let cap = self.load_at(p, 16, IrTy::I64);
                let c = self.cmp_i64_gt0(cap);
                self.if_then(c, |s| {
                    let data = s.load_at(p, 0, IrTy::Ptr);
                    if s.needs_drop(e) {
                        let len = s.load_at(p, 8, IrTy::I64);
                        let esize = s.layout(e).size;
                        s.each_element(data, len, esize, |s, q, _| s.drop_at(q, e));
                    }
                    s.emit(Op::Free(data), None, Span::new(0, 0));
                });
            }
            TyKind::Box(inner) => {
                let q = self.load_at(p, 0, IrTy::Ptr);
                self.drop_at(q, inner);
                self.emit(Op::Free(q), None, Span::new(0, 0));
            }
            TyKind::Error => {
                let s = self.types.string;
                self.drop_at(p, s);
            }
            TyKind::Tuple(_) => self.drop_fields(p, ty),
            TyKind::Adt(d, args) => match self.res.def(d).kind {
                DefKind::Obj => self.drop_object(p, ty),
                DefKind::Enum => {
                    self.call_deinit(p, d, &args);
                    self.drop_enum(p, ty);
                }
                _ => {
                    self.call_deinit(p, d, &args);
                    self.drop_fields(p, ty);
                }
            },
            TyKind::Option(_) | TyKind::Result(..) => self.drop_enum(p, ty),
            TyKind::Map(..) | TyKind::Set(_) => self.drop_collection(p, ty),
            TyKind::Any(_) => self.drop_any(p, ty),
            _ => {
                self.unsupported(Span::new(0, 0), "dropping this type");
            }
        }
    }

    fn drop_fields(&mut self, p: Value, ty: Ty) {
        for f in self.struct_fields(ty) {
            if self.needs_drop(f.ty) {
                let q = self.addr(p, f.off);
                self.drop_at(q, f.ty);
            }
        }
    }

    fn drop_enum(&mut self, p: Value, ty: Ty) {
        let Some(info) = self.enum_info(ty) else {
            return;
        };
        let tag = self.load_at(p, 0, info.tag);
        let done = self.new_block(&[]);
        for (i, fields) in info.variants.iter().enumerate() {
            let owned: Vec<Field> = fields
                .iter()
                .copied()
                .filter(|f| self.needs_drop(f.ty))
                .collect();
            if owned.is_empty() {
                continue;
            }
            let want = self.konst(info.tag, Const::Int(i as i128), Span::new(0, 0));
            let Some(c) = self.emit(
                Op::Cmp(CmpOp::Eq, tag, want),
                Some(IrTy::Bool),
                Span::new(0, 0),
            ) else {
                continue;
            };
            let yes = self.new_block(&[]);
            let no = self.new_block(&[]);
            self.branch(c, yes, no);
            self.start(yes);
            for f in owned {
                let q = self.addr(p, f.off);
                self.drop_at(q, f.ty);
            }
            self.jump(done, vec![]);
            self.start(no);
        }
        self.jump(done, vec![]);
        self.start(done);
    }

    // ---- Clone ----------------------------------------------------------------------------------------------------------

    /// Copies the value at `src` into the (uninitialised) memory at `dst`, duplicating what it owns.
    fn clone_body(&mut self, src: Value, dst: Value, ty: Ty) {
        match self.kind(ty) {
            TyKind::String => {
                let len = self.load_at(src, 8, IrTy::I64);
                let data = self.load_at(src, 0, IrTy::Ptr);
                let buf = self
                    .emit(Op::Alloc(len), Some(IrTy::Ptr), Span::new(0, 0))
                    .unwrap_or(data);
                self.emit(Op::MemCopy(buf, data, len), None, Span::new(0, 0));
                self.store_at(dst, 0, buf);
                self.store_at(dst, 8, len);
                // An empty copy still owns its (zero-size) block; a capacity of at least 1 says so.
                let one = self.i64c(1);
                let big = self.emit(
                    Op::Cmp(CmpOp::Lt, len, one),
                    Some(IrTy::Bool),
                    Span::new(0, 0),
                );
                let cap = match big {
                    Some(b) => match self.select(b, one, len, IrTy::I64, Span::new(0, 0)) {
                        Res::Val(v) => v,
                        _ => len,
                    },
                    None => len,
                };
                self.store_at(dst, 16, cap);
            }
            TyKind::List(e) | TyKind::Iter(e) => {
                let len = self.load_at(src, 8, IrTy::I64);
                let data = self.load_at(src, 0, IrTy::Ptr);
                let esize = self.layout(e).size;
                let sz = self.i64c(esize as i64);
                let bytes = self
                    .emit(
                        Op::Bin(BinOp::Mul, len, sz),
                        Some(IrTy::I64),
                        Span::new(0, 0),
                    )
                    .unwrap_or(len);
                let buf = self
                    .emit(Op::Alloc(bytes), Some(IrTy::Ptr), Span::new(0, 0))
                    .unwrap_or(data);
                self.store_at(dst, 0, buf);
                self.store_at(dst, 8, len);
                let one = self.i64c(1);
                let small = self.emit(
                    Op::Cmp(CmpOp::Lt, len, one),
                    Some(IrTy::Bool),
                    Span::new(0, 0),
                );
                let cap = match small {
                    Some(b) => match self.select(b, one, len, IrTy::I64, Span::new(0, 0)) {
                        Res::Val(v) => v,
                        _ => len,
                    },
                    None => len,
                };
                self.store_at(dst, 16, cap);
                if self.needs_drop(e) {
                    // Element by element.
                    self.each_element(data, len, esize, |s, q, off| {
                        let d = s
                            .emit(Op::PtrAdd(buf, off), Some(IrTy::Ptr), Span::new(0, 0))
                            .unwrap_or(buf);
                        s.clone_into(q, d, e);
                    });
                } else {
                    self.emit(Op::MemCopy(buf, data, bytes), None, Span::new(0, 0));
                }
            }
            TyKind::Box(inner) => {
                let q = self.load_at(src, 0, IrTy::Ptr);
                let size = self.layout(inner).size.max(1);
                let n = self.i64c(size as i64);
                let buf = self
                    .emit(Op::Alloc(n), Some(IrTy::Ptr), Span::new(0, 0))
                    .unwrap_or(q);
                self.clone_into(q, buf, inner);
                self.store_at(dst, 0, buf);
            }
            TyKind::Error => {
                let s = self.types.string;
                self.clone_body(src, dst, s);
            }
            TyKind::Tuple(_) => self.clone_fields(src, dst, ty),
            TyKind::Adt(d, _) => match self.res.def(d).kind {
                DefKind::Obj => self.clone_object(src, dst, ty),
                DefKind::Enum => self.clone_enum(src, dst, ty),
                _ => self.clone_fields(src, dst, ty),
            },
            TyKind::Option(_) | TyKind::Result(..) => self.clone_enum(src, dst, ty),
            TyKind::Map(..) | TyKind::Set(_) => self.clone_collection(src, dst, ty),
            TyKind::Any(_) => self.clone_any(src, dst, ty),
            _ => {
                self.unsupported(Span::new(0, 0), "copying this type");
            }
        }
    }

    fn clone_fields(&mut self, src: Value, dst: Value, ty: Ty) {
        let l = self.layout(ty);
        self.copy_bytes(dst, src, l.size);
        for f in self.struct_fields(ty) {
            if self.needs_drop(f.ty) {
                let s = self.addr(src, f.off);
                let d = self.addr(dst, f.off);
                self.clone_into(s, d, f.ty);
            }
        }
    }

    fn clone_enum(&mut self, src: Value, dst: Value, ty: Ty) {
        let Some(info) = self.enum_info(ty) else {
            return;
        };
        // Copy everything, then redo the cases that own something.
        self.copy_bytes(dst, src, info.layout.size);
        let tag = self.load_at(src, 0, info.tag);
        let done = self.new_block(&[]);
        for (i, fields) in info.variants.iter().enumerate() {
            let owned: Vec<Field> = fields
                .iter()
                .copied()
                .filter(|f| self.needs_drop(f.ty))
                .collect();
            if owned.is_empty() {
                continue;
            }
            let want = self.konst(info.tag, Const::Int(i as i128), Span::new(0, 0));
            let Some(c) = self.emit(
                Op::Cmp(CmpOp::Eq, tag, want),
                Some(IrTy::Bool),
                Span::new(0, 0),
            ) else {
                continue;
            };
            let yes = self.new_block(&[]);
            let no = self.new_block(&[]);
            self.branch(c, yes, no);
            self.start(yes);
            for f in owned {
                let s = self.addr(src, f.off);
                let d = self.addr(dst, f.off);
                self.clone_into(s, d, f.ty);
            }
            self.jump(done, vec![]);
            self.start(no);
        }
        self.jump(done, vec![]);
        self.start(done);
    }

    // ---- Parts for later steps ---------------------------------------------------------------------------------------------

    fn drop_object(&mut self, _p: Value, _ty: Ty) {
        self.unsupported(Span::new(0, 0), "dropping an object");
    }
    fn clone_object(&mut self, _s: Value, _d: Value, _ty: Ty) {
        self.unsupported(Span::new(0, 0), "copying an object");
    }
    fn drop_any(&mut self, _p: Value, _ty: Ty) {
        self.unsupported(Span::new(0, 0), "dropping an `any`");
    }
    fn clone_any(&mut self, _s: Value, _d: Value, _ty: Ty) {
        self.unsupported(Span::new(0, 0), "copying an `any`");
    }
}
