//! Calls, methods, constructors, fields and struct literals.

use super::*;

/// A method found for a receiver.
pub(crate) struct Method {
    pub sig: FnSig,
    /// Owner / impl generics → their values for this receiver.
    pub map: HashMap<DefId, Ty>,
}

impl<'p> Checker<'p> {
    // ---- Method lookup ----------------------------------------------------------------------------

    fn adt_map(&mut self, def: DefId, args: &[Ty]) -> HashMap<DefId, Ty> {
        let gens = self.adt_generics(def);
        gens.iter().copied().zip(args.iter().copied()).collect()
    }

    fn find_adt_method(&mut self, def: DefId, args: &[Ty], name: &str) -> Option<Method> {
        if let Some(m) = self.res.member(def, name) {
            if self.def(m).kind == DefKind::Fn {
                let sig = self.fn_sig(m)?;
                let map = self.adt_map(def, args);
                return Some(Method { sig, map });
            }
        }
        // An `obj` inherits the methods of its base.
        let map = self.adt_map(def, args);
        for b in self.bases_of(def) {
            let b = self.tt.subst(b, &map);
            if let TyKind::Adt(bd, ba) = self.kind_of(b) {
                if let Some(m) = self.find_adt_method(bd, &ba, name) {
                    return Some(m);
                }
            }
        }
        None
    }

    /// A number literal whose type is still open takes its default before a method is looked up.
    pub(crate) fn settle_literal(&mut self, t: Ty) {
        match self.tt.var_info(t) {
            Some((VarKind::Int, _)) => {
                let i = self.tt.int;
                self.tt.unify(t, i);
            }
            Some((VarKind::Float, _)) => {
                let f = self.tt.float;
                self.tt.unify(t, f);
            }
            _ => {}
        }
    }

    pub(crate) fn lookup_method(&mut self, recv: Ty, name: &str, span: Span) -> Option<Method> {
        if let Some(m) = self.find_own_method(recv, name, span) {
            return Some(m);
        }
        // Promised and default methods of the interfaces the type provides.
        let r = self.tt.shallow(recv);
        self.iface_method(r, name)
    }

    /// A method the type itself has: in its declaration, its `obj` bases or an `impl` block.
    pub(crate) fn find_own_method(&mut self, recv: Ty, name: &str, span: Span) -> Option<Method> {
        self.settle_literal(recv);
        let r = self.tt.shallow(recv);
        if let TyKind::Adt(def, args) = self.kind_of(r) {
            if let Some(m) = self.find_adt_method(def, &args, name) {
                return Some(m);
            }
        }
        if let Some(h) = self.head_of(r) {
            let candidates = self.impl_by_head.get(&h).cloned().unwrap_or_default();
            for idx in candidates {
                let Some(&mdef) = self.impls[idx].methods.get(name) else {
                    continue;
                };
                let gens = self.impls[idx].generics.clone();
                let map = self.fresh_map(&gens, span);
                let inst = self.tt.subst(self.impls[idx].self_ty, &map);
                let mark = self.tt.snapshot();
                if self.tt.unify(inst, r) {
                    let sig = self.fn_sig(mdef)?;
                    return Some(Method { sig, map });
                }
                self.tt.rollback(mark);
            }
        }
        match self.kind_of(r) {
            TyKind::String => {
                let s = self.tt.str_;
                self.lookup_method(s, name, span)
            }
            TyKind::Array(e, _) => {
                let l = self.tt.list(e);
                self.lookup_method(l, name, span)
            }
            TyKind::Box(inner) => self.lookup_method(inner, name, span),
            _ => None,
        }
    }

    // ---- Fields -------------------------------------------------------------------------------------

    /// A field of a struct or obj (including its bases): its type, with type arguments filled in.
    fn find_field(&mut self, def: DefId, args: &[Ty], name: &str, span: Span) -> Option<Ty> {
        if let Some(f) = self.res.member(def, name) {
            if self.def(f).kind == DefKind::Field {
                let d = self.def(f);
                if let Some(owner_scope) = d.module {
                    if !self.res.visible(d.vis, owner_scope, self.scope) {
                        let tn = self.def(def).name.clone();
                        self.err(span, format!("the field `{name}` of `{tn}` is private"));
                    }
                }
                let ft = self.field_ty(f);
                let map = self.adt_map(def, args);
                return Some(self.tt.subst(ft, &map));
            }
        }
        let map = self.adt_map(def, args);
        for b in self.bases_of(def) {
            let b = self.tt.subst(b, &map);
            if let TyKind::Adt(bd, ba) = self.kind_of(b) {
                if let Some(t) = self.find_field(bd, &ba, name, span) {
                    return Some(t);
                }
            }
        }
        None
    }

    /// Every field of a struct or obj, bases first, with its type for these type arguments.
    pub(crate) fn collect_fields(&mut self, def: DefId, args: &[Ty]) -> Vec<(DefId, Ty)> {
        let map = self.adt_map(def, args);
        let mut out = Vec::new();
        for b in self.bases_of(def) {
            let b = self.tt.subst(b, &map);
            if let TyKind::Adt(bd, ba) = self.kind_of(b) {
                if matches!(self.def(bd).kind, DefKind::Obj | DefKind::Struct) {
                    out.extend(self.collect_fields(bd, &ba));
                }
            }
        }
        let members: Vec<DefId> = self.def(def).members.clone();
        for m in members {
            if self.def(m).kind == DefKind::Field {
                let ft = self.field_ty(m);
                let ft = self.tt.subst(ft, &map);
                out.push((m, ft));
            }
        }
        out
    }

    /// Names of the methods a receiver has, for "did you mean".
    fn candidate_methods(&mut self, recv: Ty) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let r = self.tt.shallow(recv);
        let mut heads = vec![self.head_of(r)];
        if matches!(self.kind_of(r), TyKind::String) {
            heads.push(Some(Head::Str));
        }
        if let TyKind::Adt(def, _) = self.kind_of(r) {
            names.extend(
                self.def(def)
                    .members
                    .iter()
                    .map(|m| self.def(*m).name.clone()),
            );
        }
        for h in heads.into_iter().flatten() {
            for idx in self.impl_by_head.get(&h).cloned().unwrap_or_default() {
                names.extend(self.impls[idx].methods.keys().cloned());
            }
        }
        names
    }

    /// Is this expression something that can be changed or moved out of (a place)?
    fn is_place_expr(e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Name(_)
            | ExprKind::SelfValue
            | ExprKind::TupleIndex { .. }
            | ExprKind::Index { .. } => true,
            ExprKind::Field { .. } => true,
            ExprKind::Paren(inner) => Self::is_place_expr(inner),
            _ => false,
        }
    }

    /// The call site repeats `inout` and `sink` for places; temporaries need no `sink`.
    fn check_arg_mode(&mut self, want: Option<Mode>, a: &'p Arg, what: &str) {
        let place = Self::is_place_expr(&a.value);
        match (want, a.mode) {
            (Some(Mode::Inout), Some(Mode::Inout)) => {
                if !place {
                    self.err(a.value.span, format!("{what} is changed in place, so it must be a variable, not a temporary value"));
                }
            }
            (Some(Mode::Sink), Some(Mode::Sink)) => {}
            (Some(Mode::Inout), _) => self.err(
                a.value.span,
                format!("{what} is changed in place; write `inout ...` at the call"),
            ),
            (Some(Mode::Sink), None) => {
                if place {
                    self.err(
                        a.value.span,
                        format!("{what} takes the value away; write `sink ...` at the call"),
                    );
                }
            }
            (_, Some(m)) => self.err(
                a.value.span,
                format!("{what} only reads; remove `{}`", m.as_str()),
            ),
            _ => {}
        }
    }

    pub(crate) fn field_expr(
        &mut self,
        e: &'p Expr,
        base: &'p Expr,
        name: &'p Ident,
        optional: bool,
        exp: Exp,
    ) -> Ty {
        // A path through a module or a type: the resolver already knows the target.
        if let Some(def) = self.used(name) {
            self.record(base, self.tt.unknown);
            return self.value_of_def(def, e.span, exp);
        }
        let bt = self.check_expr(base, Exp::Any);
        let (recv, wrap) = if optional {
            match self.kind_of(bt) {
                TyKind::Option(inner) => (inner, true),
                TyKind::Unknown | TyKind::Var(_) => (self.tt.unknown, false),
                _ => {
                    let s = self.show(bt);
                    self.err(
                        base.span,
                        format!("`?.` needs an option on the left, but this is `{s}`"),
                    );
                    (self.tt.unknown, false)
                }
            }
        } else {
            (bt, false)
        };
        let n = self.text(name);
        let t = match self.kind_of(recv) {
            TyKind::Adt(def, args) => match self.find_field(def, &args, n, name.span) {
                Some(t) => t,
                None => {
                    if self.lookup_method(recv, n, name.span).is_some() {
                        self.err(
                            name.span,
                            format!("`{n}` is a method; call it with parentheses: `{n}(...)`"),
                        );
                    } else {
                        let tn = self.def(def).name.clone();
                        let cands: Vec<String> = self
                            .def(def)
                            .members
                            .iter()
                            .map(|m| self.def(*m).name.clone())
                            .collect();
                        let mut msg = format!("`{tn}` has no field or method `{n}`");
                        if let Some(s) = self.closest(n, cands.iter().map(|s| s.as_str())) {
                            msg.push_str(&format!("; did you mean `{s}`?"));
                        }
                        self.err(name.span, msg);
                    }
                    self.tt.unknown
                }
            },
            TyKind::Unknown | TyKind::Var(_) => self.tt.unknown,
            _ => {
                if self.lookup_method(recv, n, name.span).is_some() {
                    self.err(
                        name.span,
                        format!("`{n}` is a method; call it with parentheses: `{n}(...)`"),
                    );
                } else {
                    let s = self.show(recv);
                    self.err(name.span, format!("`{s}` has no field or method `{n}`"));
                }
                self.tt.unknown
            }
        };
        if wrap {
            // `a?.b` where `b` is already an option stays one option.
            if matches!(self.kind_of(t), TyKind::Option(_)) {
                t
            } else {
                self.tt.option(t)
            }
        } else {
            t
        }
    }

    // ---- Struct literals -------------------------------------------------------------------------------

    pub(crate) fn struct_lit(
        &mut self,
        e: &'p Expr,
        ty: &'p Type,
        fields: &'p [StructInit],
        exp: Exp,
    ) -> Ty {
        // Lower the written type, but let missing type arguments be inferred.
        let TypeKind::Path { path, args } = &ty.kind else {
            for f in fields {
                if let StructInit::Field(FieldInit { value: Some(v), .. }) | StructInit::Spread(v) =
                    f
                {
                    self.check_expr(v, Exp::Any);
                }
            }
            return self.tt.unknown;
        };
        let def = path.segments.last().and_then(|l| self.used(l));
        let Some(def) = def.filter(|d| matches!(self.def(*d).kind, DefKind::Struct | DefKind::Obj))
        else {
            for f in fields {
                match f {
                    StructInit::Field(FieldInit { value: Some(v), .. }) | StructInit::Spread(v) => {
                        self.check_expr(v, Exp::Any);
                    }
                    _ => {}
                }
            }
            return self.tt.unknown;
        };
        let gens = self.adt_generics(def);
        let map: HashMap<DefId, Ty> = if args.is_empty() {
            self.fresh_map(&gens, e.span)
        } else {
            let lowered = self.lower_generic_args(args);
            gens.iter().copied().zip(lowered).collect()
        };
        let targs: Vec<Ty> = gens
            .iter()
            .map(|g| map.get(g).copied().unwrap_or(self.tt.unknown))
            .collect();
        let st = self.tt.mk(TyKind::Adt(def, targs));
        if let Exp::Is(want) = exp {
            self.tt.unify(st, want);
        }
        let fields_all = {
            let a = match self.kind_of(st) {
                TyKind::Adt(_, a) => a,
                _ => Vec::new(),
            };
            self.collect_fields(def, &a)
        };
        let all: Vec<DefId> = fields_all.iter().map(|(d, _)| *d).collect();
        // The literal form needs every field to be visible here.
        for f in &all {
            let d = self.def(*f);
            if let Some(owner_scope) = d.module {
                if !self.res.visible(d.vis, owner_scope, self.scope) {
                    let tn = self.def(def).name.clone();
                    self.err(e.span, format!("`{tn}` has private fields, so it cannot be built with `{{ }}` here; use its `init`"));
                    break;
                }
            }
        }
        let mut given: Vec<DefId> = Vec::new();
        let mut spread = false;
        for f in fields {
            match f {
                StructInit::Field(fi) => {
                    let fname = self.text(&fi.name);
                    let found = fields_all
                        .iter()
                        .find(|(d, _)| self.def(*d).name == fname)
                        .copied();
                    match found {
                        Some((fd, ft)) => {
                            given.push(fd);
                            match &fi.value {
                                Some(v) => {
                                    self.check_coerce(v, ft);
                                }
                                None => {
                                    // Shorthand: the variable of the same name.
                                    let vt = match self.used(&fi.name) {
                                        Some(d) => self.value_of_def(d, fi.name.span, Exp::Is(ft)),
                                        None => self.tt.unknown,
                                    };
                                    self.coerce_at(vt, ft, fi.name.span, "this field");
                                }
                            }
                        }
                        None => {
                            if let Some(v) = &fi.value {
                                self.check_expr(v, Exp::Any);
                            }
                            let tn = self.def(def).name.clone();
                            let cands: Vec<String> =
                                all.iter().map(|m| self.def(*m).name.clone()).collect();
                            let mut msg = format!("`{tn}` has no field `{fname}`");
                            if let Some(s) = self.closest(fname, cands.iter().map(|s| s.as_str())) {
                                msg.push_str(&format!("; did you mean `{s}`?"));
                            }
                            self.err(fi.name.span, msg);
                        }
                    }
                }
                StructInit::Spread(v) => {
                    spread = true;
                    let vt = self.check_expr(v, Exp::Is(st));
                    self.coerce_at(vt, st, v.span, "the spread value");
                }
            }
        }
        if !spread {
            let missing: Vec<String> = all
                .iter()
                .filter(|f| !given.contains(f))
                .filter(|f| !self.field_has_default(**f))
                .map(|f| format!("`{}`", self.def(*f).name))
                .collect();
            if !missing.is_empty() {
                let tn = self.def(def).name.clone();
                self.err(e.span, format!("`{tn}` needs {}", missing.join(", ")));
            }
        }
        st
    }

    fn field_has_default(&self, field: DefId) -> bool {
        matches!(self.nodes.get(&field), Some(Node::Field { f, .. }) if f.default.is_some())
    }

    // ---- Calls ---------------------------------------------------------------------------------------------

    /// Matches arguments to parameters (by position and by name) and checks each one.
    fn match_args(
        &mut self,
        params: &[ParamSig],
        map: &HashMap<DefId, Ty>,
        args: &'p [Arg],
        call: Span,
        name: &str,
    ) {
        let ptys: Vec<Ty> = params.iter().map(|p| self.tt.subst(p.ty, map)).collect();
        let mut filled = vec![false; params.len()];
        let mut next = 0usize;
        for a in args {
            let idx = match &a.label {
                Some(l) => {
                    let n = self.text(l);
                    match params.iter().position(|p| p.name == n) {
                        Some(i) => Some(i),
                        None => {
                            let mut msg = format!("`{name}` has no parameter named `{n}`");
                            if let Some(s) = self.closest(n, params.iter().map(|p| p.name.as_str()))
                            {
                                msg.push_str(&format!("; did you mean `{s}`?"));
                            }
                            self.err(l.span, msg);
                            None
                        }
                    }
                }
                None => {
                    while next < params.len() && filled[next] && !params[next].variadic {
                        next += 1;
                    }
                    if next < params.len() {
                        Some(next)
                    } else {
                        None
                    }
                }
            };
            let Some(i) = idx else {
                if a.label.is_none() {
                    self.err(
                        a.value.span,
                        format!(
                            "`{name}` takes {} argument{}, but more were given",
                            params.len(),
                            if params.len() == 1 { "" } else { "s" }
                        ),
                    );
                }
                self.check_expr(&a.value, Exp::Any);
                continue;
            };
            let p = &params[i];
            let what = format!("the argument `{}`", p.name);
            self.check_arg_mode(p.mode, a, &what);
            if p.variadic {
                if a.spread {
                    let want = self.tt.list(ptys[i]);
                    let t = self.check_expr(&a.value, Exp::Is(want));
                    if !self.try_coerce(t, want) {
                        let arr = self.kind_of(t);
                        if !matches!(arr, TyKind::Array(..) | TyKind::Unknown) {
                            let s = self.show(t);
                            self.err(
                                a.value.span,
                                format!("`...` here needs a list, but this is `{s}`"),
                            );
                        }
                    }
                } else {
                    self.check_coerce(&a.value, ptys[i]);
                }
                filled[i] = true;
            } else {
                if a.spread {
                    self.err(
                        a.value.span,
                        format!("`{}` is not variadic, so `...` does not apply", p.name),
                    );
                }
                if filled[i] {
                    self.err(
                        a.value.span,
                        format!("the argument `{}` is given twice", p.name),
                    );
                }
                filled[i] = true;
                self.check_coerce(&a.value, ptys[i]);
            }
        }
        let missing: Vec<String> = params
            .iter()
            .enumerate()
            .filter(|(i, p)| !filled[*i] && !p.has_default && !p.variadic)
            .map(|(_, p)| format!("`{}`", p.name))
            .collect();
        if !missing.is_empty() {
            self.err(call, format!("`{name}` is missing {}", missing.join(", ")));
        }
    }

    /// Calls a function value (`f(1, 2)` where `f` has a `fn` type).
    fn call_value(&mut self, e: &'p Expr, ft: Ty, args: &'p [Arg]) -> Ty {
        match self.kind_of(ft) {
            TyKind::Fn(ps, ret) => {
                if ps.len() != args.len() {
                    self.err(
                        e.span,
                        format!(
                            "this function takes {} argument{}, but {} {} given",
                            ps.len(),
                            if ps.len() == 1 { "" } else { "s" },
                            args.len(),
                            if args.len() == 1 { "was" } else { "were" }
                        ),
                    );
                }
                for (i, a) in args.iter().enumerate() {
                    if a.label.is_some() {
                        self.err(
                            a.value.span,
                            "arguments of a function value cannot be named",
                        );
                    }
                    match ps.get(i) {
                        Some(p) => {
                            self.check_arg_mode(p.mode, a, "this argument");
                            self.check_coerce(&a.value, p.ty);
                        }
                        None => {
                            self.check_expr(&a.value, Exp::Any);
                        }
                    }
                }
                ret
            }
            TyKind::Unknown | TyKind::Var(_) => {
                for a in args {
                    self.check_expr(&a.value, Exp::Any);
                }
                self.tt.unknown
            }
            _ => {
                let s = self.show(ft);
                self.err(e.span, format!("`{s}` cannot be called"));
                for a in args {
                    self.check_expr(&a.value, Exp::Any);
                }
                self.tt.unknown
            }
        }
    }

    pub(crate) fn call_expr(
        &mut self,
        e: &'p Expr,
        callee: &'p Expr,
        args: &'p [Arg],
        mark: CallMark,
        exp: Exp,
    ) -> Ty {
        match &callee.kind {
            ExprKind::Field {
                base,
                name,
                optional,
            } => {
                if let Some(def) = self.used(name) {
                    self.record(callee, self.tt.unknown);
                    return self.call_def(e, def, args, exp, None);
                }
                self.method_call(e, callee, base, name, *optional, args, mark)
            }
            ExprKind::Name(id) => match self.used(id) {
                Some(def) => {
                    let t = self.call_def(e, def, args, exp, None);
                    self.record(callee, self.tt.unknown);
                    t
                }
                None => {
                    for a in args {
                        self.check_expr(&a.value, Exp::Any);
                    }
                    self.tt.unknown
                }
            },
            ExprKind::DotName(name) => self.dot_call(e, name, args, exp),
            ExprKind::TypeApply { base, args: gargs } => {
                let def = match &base.kind {
                    ExprKind::Name(id) => self.used(id),
                    ExprKind::Field { name, .. } => self.used(name),
                    _ => None,
                };
                match def {
                    Some(d) => self.call_def(e, d, args, exp, Some(gargs)),
                    None => {
                        for a in args {
                            self.check_expr(&a.value, Exp::Any);
                        }
                        self.tt.unknown
                    }
                }
            }
            _ => {
                let ft = self.check_expr(callee, Exp::Any);
                self.call_value(e, ft, args)
            }
        }
    }

    /// Calls something a name was resolved to.
    fn call_def(
        &mut self,
        e: &'p Expr,
        def: DefId,
        args: &'p [Arg],
        exp: Exp,
        explicit: Option<&'p [GenericArg]>,
    ) -> Ty {
        let d = self.def(def);
        match d.kind {
            DefKind::Fn => {
                let Some(sig) = self.fn_sig(def) else {
                    for a in args {
                        self.check_expr(&a.value, Exp::Any);
                    }
                    return self.tt.unknown;
                };
                if sig.has_self {
                    let n = d.name.clone();
                    self.err(
                        e.span,
                        format!("`{n}` is a method; call it on a value: `value.{n}(...)`"),
                    );
                }
                let map = self.generic_map(&sig.generics, explicit, e.span);
                let name = d.name.clone();
                self.match_args(&sig.params, &map, args, e.span, &name);
                let ret = self.tt.subst(sig.ret, &map);
                if let Exp::Is(want) = exp {
                    // Let the expected type help generic results (`const xs: [int] = empty()`).
                    let mark = self.tt.snapshot();
                    if !self.tt.unify(ret, want) {
                        self.tt.rollback(mark);
                    }
                }
                ret
            }
            DefKind::Struct | DefKind::Obj => self.construct(e, def, args, exp),
            DefKind::Variant => self.variant_call(e, def, args, exp),
            DefKind::BuiltinFn if d.name == "ok" || d.name == "err" => {
                let name = d.name.clone();
                self.result_ctor(e, &name, args, exp)
            }
            DefKind::Local | DefKind::Param | DefKind::Const | DefKind::Var => {
                let ft = self.value_of_def(def, e.span, Exp::Any);
                self.call_value(e, ft, args)
            }
            _ => {
                let n = d.name.clone();
                self.err(e.span, format!("`{n}` cannot be called"));
                for a in args {
                    self.check_expr(&a.value, Exp::Any);
                }
                self.tt.unknown
            }
        }
    }

    fn generic_map(
        &mut self,
        generics: &[DefId],
        explicit: Option<&'p [GenericArg]>,
        span: Span,
    ) -> HashMap<DefId, Ty> {
        match explicit {
            Some(gargs) => {
                let lowered = self.lower_generic_args(gargs);
                if lowered.len() != generics.len() {
                    self.err(
                        span,
                        format!(
                            "this function takes {} type argument{}",
                            generics.len(),
                            if generics.len() == 1 { "" } else { "s" }
                        ),
                    );
                    return self.fresh_map(generics, span);
                }
                generics.iter().copied().zip(lowered).collect()
            }
            None => self.fresh_map(generics, span),
        }
    }

    /// `point(x: 1)` runs the type's `init`.
    fn construct(&mut self, e: &'p Expr, def: DefId, args: &'p [Arg], exp: Exp) -> Ty {
        let tn = self.def(def).name.clone();
        let Some(init) = self
            .res
            .member(def, "init")
            .filter(|m| self.def(*m).kind == DefKind::Fn)
        else {
            self.err(
                e.span,
                format!("`{tn}` has no `init`; build it with `{tn} {{ ... }}`"),
            );
            for a in args {
                self.check_expr(&a.value, Exp::Any);
            }
            return self.tt.unknown;
        };
        let Some(sig) = self.fn_sig(init) else {
            return self.tt.unknown;
        };
        let gens = self.adt_generics(def);
        let mut map = self.fresh_map(&gens, e.span);
        let own = self.fresh_map(&sig.generics, e.span);
        map.extend(own);
        self.match_args(&sig.params, &map, args, e.span, &tn);
        // `init` returns `Self`, or `Self ! error`.
        let ret = if self.tt.kind_of(sig.ret) == &TyKind::Void {
            self.own_adt_ty(def)
        } else {
            sig.ret
        };
        let ret = self.tt.subst(ret, &map);
        if let Exp::Is(want) = exp {
            let mark = self.tt.snapshot();
            if !self.tt.unify(ret, want) {
                self.tt.rollback(mark);
            }
        }
        ret
    }

    /// `shape.circle(r: 1.0)`
    fn variant_call(&mut self, e: &'p Expr, variant: DefId, args: &'p [Arg], exp: Exp) -> Ty {
        let Some(owner) = self.def(variant).parent else {
            return self.tt.unknown;
        };
        let fields = self.variant_fields(variant);
        let gens = self.adt_generics(owner);
        let map = self.fresh_map(&gens, e.span);
        let targs: Vec<Ty> = gens.iter().map(|g| map[g]).collect();
        let et = self.tt.mk(TyKind::Adt(owner, targs));
        if let Exp::Is(want) = exp {
            let mark = self.tt.snapshot();
            let w = match self.kind_of(want) {
                TyKind::Option(inner) => inner,
                _ => want,
            };
            if !self.tt.unify(et, w) {
                self.tt.rollback(mark);
            }
        }
        let params: Vec<ParamSig> = fields
            .iter()
            .enumerate()
            .map(|(i, (l, t))| ParamSig {
                name: l.clone().unwrap_or_else(|| format!("#{i}")),
                mode: None,
                ty: *t,
                has_default: false,
                variadic: false,
            })
            .collect();
        let n = self.def(variant).name.clone();
        self.match_args(&params, &map, args, e.span, &n);
        et
    }

    /// `.circle(r: 1.0)`: the enum comes from the expected type.
    fn dot_call(&mut self, e: &'p Expr, name: &'p Ident, args: &'p [Arg], exp: Exp) -> Ty {
        let want = exp.ty().map(|t| match self.kind_of(t) {
            TyKind::Option(inner) => inner,
            _ => t,
        });
        let Some(want) = want else {
            self.err(e.span, "cannot tell which enum this case belongs to; write `type.case(...)` or give a type");
            for a in args {
                self.check_expr(&a.value, Exp::Any);
            }
            return self.tt.unknown;
        };
        let n = self.text(name);
        match self.kind_of(want) {
            TyKind::Adt(def, _) => match self
                .res
                .member(def, n)
                .filter(|m| self.def(*m).kind == DefKind::Variant)
            {
                Some(v) => self.variant_call(e, v, args, Exp::Is(want)),
                None => {
                    let tn = self.def(def).name.clone();
                    self.err(name.span, format!("`{tn}` has no case `{n}`"));
                    for a in args {
                        self.check_expr(&a.value, Exp::Any);
                    }
                    self.tt.unknown
                }
            },
            _ => {
                for a in args {
                    self.check_expr(&a.value, Exp::Any);
                }
                if !self.tt.is_unknown(want) {
                    let s = self.show(want);
                    self.err(
                        e.span,
                        format!("`.{n}(...)` needs an enum, but `{s}` is expected here"),
                    );
                }
                self.tt.unknown
            }
        }
    }

    /// `ok(x)` and `err(e)`.
    fn result_ctor(&mut self, e: &'p Expr, name: &str, args: &'p [Arg], exp: Exp) -> Ty {
        if args.len() != 1 {
            self.err(e.span, format!("`{name}` takes one value"));
            for a in args {
                self.check_expr(&a.value, Exp::Any);
            }
            return self.tt.unknown;
        }
        let (mut okt, mut errt) = match exp.ty().map(|t| self.kind_of(t)) {
            Some(TyKind::Result(a, b)) => (a, b),
            Some(TyKind::Option(inner)) => match self.kind_of(inner) {
                TyKind::Result(a, b) => (a, b),
                _ => (self.var(e.span), self.var(e.span)),
            },
            _ => (self.var(e.span), self.var(e.span)),
        };
        let is_ok = name == "ok";
        let arg = &args[0].value;
        let target = if is_ok { okt } else { errt };
        let t = self.check_expr(arg, Exp::Is(target));
        if self.try_coerce(t, target) {
            // keep the expected type
        } else if !self.tt.is_unknown(t) && !self.tt.is_unknown(target) {
            let (a, b) = (self.show(t), self.show(target));
            self.err(
                arg.span,
                format!("`{name}` carries `{a}`, but `{b}` is expected"),
            );
        }
        if is_ok {
            okt = target;
        } else {
            errt = target;
        }
        self.tt.result(okt, errt)
    }

    /// `value.method(args)`
    #[allow(clippy::too_many_arguments)]
    fn method_call(
        &mut self,
        e: &'p Expr,
        callee: &'p Expr,
        base: &'p Expr,
        name: &'p Ident,
        optional: bool,
        args: &'p [Arg],
        mark: CallMark,
    ) -> Ty {
        let bt = self.check_expr(base, Exp::Any);
        let (recv, wrap) = if optional {
            match self.kind_of(bt) {
                TyKind::Option(inner) => (inner, true),
                _ => (bt, false),
            }
        } else {
            (bt, false)
        };
        let n = self.text(name);
        self.record(callee, self.tt.unknown);
        self.settle_literal(recv);
        let rk = self.kind_of(recv);
        if matches!(rk, TyKind::Unknown | TyKind::Var(_)) {
            for a in args {
                self.check_expr(&a.value, Exp::Any);
            }
            return self.tt.unknown;
        }
        let Some(m) = self.lookup_method(recv, n, name.span) else {
            // A field that holds a function: `self.callback(x)`.
            if let TyKind::Adt(def, targs) = &rk {
                if let Some(ft) = self.find_field(*def, targs, n, name.span) {
                    if matches!(self.kind_of(ft), TyKind::Fn(..)) {
                        return self.call_value(e, ft, args);
                    }
                }
            }
            for a in args {
                self.check_expr(&a.value, Exp::Any);
            }
            let s = self.show(recv);
            let mut msg = format!("`{s}` has no method `{n}`");
            let cands = self.candidate_methods(recv);
            if let Some(c) = self.closest(n, cands.iter().map(|s| s.as_str())) {
                msg.push_str(&format!("; did you mean `{c}`?"));
            }
            self.err(name.span, msg);
            return self.tt.unknown;
        };
        if !m.sig.has_self {
            let s = self.show(recv);
            self.err(
                name.span,
                format!("`{n}` has no `self`; call it on the type: `{s}.{n}(...)`"),
            );
        }
        // The call site shows whether the receiver changes or is consumed.
        let wanted = match m.sig.self_mode {
            Some(Mode::Inout) => CallMark::Changes,
            Some(Mode::Sink) => CallMark::Consumes,
            _ => CallMark::None,
        };
        if wanted != mark {
            let msg = match (wanted, mark) {
                (CallMark::Changes, _) => {
                    format!("`{n}` changes the value it is called on; write `.{n}!(...)`")
                }
                (CallMark::Consumes, _) => {
                    format!("`{n}` consumes the value it is called on; write `.{n}!!(...)`")
                }
                (CallMark::None, CallMark::Changes) => format!("`{n}` only reads; remove the `!`"),
                (CallMark::None, _) => format!("`{n}` only reads; remove the `!!`"),
            };
            self.err(name.span, msg);
        }
        let mut map = m.map.clone();
        let own = self.fresh_map(&m.sig.generics, e.span);
        map.extend(own);
        self.match_args(&m.sig.params, &map, args, e.span, n);
        let ret = self.tt.subst(m.sig.ret, &map);
        if wrap {
            if matches!(self.kind_of(ret), TyKind::Option(_)) {
                ret
            } else {
                self.tt.option(ret)
            }
        } else {
            ret
        }
    }

    pub(crate) fn type_apply(
        &mut self,
        e: &'p Expr,
        base: &'p Expr,
        gargs: &'p [GenericArg],
    ) -> Ty {
        let def = match &base.kind {
            ExprKind::Name(id) => self.used(id),
            ExprKind::Field { name, .. } => self.used(name),
            _ => None,
        };
        let Some(def) = def else {
            return self.tt.unknown;
        };
        if self.def(def).kind != DefKind::Fn {
            return self.tt.unknown;
        }
        let Some(sig) = self.fn_sig(def) else {
            return self.tt.unknown;
        };
        let map = self.generic_map(&sig.generics, Some(gargs), e.span);
        let params: Vec<ParamTy> = sig
            .params
            .iter()
            .map(|p| ParamTy {
                mode: p.mode,
                ty: self.tt.subst(p.ty, &map),
            })
            .collect();
        let ret = self.tt.subst(sig.ret, &map);
        self.tt.mk(TyKind::Fn(params, ret))
    }
}
