//! Typing patterns: what a pattern binds, and that it fits the value it is matched against.

use super::*;

impl<'p> Checker<'p> {
    fn bind_name(&mut self, id: &Ident, ty: Ty) {
        if let Some(d) = self.declared(id) {
            self.local_tys.insert(d, ty);
        } else if let Some(d) = self.used(id) {
            // A later alternative of a `case` binds the same name: the types must agree.
            if let Some(&prev) = self.local_tys.get(&d) {
                if !self.tt.unify(prev, ty) {
                    let (a, b) = (self.show(prev), self.show(ty));
                    self.err(
                        id.span,
                        format!(
                            "this alternative binds `{}` as `{b}`, but another binds it as `{a}`",
                            self.text(id)
                        ),
                    );
                }
            }
        }
    }

    pub(crate) fn check_pattern(&mut self, p: &'p Pattern, ty: Ty) {
        match &p.kind {
            PatternKind::Wildcard | PatternKind::Error => {}
            PatternKind::Binding { name, .. } => self.bind_name(name, ty),
            PatternKind::As { name, pattern } => {
                if let PatternKind::TypeTest(path) = &pattern.kind {
                    let t = self.type_test(path, ty, pattern.span);
                    self.bind_name(name, t);
                } else {
                    self.bind_name(name, ty);
                    self.check_pattern(pattern, ty);
                }
            }
            PatternKind::Literal(e) => {
                let lt = self.check_expr(e, Exp::Is(ty));
                if !self.tt.unify(lt, ty) && !self.tt.is_unknown(ty) && !self.tt.is_unknown(lt) {
                    // Text patterns match `string` and `str` alike.
                    let text =
                        |c: &Self, t: Ty| matches!(c.kind_of(t), TyKind::String | TyKind::Str);
                    if !(text(self, lt) && text(self, ty)) {
                        let (a, b) = (self.show(lt), self.show(ty));
                        self.err(
                            p.span,
                            format!("this pattern is `{a}`, but the value is `{b}`"),
                        );
                    }
                }
            }
            PatternKind::Range { start, end, .. } => {
                for x in [start, end].into_iter().flatten() {
                    let t = self.check_expr(x, Exp::Is(ty));
                    if !self.tt.unify(t, ty) && !self.tt.is_unknown(ty) {
                        let (a, b) = (self.show(t), self.show(ty));
                        self.err(
                            x.span,
                            format!("this range end is `{a}`, but the value is `{b}`"),
                        );
                    }
                }
            }
            PatternKind::Tuple(ps) => self.tuple_pattern(p, ps, ty),
            PatternKind::List(items) => self.list_pattern(p, items, ty),
            PatternKind::Variant {
                leading_dot,
                path,
                args,
            } => {
                self.variant_pattern(p, *leading_dot, path, args.as_deref(), ty);
            }
            PatternKind::None => {
                let inner = self.var(p.span);
                let opt = self.tt.option(inner);
                if !self.tt.unify(opt, ty) && !self.tt.is_unknown(ty) {
                    let s = self.show(ty);
                    self.err(
                        p.span,
                        format!("`none` is a pattern for options, but the value is `{s}`"),
                    );
                }
            }
            PatternKind::TypeTest(path) => {
                self.type_test(path, ty, p.span);
            }
            PatternKind::Struct {
                path,
                fields,
                has_rest,
            } => {
                self.struct_pattern(p, path, fields, *has_rest, ty);
            }
        }
    }

    fn tuple_pattern(&mut self, p: &'p Pattern, ps: &'p [Pattern], ty: Ty) {
        if ps.is_empty() {
            return;
        }
        match self.kind_of(ty) {
            TyKind::Tuple(ts) if ts.len() == ps.len() => {
                for (sub, t) in ps.iter().zip(ts.iter()) {
                    self.check_pattern(sub, *t);
                }
            }
            TyKind::AltTuple(ts, _) if !ts.is_empty() => {
                for (i, sub) in ps.iter().enumerate() {
                    self.check_pattern(sub, ts[i % ts.len()]);
                }
            }
            TyKind::Var(_) => {
                let vars: Vec<Ty> = ps.iter().map(|_| self.var(p.span)).collect();
                let tup = self.tt.tuple(vars.clone());
                self.tt.unify(tup, ty);
                for (sub, t) in ps.iter().zip(vars.iter()) {
                    self.check_pattern(sub, *t);
                }
            }
            TyKind::Unknown => {
                for sub in ps {
                    self.check_pattern(sub, self.tt.unknown);
                }
            }
            _ => {
                let s = self.show(ty);
                self.err(
                    p.span,
                    format!(
                        "this pattern has {} parts, but the value is `{s}`",
                        ps.len()
                    ),
                );
                for sub in ps {
                    self.check_pattern(sub, self.tt.unknown);
                }
            }
        }
    }

    fn list_pattern(&mut self, p: &'p Pattern, items: &'p [ListPatItem], ty: Ty) {
        let elem = match self.kind_of(ty) {
            TyKind::List(e) | TyKind::Array(e, _) => e,
            TyKind::Unknown => self.tt.unknown,
            TyKind::Var(_) => {
                let e = self.var(p.span);
                let l = self.tt.list(e);
                self.tt.unify(l, ty);
                e
            }
            _ => {
                let s = self.show(ty);
                self.err(
                    p.span,
                    format!("a list pattern needs a list, but the value is `{s}`"),
                );
                self.tt.unknown
            }
        };
        for it in items {
            match it {
                ListPatItem::Pattern(sub) => self.check_pattern(sub, elem),
                ListPatItem::Rest { name: Some(n), .. } => {
                    let l = self.tt.list(elem);
                    self.bind_name(n, l);
                }
                ListPatItem::Rest { .. } => {}
            }
        }
    }

    /// `x is dog`: the tested type must be the value's type or a kind of it. Returns the tested type.
    fn type_test(&mut self, path: &'p Path, ty: Ty, span: Span) -> Ty {
        let Some(last) = path.segments.last() else {
            return self.tt.unknown;
        };
        let Some(def) = self.used(last) else {
            return self.tt.unknown;
        };
        if !matches!(
            self.def(def).kind,
            DefKind::Struct | DefKind::Obj | DefKind::Enum
        ) {
            return self.tt.unknown;
        }
        let gens = self.adt_generics(def);
        let map = self.fresh_map(&gens, span);
        let args: Vec<Ty> = gens.iter().map(|g| map[g]).collect();
        let tested = self.tt.mk(TyKind::Adt(def, args));
        let vt = self.tt.shallow(ty);
        if self.tt.is_unknown(vt) || matches!(self.tt.kind(vt), TyKind::Var(_)) {
            return tested;
        }
        let ok = self.tt.unify(vt, tested)
            || match (self.kind_of(tested), self.kind_of(vt)) {
                (TyKind::Adt(d1, a1), TyKind::Adt(d2, a2)) => self.is_subtype(d1, &a1, d2, &a2),
                (_, TyKind::Any(_)) => true,
                (_, TyKind::Option(inner)) => self.tt.unify(tested, inner),
                _ => false,
            };
        if !ok {
            let (a, b) = (self.show(tested), self.show(ty));
            self.err(span, format!("`{a}` is not a kind of `{b}`"));
        }
        tested
    }

    fn struct_pattern(
        &mut self,
        p: &'p Pattern,
        path: &'p Path,
        fields: &'p [FieldPat],
        has_rest: bool,
        ty: Ty,
    ) {
        let Some(last) = path.segments.last() else {
            return;
        };
        let Some(def) = self.used(last) else {
            for f in fields {
                if let Some(sub) = &f.pattern {
                    self.check_pattern(sub, self.tt.unknown);
                } else {
                    self.bind_name(&f.name, self.tt.unknown);
                }
            }
            return;
        };
        if !matches!(self.def(def).kind, DefKind::Struct | DefKind::Obj) {
            return;
        }
        let gens = self.adt_generics(def);
        let map = self.fresh_map(&gens, p.span);
        let args: Vec<Ty> = gens.iter().map(|g| map[g]).collect();
        let st = self.tt.mk(TyKind::Adt(def, args));
        if !self.tt.unify(st, ty) && !self.tt.is_unknown(ty) {
            let (a, b) = (self.show(st), self.show(ty));
            self.err(
                p.span,
                format!("this pattern is for `{a}`, but the value is `{b}`"),
            );
        }
        let members: Vec<DefId> = self
            .def(def)
            .members
            .iter()
            .copied()
            .filter(|m| self.def(*m).kind == DefKind::Field)
            .collect();
        let mut seen: Vec<DefId> = Vec::new();
        for f in fields {
            let name = self.text(&f.name);
            let Some(fd) = self
                .res
                .member(def, name)
                .filter(|m| self.def(*m).kind == DefKind::Field)
            else {
                let tn = self.def(def).name.clone();
                self.err(f.name.span, format!("`{tn}` has no field `{name}`"));
                match &f.pattern {
                    Some(sub) => self.check_pattern(sub, self.tt.unknown),
                    None => self.bind_name(&f.name, self.tt.unknown),
                }
                continue;
            };
            seen.push(fd);
            let ft = self.field_ty(fd);
            let ft = self.tt.subst(ft, &map);
            match &f.pattern {
                Some(sub) => self.check_pattern(sub, ft),
                None => self.bind_name(&f.name, ft),
            }
        }
        if !has_rest {
            let missing: Vec<&str> = members
                .iter()
                .filter(|m| !seen.contains(m))
                .map(|m| self.def(*m).name.as_str())
                .collect();
            if !missing.is_empty() {
                self.err(
                    p.span,
                    format!(
                        "this pattern leaves out {}; add them or write `..`",
                        missing
                            .iter()
                            .map(|m| format!("`{m}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
            }
        }
    }

    fn variant_pattern(
        &mut self,
        p: &'p Pattern,
        leading_dot: bool,
        path: &'p Path,
        args: Option<&'p [PatArg]>,
        ty: Ty,
    ) {
        let Some(first) = path.segments.first() else {
            return;
        };
        let last = path.segments.last().unwrap_or(first);
        let name = self.text(last);
        let unknown_args = |c: &mut Self| {
            for a in args.into_iter().flatten() {
                c.check_pattern(&a.pattern, c.tt.unknown);
            }
        };
        // `ok`, `err` and `some` belong to results and options.
        if !leading_dot && path.segments.len() == 1 {
            let builtin = self
                .used(first)
                .map(|d| (self.def(d).kind, self.def(d).name.clone()));
            if let Some((DefKind::BuiltinFn | DefKind::BuiltinCase, n)) = builtin {
                self.builtin_case_pattern(p, &n, args, ty);
                return;
            }
        }
        // Find the case and its enum.
        let variant = if leading_dot {
            let want = self.tt.shallow(ty);
            match self.kind_of(want) {
                TyKind::Adt(def, _) => self.res.member(def, name),
                TyKind::Unknown | TyKind::Var(_) => None,
                _ => {
                    let s = self.show(ty);
                    self.err(
                        p.span,
                        format!("`.{name}` needs an enum, but the value is `{s}`"),
                    );
                    None
                }
            }
        } else {
            self.used(last)
        };
        let Some(v) = variant.filter(|v| self.def(*v).kind == DefKind::Variant) else {
            if leading_dot && matches!(self.kind_of(ty), TyKind::Adt(..)) {
                let tn = match self.kind_of(ty) {
                    TyKind::Adt(d, _) => self.def(d).name.clone(),
                    _ => String::new(),
                };
                self.err(last.span, format!("`{tn}` has no case `{name}`"));
            }
            unknown_args(self);
            return;
        };
        let Some(owner) = self.def(v).parent else {
            return;
        };
        let gens = self.adt_generics(owner);
        let map = self.fresh_map(&gens, p.span);
        let eargs: Vec<Ty> = gens.iter().map(|g| map[g]).collect();
        let et = self.tt.mk(TyKind::Adt(owner, eargs));
        if !self.tt.unify(et, ty) && !self.tt.is_unknown(ty) {
            let (a, b) = (self.show(et), self.show(ty));
            self.err(
                p.span,
                format!("this case is of `{a}`, but the value is `{b}`"),
            );
        }
        let fields = self.variant_fields(v);
        match args {
            None if fields.is_empty() => {}
            None => {
                let n = self.def(v).name.clone();
                self.err(p.span, format!("`{n}` carries {} value{}; write `{n}(...)`, using `_` for what you do not need", fields.len(), if fields.len() == 1 { "" } else { "s" }));
            }
            Some(args) => {
                if args.len() != fields.len() {
                    let n = self.def(v).name.clone();
                    self.err(
                        p.span,
                        format!(
                            "`{n}` carries {} value{}, but this pattern has {}",
                            fields.len(),
                            if fields.len() == 1 { "" } else { "s" },
                            args.len()
                        ),
                    );
                    unknown_args(self);
                    return;
                }
                for (i, a) in args.iter().enumerate() {
                    // A label picks the field by name; otherwise by position.
                    let idx = match &a.label {
                        Some(l) => {
                            let ln = self.text(l);
                            match fields.iter().position(|(fl, _)| fl.as_deref() == Some(ln)) {
                                Some(k) => k,
                                None => {
                                    self.err(l.span, format!("this case has no field `{ln}`"));
                                    i
                                }
                            }
                        }
                        None => i,
                    };
                    let ft = self.tt.subst(fields[idx].1, &map);
                    self.check_pattern(&a.pattern, ft);
                }
            }
        }
    }

    fn builtin_case_pattern(
        &mut self,
        p: &'p Pattern,
        name: &str,
        args: Option<&'p [PatArg]>,
        ty: Ty,
    ) {
        let a = self.var(p.span);
        let (shape, payload) = match name {
            "some" => (self.tt.option(a), a),
            "ok" => {
                let b = self.var(p.span);
                (self.tt.result(a, b), a)
            }
            _ => {
                let b = self.var(p.span);
                (self.tt.result(b, a), a)
            }
        };
        if !self.tt.unify(shape, ty) && !self.tt.is_unknown(ty) {
            let s = self.show(ty);
            let want = if name == "some" {
                "an option"
            } else {
                "a result"
            };
            self.err(
                p.span,
                format!("`{name}` is a pattern for {want}, but the value is `{s}`"),
            );
        }
        match args {
            Some([one]) => self.check_pattern(&one.pattern, payload),
            _ => self.err(p.span, format!("`{name}` carries one value: `{name}(x)`")),
        }
    }
}
