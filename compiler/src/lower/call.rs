//! Calls: finding the function, passing arguments by the rules of their modes, and getting the
//! result back.
//!
//! - A read-only (`let`) argument of a memory type is passed by address; if it is not a place it is
//!   built in a temporary first. A scalar is passed by value.
//! - An `inout` argument is the address of the place.
//! - A `sink` argument is built in a fresh temporary that the callee then owns.
//! - A memory result is written to the address the caller provides (the hidden first parameter).

use super::expr::strip_parens;
use super::*;

impl<'a> Lowerer<'a> {
    /// The function a call runs and its type arguments, with this function's own type arguments
    /// filled in.
    pub(crate) fn call_target(&mut self, span: Span) -> Option<(DefId, HashMap<DefId, Ty>)> {
        let t = self.typed.calls.get(&(self.f.file, span.start, span.end))?;
        let def = t.def;
        let pairs = t.subst.clone();
        let mut map = HashMap::new();
        for (k, v) in pairs {
            let v = self.conc(v);
            map.insert(k, v);
        }
        Some((def, map))
    }

    pub(crate) fn call(
        &mut self,
        e: &'a Expr,
        callee: &'a Expr,
        args: &'a [Arg],
        mark: CallMark,
        dst: Option<Value>,
    ) -> Res {
        if self.box_get_base(e).is_some() {
            let Some(t) = self.ty_of(e) else {
                return self.unsupported(e.span, "a value of unknown type");
            };
            return match self.place(e) {
                Some(p) => self.read_place(p, t, e.span),
                None => Res::Never,
            };
        }
        let callee = strip_parens(callee);
        match &callee.kind {
            ExprKind::Name(id) => {
                let Some(def) = self.res.use_at(self.f.file, id.span.start) else {
                    return self.unsupported(e.span, "an unresolved name");
                };
                self.call_def(e, def, None, args, mark, dst)
            }
            ExprKind::TypeApply { base, .. } => {
                let def = match &strip_parens(base).kind {
                    ExprKind::Name(id) => self.res.use_at(self.f.file, id.span.start),
                    ExprKind::Field { name, .. } => self.res.use_at(self.f.file, name.span.start),
                    _ => None,
                };
                let Some(def) = def else {
                    return self.unsupported(e.span, "this kind of call");
                };
                self.call_def(e, def, None, args, mark, dst)
            }
            ExprKind::DotName(name) => self.dot_call(e, name, args, dst),
            ExprKind::Field {
                base,
                name,
                optional,
            } => {
                if let Some(def) = self.res.use_at(self.f.file, name.span.start) {
                    // A path: `shape.circle(1.0)`, `util.helper(2)`.
                    return self.call_def(e, def, None, args, mark, dst);
                }
                if *optional {
                    return self.opt_call(e, base, args, dst);
                }
                self.method_call(e, base, name, args, mark, dst)
            }
            _ => self.call_value(e, callee, args, dst),
        }
    }

    fn call_def(
        &mut self,
        e: &'a Expr,
        def: DefId,
        recv: Option<&'a Expr>,
        args: &'a [Arg],
        mark: CallMark,
        dst: Option<Value>,
    ) -> Res {
        match self.res.def(def).kind {
            DefKind::Fn => {
                let Some((target, map)) = self.call_target(e.span) else {
                    return self.unsupported(e.span, "a call the checker did not record");
                };
                self.emit_call(e, target, map, recv, args, mark, dst)
            }
            DefKind::Variant => self.variant_call(e, def, args, dst),
            DefKind::BuiltinType if self.res.def(def).name == "box" => {
                let Some((target, map)) = self.call_target(e.span) else {
                    return self.unsupported(e.span, "a call the checker did not record");
                };
                self.emit_call(e, target, map, None, args, mark, dst)
            }
            DefKind::Struct | DefKind::Obj => self.construct(e, def, args, dst),
            DefKind::BuiltinFn | DefKind::BuiltinCase => {
                let name = self.res.def(def).name.clone();
                self.builtin_call(e, &name, args, dst)
            }
            DefKind::Local | DefKind::Param | DefKind::Const | DefKind::Var => match &e.kind {
                ExprKind::Call { callee, .. } => self.call_value(e, callee, args, dst),
                _ => self.unsupported(e.span, "this kind of call"),
            },
            _ => self.unsupported(e.span, "this kind of call"),
        }
    }

    fn method_call(
        &mut self,
        e: &'a Expr,
        base: &'a Expr,
        name: &Ident,
        args: &'a [Arg],
        mark: CallMark,
        dst: Option<Value>,
    ) -> Res {
        let Some((target, map)) = self.call_target(e.span) else {
            // A field that holds a function: `self.callback(x)`.
            let _ = name;
            return self.unsupported(e.span, "calling a field that holds a function");
        };
        self.emit_call(e, target, map, Some(base), args, mark, dst)
    }

    /// `.circle(2.0)`: the case comes from the type the context expects.
    fn dot_call(&mut self, e: &'a Expr, name: &Ident, args: &'a [Arg], dst: Option<Value>) -> Res {
        let _ = name;
        let Some(t) = self.ty_of(e) else {
            return self.unsupported(e.span, "a case of unknown type");
        };
        self.enum_ctor_by_name(e, t, self.src_text(name.span), args, dst)
    }

    // ---- The call itself ------------------------------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn emit_call(
        &mut self,
        e: &'a Expr,
        def: DefId,
        map: HashMap<DefId, Ty>,
        recv: Option<&'a Expr>,
        args: &'a [Arg],
        mark: CallMark,
        dst: Option<Value>,
    ) -> Res {
        let _ = mark;
        let (def, map) = self.resolve_dispatch(def, map);
        if self.is_intrinsic(def) {
            return self.intrinsic_call(e, def, &map, recv, args, dst);
        }
        let Some(info) = self.typed.fn_infos.get(&def).cloned() else {
            return self.unsupported(e.span, "a function without a signature");
        };
        // The pieces to pass: the receiver, then each declared parameter.
        let mut sources: Vec<ArgSource<'a>> = Vec::new();
        if info.has_self {
            match recv {
                Some(r) => {
                    let through = self.through_boxes(r, def, &info, &map);
                    match through {
                        Some(Some(p)) => sources.push(ArgSource::Addr(p)),
                        Some(None) => return Res::Never,
                        None => sources.push(ArgSource::Expr(r)),
                    }
                }
                None => return self.unsupported(e.span, "a method called without a receiver"),
            }
        }
        match self.match_args(&info, def, args, e.span) {
            Some(mut v) => sources.append(&mut v),
            None => return Res::Never,
        }
        self.call_with(e, def, &map, sources, dst)
    }

    /// A receiver that is a `box` whose method belongs to the value inside: the address of that
    /// value. `None` when the receiver is passed as it is; `Some(None)` when lowering failed.
    fn through_boxes(
        &mut self,
        r: &'a Expr,
        def: DefId,
        info: &crate::typecheck::FnInfo,
        map: &HashMap<DefId, Ty>,
    ) -> Option<Option<Value>> {
        let rt = self.ty_of(r)?;
        if !matches!(self.kind(rt), TyKind::Box(_)) {
            return None;
        }
        let info = self.fn_info_for(def, info);
        let want = self.sig_of(&info, map).params.first().map(|p| p.1)?;
        if matches!(self.kind(want), TyKind::Box(_)) {
            return None;
        }
        let mut p = self.scalar(r)?;
        let mut t = self.peel_box(rt);
        while let TyKind::Box(inner) = self.kind(t) {
            p = self.load_at(p, 0, IrTy::Ptr);
            t = inner;
        }
        Some(Some(p))
    }

    /// Calls `def` with the pieces already matched to its parameters (the receiver first).
    pub(crate) fn call_with(
        &mut self,
        e: &'a Expr,
        def: DefId,
        map: &HashMap<DefId, Ty>,
        sources: Vec<ArgSource<'a>>,
        dst: Option<Value>,
    ) -> Res {
        let Some(info) = self.typed.fn_infos.get(&def).cloned() else {
            return self.unsupported(e.span, "a function without a signature");
        };
        let info = self.fn_info_for(def, &info);
        let sig = self.sig_of(&info, map);
        let any_slot = if info.has_self
            && matches!(sig.params.first(), Some((_, t, _)) if matches!(self.kind(*t), TyKind::Any(_)))
        {
            self.any_slot(def)
        } else {
            None
        };
        let slot = if info.has_self && any_slot.is_none() {
            self.virtual_slot(def)
        } else {
            None
        };
        let slot = slot.or(any_slot);
        let fid = if slot.is_some() {
            None
        } else {
            match self.request_fn(def, map) {
                Some(f) => Some(f),
                None => return self.unsupported(e.span, "a function that cannot be lowered"),
            }
        };
        let mut ir_args: Vec<Value> = Vec::new();
        // Where a memory result goes.
        let mut ret_slot = None;
        if sig.ret_pass == RetPass::Sret {
            let d = match dst {
                Some(d) => d,
                None => self.slot_for(sig.ret),
            };
            ret_slot = Some(d);
            ir_args.push(d);
        }
        for ((pass, ty, mode), src) in sig.params.iter().zip(sources) {
            let v = self.lower_arg(*pass, *ty, *mode, src, e.span);
            match (pass, v) {
                (Pass::Skip, _) => {}
                (_, Some(v)) => ir_args.push(v),
                (_, None) => return Res::Never,
            }
        }
        if self.f.cur.is_none() {
            return Res::Never;
        }
        let ret_ir = match sig.ret_pass {
            RetPass::Val(t) => Some(t),
            _ => None,
        };
        let result = match (fid, slot) {
            (Some(fid), _) => self.emit(Op::Call(fid, ir_args), ret_ir, e.span),
            (None, Some(slot)) => {
                // The method is looked up in a table at run time.
                let at = usize::from(sig.ret_pass == RetPass::Sret);
                let recv = ir_args[at];
                let table = if any_slot.is_some() {
                    // An `any`: the table is the second word, and `self` is the heap copy.
                    let data = self.load_at(recv, 0, IrTy::Ptr);
                    ir_args[at] = data;
                    self.load_at(recv, 8, IrTy::Ptr)
                } else {
                    self.load_at(recv, 0, IrTy::Ptr)
                };
                let f = self.load_at(table, (slot * 8) as u32, IrTy::Ptr);
                let tys: Vec<IrTy> = ir_args.iter().map(|v| self.vty(*v)).collect();
                self.emit(
                    Op::CallPtr(tys, ret_ir.unwrap_or(IrTy::Void), f, ir_args),
                    ret_ir,
                    e.span,
                )
            }
            _ => None,
        };
        let never = matches!(self.kind(sig.ret), TyKind::Never);
        if never {
            self.terminate(Term::Unreachable);
            return Res::Never;
        }
        match sig.ret_pass {
            RetPass::Val(_) => match result {
                Some(v) => Res::Val(v),
                None => Res::Never,
            },
            RetPass::Void => Res::Unit,
            RetPass::Sret => {
                if dst.is_none() {
                    // A temporary result, owned by the current scope.
                    if let Some(slot) = ret_slot {
                        self.own(slot, sig.ret, false);
                        return Res::Val(slot);
                    }
                }
                Res::Unit
            }
        }
    }

    /// One argument, passed the way its parameter wants it.
    pub(crate) fn lower_arg(
        &mut self,
        pass: Pass,
        ty: Ty,
        mode: Option<Mode>,
        src: ArgSource<'a>,
        span: Span,
    ) -> Option<Value> {
        let arg = match src {
            ArgSource::Expr(arg) => {
                if mode != Some(Mode::Sink) {
                    if let ExprKind::Closure(_) = &strip_parens(arg).kind {
                        let sp = strip_parens(arg).span;
                        self.f.stack_env = Some((sp.start, sp.end));
                    }
                }
                arg
            }
            ArgSource::Default(file, arg) => {
                let saved = self.f.file;
                self.f.file = file;
                let r = self.lower_arg(pass, ty, mode, ArgSource::Expr(arg), span);
                self.f.file = saved;
                return r;
            }
            ArgSource::Val(v) => return Some(v),
            ArgSource::Addr(p) => {
                return match pass {
                    Pass::Skip => None,
                    Pass::Val(t) => self.emit(Op::Load(t, p), Some(t), span),
                    Pass::Ptr => Some(p),
                };
            }
            ArgSource::Pack(items) => {
                let tmp = self.slot_for(ty);
                let exprs: Vec<&'a Expr> = items.iter().map(|a| &a.value).collect();
                if let Res::Never = self.list_from(span, &exprs, tmp, ty) {
                    return None;
                }
                self.own(tmp, ty, false);
                return Some(tmp);
            }
        };
        match pass {
            Pass::Skip => {
                self.expr(arg);
                None
            }
            Pass::Val(_) => {
                let have = self.ty_of(arg);
                if mode == Some(Mode::Sink) {
                    self.consume_scalar(arg)
                } else if have.is_some() && have != Some(ty) && !self.same_scalar(have, ty) {
                    // A conversion such as a value into an `option` is not a scalar thing.
                    self.scalar(arg)
                } else {
                    self.scalar(arg)
                }
            }
            Pass::Ptr => match mode {
                Some(Mode::Inout) => match self.place(arg) {
                    Some(p) => Some(p),
                    None => {
                        if !self.failed() {
                            self.unsupported(arg.span, "an `inout` argument that is not a place");
                        }
                        None
                    }
                },
                Some(Mode::Sink) => {
                    let tmp = self.slot_for(ty);
                    match self.into(arg, tmp, ty) {
                        Res::Never => None,
                        _ => Some(tmp),
                    }
                }
                _ => {
                    let have = self.ty_of(arg);
                    if have == Some(ty) {
                        match self.expr(arg) {
                            Res::Val(p) => Some(p),
                            _ => None,
                        }
                    } else {
                        // The argument needs converting (a value into an `option`, text into a view).
                        let tmp = self.slot_for(ty);
                        match self.into(arg, tmp, ty) {
                            Res::Never => None,
                            _ => {
                                self.own(tmp, ty, false);
                                Some(tmp)
                            }
                        }
                    }
                }
            },
        }
    }

    fn same_scalar(&mut self, a: Option<Ty>, b: Ty) -> bool {
        match a {
            Some(a) => self.scalar_of(a) == self.scalar_of(b),
            None => false,
        }
    }

    /// Matches the arguments of a call to the declared parameters: by label, then by position, with
    /// defaults filling the gaps.
    pub(crate) fn match_args(
        &mut self,
        info: &FnInfo,
        def: DefId,
        args: &'a [Arg],
        span: Span,
    ) -> Option<Vec<ArgSource<'a>>> {
        let n = info.params.len();
        let mut slots: Vec<Option<ArgSource<'a>>> = vec![None; n];
        let mut packs: HashMap<usize, (usize, usize)> = HashMap::new();
        let mut next = 0usize;
        for (pos, a) in args.iter().enumerate() {
            let idx = match &a.label {
                Some(l) => {
                    let name = self.src_text(l.span);
                    info.params.iter().position(|p| p.name == name)?
                }
                None => {
                    while next < n && slots[next].is_some() {
                        next += 1;
                    }
                    next
                }
            };
            if idx >= n {
                self.unsupported(a.value.span, "an extra argument");
                return None;
            }
            if info.params[idx].variadic {
                if a.spread {
                    slots[idx] = Some(ArgSource::Expr(&a.value));
                } else {
                    let e = packs.entry(idx).or_insert((pos, 0));
                    e.1 += 1;
                }
                continue;
            }
            if a.spread {
                self.unsupported(a.value.span, "a spread argument");
                return None;
            }
            slots[idx] = Some(ArgSource::Expr(&a.value));
        }
        for (idx, (start, count)) in packs {
            slots[idx] = Some(ArgSource::Pack(&args[start..start + count]));
        }
        let mut out = Vec::new();
        for (i, s) in slots.into_iter().enumerate() {
            match s {
                Some(s) => out.push(s),
                None => {
                    if info.params[i].variadic {
                        out.push(ArgSource::Pack(&[]));
                    } else if info.params[i].has_default {
                        match self.default_arg(def, i) {
                            Some(e) => {
                                let file =
                                    self.fns.get(&def).map(|x| x.file).unwrap_or(self.f.file);
                                out.push(ArgSource::Default(file, e));
                            }
                            None => {
                                self.unsupported(span, "a default argument");
                                return None;
                            }
                        }
                    } else {
                        self.unsupported(span, "a missing argument");
                        return None;
                    }
                }
            }
        }
        Some(out)
    }

    /// The default value expression of the i-th declared parameter of a function.
    fn default_arg(&mut self, def: DefId, i: usize) -> Option<&'a Expr> {
        let entry = self.fns.get(&def)?;
        let decl = entry.decl;
        decl.params
            .iter()
            .filter(|p| !p.is_self)
            .nth(i)?
            .default
            .as_ref()
    }

    // ---- Which function runs --------------------------------------------------------------------------------------------

    /// A call through an interface method is resolved to the type's own method (or the interface's
    /// default).
    fn resolve_dispatch(
        &mut self,
        def: DefId,
        map: HashMap<DefId, Ty>,
    ) -> (DefId, HashMap<DefId, Ty>) {
        let Some(parent) = self.res.def(def).parent else {
            return (def, map);
        };
        if self.res.def(parent).kind != DefKind::Interface {
            return (def, map);
        }
        let Some(&self_ty) = map.get(&parent) else {
            return (def, map);
        };
        let name = self.res.def(def).name.clone();
        match self.find_impl_method(self_ty, &name) {
            Some((m, mut found)) => {
                // Keep the method's own type arguments from the call.
                for (k, v) in &map {
                    found.entry(*k).or_insert(*v);
                }
                (m, found)
            }
            None => (def, map),
        }
    }

    /// The method called `name` that a concrete type has: its own, one from an `impl` block.
    pub(crate) fn find_impl_method(
        &mut self,
        self_ty: Ty,
        name: &str,
    ) -> Option<(DefId, HashMap<DefId, Ty>)> {
        let self_ty = self.types.zonk(self_ty);
        if let TyKind::Adt(d, args) = self.kind(self_ty) {
            if let Some(m) = self.res.member(d, name) {
                if self.res.def(m).kind == DefKind::Fn && self.fns.contains_key(&m) {
                    let map = self.adt_subst(d, &args);
                    return Some((m, map));
                }
            }
        }
        let impls = self.typed.impls.clone();
        for im in &impls {
            let Some(&m) = im.methods.get(name) else {
                continue;
            };
            let mut map = HashMap::new();
            if self.match_ty(im.self_ty, self_ty, &mut map) {
                return Some((m, map));
            }
        }
        None
    }

    /// Does `concrete` fit the type `pat`, whose type parameters stand for anything? Fills `map`.
    pub(crate) fn match_ty(&mut self, pat: Ty, concrete: Ty, map: &mut HashMap<DefId, Ty>) -> bool {
        let pat = self.types.shallow(pat);
        let concrete = self.types.shallow(concrete);
        if pat == concrete {
            return true;
        }
        let (pk, ck) = (self.kind(pat), self.kind(concrete));
        match (pk, ck) {
            (TyKind::Param(g), _) => match map.get(&g) {
                Some(prev) => *prev == concrete,
                None => {
                    map.insert(g, concrete);
                    true
                }
            },
            (TyKind::Adt(a, xs), TyKind::Adt(b, ys)) => a == b && self.match_all(&xs, &ys, map),
            (TyKind::List(a), TyKind::List(b))
            | (TyKind::Option(a), TyKind::Option(b))
            | (TyKind::Set(a), TyKind::Set(b))
            | (TyKind::Box(a), TyKind::Box(b))
            | (TyKind::Range(a), TyKind::Range(b))
            | (TyKind::Iter(a), TyKind::Iter(b)) => self.match_ty(a, b, map),
            (TyKind::Array(a, _), TyKind::Array(b, _)) => self.match_ty(a, b, map),
            (TyKind::Result(a, b), TyKind::Result(c, d))
            | (TyKind::Map(a, b), TyKind::Map(c, d)) => {
                self.match_ty(a, c, map) && self.match_ty(b, d, map)
            }
            (TyKind::Tuple(xs), TyKind::Tuple(ys)) => self.match_all(&xs, &ys, map),
            (TyKind::Fn(ps, r), TyKind::Fn(qs, s)) => {
                ps.len() == qs.len()
                    && ps
                        .iter()
                        .zip(qs.iter())
                        .all(|(p, q)| self.match_ty(p.ty, q.ty, map))
                    && self.match_ty(r, s, map)
            }
            _ => false,
        }
    }

    fn match_all(&mut self, xs: &[Ty], ys: &[Ty], map: &mut HashMap<DefId, Ty>) -> bool {
        xs.len() == ys.len()
            && xs
                .iter()
                .zip(ys.iter())
                .all(|(x, y)| self.match_ty(*x, *y, map))
    }

    // ---- Things that are not plain function calls ----------------------------------------------------------------------------

    fn variant_call(
        &mut self,
        e: &'a Expr,
        variant: DefId,
        args: &'a [Arg],
        dst: Option<Value>,
    ) -> Res {
        let Some(t) = self.ty_of(e) else {
            return self.unsupported(e.span, "a case of unknown type");
        };
        let Some(owner) = self.res.def(variant).parent else {
            return Res::Never;
        };
        let dst = match dst {
            Some(d) => d,
            None => {
                let slot = self.slot_for(t);
                let r = self.enum_ctor(e, t, owner, variant, args, slot);
                if !matches!(r, Res::Never) {
                    self.own(slot, t, false);
                    return Res::Val(slot);
                }
                return r;
            }
        };
        self.enum_ctor(e, t, owner, variant, args, dst)
    }

    fn builtin_call(
        &mut self,
        e: &'a Expr,
        name: &str,
        args: &'a [Arg],
        dst: Option<Value>,
    ) -> Res {
        match name {
            "ok" | "err" => self.result_ctor(e, name == "err", args, dst),
            _ => self.intrinsic_by_name(e, name, args, dst),
        }
    }

    fn construct(&mut self, e: &'a Expr, def: DefId, args: &'a [Arg], dst: Option<Value>) -> Res {
        let Some((init, map)) = self.call_target(e.span) else {
            return self.unsupported(e.span, "constructing without an `init`");
        };
        if self.res.def(def).kind == DefKind::Obj {
            let Some(t) = self.ty_of(e) else {
                return self.unsupported(e.span, "an object of unknown type");
            };
            if self.obj_def(t).is_none() {
                return self.unsupported(e.span, "an `init` that can fail on an object");
            }
            return self.obj_construct(e, init, map, t, args);
        }
        self.emit_call(e, init, map, None, args, CallMark::None, dst)
    }
}

/// Where the value for a parameter comes from.
#[derive(Clone, Copy)]
pub(crate) enum ArgSource<'a> {
    Expr(&'a Expr),
    /// The arguments of a variadic parameter, collected into a list.
    Pack(&'a [Arg]),
    /// An existing value of the parameter's type, given by address.
    Addr(Value),
    /// A ready scalar value.
    Val(Value),
    /// A default written in another file, where its names and types are known.
    Default(ModuleId, &'a Expr),
}
