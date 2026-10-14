//! Building aggregates: tuples, structs, enum cases, `option` and `result` values, and the
//! implicit conversions that put a value into a differently shaped destination.

use super::*;

impl<'a> Lowerer<'a> {
    // ---- Conversions at a destination --------------------------------------------------------------------------

    /// When the destination type differs from the expression's type: a value into an `option`, a
    /// `string` into a `str` view, an `obj` into its base. `None` means no conversion applies.
    pub(crate) fn coerce_into(
        &mut self,
        e: &'a Expr,
        have: Ty,
        dst: Value,
        want: Ty,
    ) -> Option<Res> {
        let (hk, wk) = (self.kind(have), self.kind(want));
        match (hk, wk) {
            (TyKind::Never, _) => Some(self.expr(e)),
            (_, TyKind::Option(inner)) if !matches!(self.kind(have), TyKind::Option(_)) => {
                let info = self.enum_info(want)?;
                let some = info.variants.get(1)?.first().copied()?;
                let p = self.at(dst, some.off);
                let r = self.into(e, p, inner);
                if matches!(r, Res::Never) {
                    return Some(r);
                }
                let tag = self.konst(info.tag, Const::Int(1), e.span);
                self.store_at(dst, 0, tag);
                Some(Res::Unit)
            }
            (TyKind::String, TyKind::Str) => {
                let Res::Val(h) = self.expr(e) else {
                    return Some(Res::Never);
                };
                let p = self.load_at(h, 0, IrTy::Ptr);
                let n = self.load_at(h, 8, IrTy::I64);
                self.store_at(dst, 0, p);
                self.store_at(dst, 8, n);
                Some(Res::Unit)
            }
            (TyKind::Adt(a, _), TyKind::Adt(b, _))
                if a != b && self.res.def(a).kind == DefKind::Obj =>
            {
                // An object used as its base: the same address.
                Some(self.into_exact(e, dst, have))
            }
            (TyKind::Array(_, len), TyKind::List(_) | TyKind::Iter(_)) => {
                // A fixed array used as a list: a view of its elements.
                let n = self.array_len(len);
                let (p, _) = self.scrutinee(e)?;
                let nv = self.i64c(n);
                let zero = self.i64c(0);
                self.store_at(dst, 0, p);
                self.store_at(dst, 8, nv);
                self.store_at(dst, 16, zero);
                Some(Res::Unit)
            }
            (_, TyKind::Any(iface)) => self.coerce_to_any(e, have, dst, iface),
            _ => None,
        }
    }

    /// An address `off` bytes into a value (computed now, before the arguments that use it).
    pub(crate) fn at(&mut self, base: Value, off: u32) -> Value {
        self.addr(base, off)
    }

    // ---- Tuples, structs --------------------------------------------------------------------------------------------

    pub(crate) fn tuple_lit(&mut self, e: &'a Expr, es: &'a [Expr], dst: Value, t: Ty) -> Res {
        let fs = self.struct_fields(t);
        if fs.len() != es.len() {
            return self.unsupported(e.span, "a tuple of this shape");
        }
        for (x, f) in es.iter().zip(fs.iter()) {
            let p = self.at(dst, f.off);
            if let Res::Never = self.into(x, p, f.ty) {
                return Res::Never;
            }
        }
        Res::Unit
    }

    pub(crate) fn struct_lit(
        &mut self,
        e: &'a Expr,
        inits: &'a [StructInit],
        dst: Value,
        t: Ty,
    ) -> Res {
        let TyKind::Adt(d, _) = self.kind(t) else {
            return self.unsupported(e.span, "a struct literal of this type");
        };
        if self.res.def(d).kind == DefKind::Obj {
            return self.obj_literal(e, inits, dst, t);
        }
        self.fill_struct(e, inits, dst, t)
    }

    /// Writes the fields of a struct value at `dst` (also used for the part of an `obj` after its header).
    pub(crate) fn fill_struct(
        &mut self,
        e: &'a Expr,
        inits: &'a [StructInit],
        dst: Value,
        t: Ty,
    ) -> Res {
        let TyKind::Adt(d, _) = self.kind(t) else {
            return self.unsupported(e.span, "a struct literal of this type");
        };
        let fs = self.struct_fields(t);
        let decls: Vec<DefId> = self
            .typed
            .adts
            .get(&d)
            .map(|a| a.fields.iter().map(|(f, _)| *f).collect())
            .unwrap_or_default();
        // The spread source, evaluated once.
        let mut spread: Option<Value> = None;
        for i in inits {
            if let StructInit::Spread(b) = i {
                match self.expr(b) {
                    Res::Val(p) => spread = Some(p),
                    _ => return Res::Never,
                }
            }
        }
        for (fd, f) in decls.iter().zip(fs.iter()) {
            let name = self.res.def(*fd).name.clone();
            let p = self.at(dst, f.off);
            let init = inits.iter().find_map(|i| match i {
                StructInit::Field(fi) if self.src_text(fi.name.span) == name => Some(fi),
                _ => None,
            });
            match init {
                Some(fi) => match &fi.value {
                    Some(v) => {
                        if let Res::Never = self.into(v, p, f.ty) {
                            return Res::Never;
                        }
                    }
                    None => {
                        // `point { x, y }`: the variable of the same name.
                        let Some(def) = self.res.use_at(self.f.file, fi.name.span.start) else {
                            return self.unsupported(fi.name.span, "a field shorthand");
                        };
                        self.move_local_into(def, p, f.ty, fi.name.span);
                    }
                },
                None => match spread {
                    Some(base) => {
                        let src = self.at(base, f.off);
                        self.clone_into(src, p, f.ty);
                    }
                    None => {
                        let Some(&(file, dflt)) = self.field_defaults.get(fd) else {
                            return self.unsupported(e.span, "a field without a value");
                        };
                        let saved = self.f.file;
                        self.f.file = file;
                        let r = self.into(dflt, p, f.ty);
                        self.f.file = saved;
                        if let Res::Never = r {
                            return Res::Never;
                        }
                    }
                },
            }
        }
        Res::Unit
    }

    /// Writes the default value of every field that has one.
    pub(crate) fn fill_defaults(&mut self, dst: Value, t: Ty) {
        let TyKind::Adt(d, _) = self.kind(t) else {
            return;
        };
        let fs = self.struct_fields(t);
        let decls: Vec<DefId> = self
            .typed
            .adts
            .get(&d)
            .map(|a| a.fields.iter().map(|(f, _)| *f).collect())
            .unwrap_or_default();
        for (fd, f) in decls.iter().zip(fs.iter()) {
            if let Some(&(file, dflt)) = self.field_defaults.get(fd) {
                let p = self.at(dst, f.off);
                let saved = self.f.file;
                self.f.file = file;
                self.into(dflt, p, f.ty);
                self.f.file = saved;
            }
        }
    }

    /// Moves (or copies) the value of a variable to an address.
    pub(crate) fn move_local_into(&mut self, def: DefId, dst: Value, t: Ty, span: Span) {
        match self.f.locals.get(&def).copied() {
            Some(Local::Val(v)) => self.store_scalar_ty(dst, v),
            Some(Local::Slot { ptr, ty, flag }) => match self.repr(ty) {
                Repr::Scalar(s) => {
                    if let Some(v) = self.emit(Op::Load(s, ptr), Some(s), span) {
                        self.store_scalar_ty(dst, v);
                    }
                    if let Some(f) = flag {
                        self.clear_flag(f);
                    }
                }
                Repr::Mem(_) => {
                    self.copy_mem(dst, ptr, t);
                    if let Some(f) = flag {
                        self.clear_flag(f);
                    }
                }
                Repr::Zst => {}
            },
            None => {
                self.unsupported(span, "a variable here");
            }
        }
    }

    /// A deep copy of the value at `src` into `dst` (a plain copy of bytes when nothing is owned).
    pub(crate) fn clone_into(&mut self, src: Value, dst: Value, t: Ty) {
        match self.repr(t) {
            Repr::Zst => {}
            Repr::Scalar(s) => {
                if self.needs_drop(t) {
                    if let Some(g) = self.glue_fn(GlueKind::Clone, t) {
                        self.emit(Op::Call(g, vec![src, dst]), None, Span::new(0, 0));
                        return;
                    }
                }
                let v = self.load_at(src, 0, s);
                self.store_scalar_ty(dst, v);
            }
            Repr::Mem(_) => {
                if self.needs_drop(t) {
                    if let Some(g) = self.glue_fn(GlueKind::Clone, t) {
                        self.emit(Op::Call(g, vec![src, dst]), None, Span::new(0, 0));
                        return;
                    }
                }
                self.copy_mem(dst, src, t);
            }
        }
    }

    // ---- Enums, options, results -------------------------------------------------------------------------------------

    /// Runs `f` with an address for the result: `dst`, or a new temporary owned by the scope.
    pub(crate) fn with_dst(
        &mut self,
        dst: Option<Value>,
        t: Ty,
        f: impl FnOnce(&mut Self, Value) -> Res,
    ) -> Res {
        match dst {
            Some(d) => f(self, d),
            None => {
                let slot = self.slot_for(t);
                let r = f(self, slot);
                if matches!(r, Res::Never) {
                    return r;
                }
                self.own(slot, t, false);
                Res::Val(slot)
            }
        }
    }

    pub(crate) fn none_lit(&mut self, e: &'a Expr, dst: Value, t: Ty) -> Res {
        let Some(info) = self.enum_info(t) else {
            return self.unsupported(e.span, "`none` here");
        };
        let tag = self.konst(info.tag, Const::Int(0), e.span);
        self.store_at(dst, 0, tag);
        Res::Unit
    }

    /// A case without payload used as a value: `empty`, `.empty`, `shape.empty`.
    pub(crate) fn variant_value(&mut self, e: &'a Expr, dst: Value, t: Ty) -> Res {
        let variant = match &e.kind {
            ExprKind::Name(id) => self.res.use_at(self.f.file, id.span.start),
            ExprKind::Field { name, .. } => self.res.use_at(self.f.file, name.span.start),
            ExprKind::DotName(name) => {
                let TyKind::Adt(d, _) = self.kind(t) else {
                    return self.unsupported(e.span, "a case of this type");
                };
                self.res.member(d, self.src_text(name.span))
            }
            _ => None,
        };
        let Some(v) = variant.filter(|v| self.res.def(*v).kind == DefKind::Variant) else {
            return self.unsupported(e.span, "this name as a value");
        };
        let Some(owner) = self.res.def(v).parent else {
            return Res::Never;
        };
        let (Some(info), Some(idx)) = (self.enum_info(t), self.variant_index(owner, v)) else {
            return self.unsupported(e.span, "an enum case");
        };
        let tag = self.konst(info.tag, Const::Int(idx as i128), e.span);
        self.store_at(dst, 0, tag);
        Res::Unit
    }

    pub(crate) fn enum_ctor(
        &mut self,
        e: &'a Expr,
        t: Ty,
        owner: DefId,
        variant: DefId,
        args: &'a [Arg],
        dst: Value,
    ) -> Res {
        let (Some(info), Some(idx)) = (self.enum_info(t), self.variant_index(owner, variant))
        else {
            return self.unsupported(e.span, "an enum case");
        };
        let labels: Vec<Option<String>> = self
            .typed
            .adts
            .get(&owner)
            .and_then(|a| a.variants.get(idx))
            .map(|(_, fs)| fs.iter().map(|(l, _)| l.clone()).collect())
            .unwrap_or_default();
        let fields = info.variants[idx].clone();
        if self.fill_payload(e, args, &labels, &fields, dst).is_none() {
            return Res::Never;
        }
        let tag = self.konst(info.tag, Const::Int(idx as i128), e.span);
        self.store_at(dst, 0, tag);
        Res::Unit
    }

    /// Evaluates the arguments of a case into its payload fields.
    #[allow(clippy::explicit_counter_loop)]
    fn fill_payload(
        &mut self,
        e: &'a Expr,
        args: &'a [Arg],
        labels: &[Option<String>],
        fields: &[Field],
        dst: Value,
    ) -> Option<()> {
        if args.len() != fields.len() {
            self.unsupported(e.span, "a case with a different number of values");
            return None;
        }
        let mut next = 0usize;
        for a in args {
            let idx = match &a.label {
                Some(l) => {
                    let name = self.src_text(l.span);
                    labels
                        .iter()
                        .position(|x| x.as_deref() == Some(name))
                        .unwrap_or(next)
                }
                None => next,
            };
            next += 1;
            let f = fields.get(idx)?;
            let p = self.at(dst, f.off);
            if let Res::Never = self.into(&a.value, p, f.ty) {
                return None;
            }
        }
        Some(())
    }

    pub(crate) fn enum_ctor_by_name(
        &mut self,
        e: &'a Expr,
        t: Ty,
        name: &str,
        args: &'a [Arg],
        dst: Option<Value>,
    ) -> Res {
        // The case belongs to the enum the context expects (possibly inside an `option`).
        let et = match self.kind(t) {
            TyKind::Option(inner) => inner,
            _ => t,
        };
        let TyKind::Adt(d, _) = self.kind(et) else {
            return self.unsupported(e.span, "a case of this type");
        };
        let Some(v) = self.res.member(d, name) else {
            return self.unsupported(e.span, "an unknown case");
        };
        self.with_dst(dst, et, |s, dst| s.enum_ctor(e, et, d, v, args, dst))
    }

    pub(crate) fn result_ctor(
        &mut self,
        e: &'a Expr,
        is_err: bool,
        args: &'a [Arg],
        dst: Option<Value>,
    ) -> Res {
        let Some(t) = self.ty_of(e) else {
            return self.unsupported(e.span, "`ok` / `err` of unknown type");
        };
        let Some(info) = self.enum_info(t) else {
            return self.unsupported(e.span, "`ok` / `err` here");
        };
        let idx = usize::from(is_err);
        let fields = info.variants[idx].clone();
        let tag_ty = info.tag;
        self.with_dst(dst, t, |s, dst| {
            if let (Some(f), Some(a)) = (fields.first(), args.first()) {
                let p = s.at(dst, f.off);
                if let Res::Never = s.into(&a.value, p, f.ty) {
                    return Res::Never;
                }
            } else if let Some(a) = args.first() {
                s.expr(&a.value);
            }
            let tag = s.konst(tag_ty, Const::Int(idx as i128), e.span);
            s.store_at(dst, 0, tag);
            Res::Unit
        })
    }

    // ---- Names that are not variables ------------------------------------------------------------------------------------

    pub(crate) fn global_value(&mut self, e: &'a Expr, def: DefId) -> Res {
        match self.res.def(def).kind {
            DefKind::Variant => self.rvalue_expr(e),
            DefKind::Var if self.globals.contains_key(&def) => {
                let Some(p) = self.global_place(def) else {
                    return Res::Never;
                };
                let Some(t) = self.ty_of(e) else {
                    return self.unsupported(e.span, "a global of unknown type");
                };
                match self.repr(t) {
                    Repr::Scalar(s) => match self.emit(Op::Load(s, p), Some(s), e.span) {
                        Some(v) => Res::Val(v),
                        None => Res::Never,
                    },
                    Repr::Mem(_) => Res::Val(p),
                    Repr::Zst => Res::Unit,
                }
            }
            DefKind::Const | DefKind::Var | DefKind::Local => {
                let Some(g) = self.globals.get(&def) else {
                    return self.unsupported(e.span, "a name that is not a variable");
                };
                let (file, init) = (g.file, g.init);
                // The initializer is evaluated where the constant is used.
                let saved = self.f.file;
                self.f.file = file;
                let r = self.expr(init);
                self.f.file = saved;
                r
            }
            DefKind::Fn => self.fn_as_value(e, def),
            _ => self.unsupported(e.span, "a name that is not a variable"),
        }
    }

    /// The address of a module-level `var`, initialized the first time it is reached.
    pub(crate) fn global_place(&mut self, def: DefId) -> Option<Value> {
        let g = self.globals.get(&def)?;
        let (file, init) = (g.file, g.init);
        let saved = self.f.file;
        self.f.file = file;
        let t = self.ty_of(init);
        let result = (|| {
            let t = t?;
            let size = self.layout(t).size as usize;
            let idx = match self.global_data.get(&def) {
                Some(i) => *i,
                None => {
                    let i = self.module.add_zeroed_data(size + 1);
                    self.global_data.insert(def, i);
                    i
                }
            };
            let sp = init.span;
            let p = self.emit(Op::DataAddr(idx), Some(IrTy::Ptr), sp)?;
            let flag = self.addr(p, size as u32);
            let set = self.load_at(flag, 0, IrTy::Bool);
            let go = self.new_block(&[]);
            let join = self.new_block(&[]);
            self.branch(set, join, go);
            self.start(go);
            let one = self.konst(IrTy::Bool, Const::Bool(true), sp);
            self.emit(Op::Store(flag, one), None, sp);
            self.push_scope();
            let r = self.into(init, p, t);
            if matches!(r, Res::Never) {
                self.f.scopes.pop();
            } else {
                self.pop_scope();
                self.jump(join, vec![]);
            }
            self.start(join);
            Some(p)
        })();
        self.f.file = saved;
        result
    }

    pub(crate) fn global_or_variant(&mut self, e: &'a Expr) -> Res {
        let ExprKind::Field { name, .. } = &e.kind else {
            return self.unsupported(e.span, "a path");
        };
        match self.res.use_at(self.f.file, name.span.start) {
            Some(def) => self.global_value(e, def),
            None => self.unsupported(e.span, "a path"),
        }
    }
}
