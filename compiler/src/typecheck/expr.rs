//! Typing expressions.

use super::*;
use crate::token::Punct;

impl<'p> Checker<'p> {
    // ---- Coercion ---------------------------------------------------------------------------

    fn peel_option(&self, t: Ty) -> Ty {
        match self.tt.kind_of(t) {
            TyKind::Option(inner) => *inner,
            _ => t,
        }
    }

    /// May a value of type `found` be used where `expected` is wanted? (TC2: a value to an
    /// option of its type, an `obj` to its base, `never` to anything, `string` to a `str` view.)
    pub(crate) fn try_coerce(&mut self, found: Ty, expected: Ty) -> bool {
        if self.tt.unify(found, expected) {
            return true;
        }
        let f = self.tt.shallow(found);
        let e = self.tt.shallow(expected);
        match (self.tt.kind(f).clone(), self.tt.kind(e).clone()) {
            (TyKind::Never, _) => true,
            (_, TyKind::Option(inner)) => self.try_coerce(f, inner),
            (TyKind::String, TyKind::Str) => true,
            (_, TyKind::Any(i)) => self.provides(f, i),
            (TyKind::Adt(d1, a1), TyKind::Adt(d2, a2)) => self.is_subtype(d1, &a1, d2, &a2),
            _ => false,
        }
    }

    /// Is the type `d1<a1>` an `obj` that extends `d2<a2>`?
    pub(crate) fn is_subtype(&mut self, d1: DefId, a1: &[Ty], d2: DefId, a2: &[Ty]) -> bool {
        if d1 == d2 {
            return false; // equal types were unified already
        }
        let bases = self.bases_of(d1);
        let gens = self.adt_generics(d1);
        let map: HashMap<DefId, Ty> = gens.iter().copied().zip(a1.iter().copied()).collect();
        for b in bases {
            let b = self.tt.subst(b, &map);
            if let TyKind::Adt(bd, ba) = self.tt.kind(b).clone() {
                if bd == d2 {
                    return a2.len() == ba.len()
                        && ba.iter().zip(a2.iter()).all(|(x, y)| self.tt.unify(*x, *y));
                }
                if self.is_subtype(bd, &ba, d2, a2) {
                    return true;
                }
            }
        }
        false
    }

    /// Coerces and reports a mismatch.
    pub(crate) fn coerce_at(&mut self, found: Ty, expected: Ty, span: Span, what: &str) -> bool {
        if self.try_coerce(found, expected) {
            return true;
        }
        if !self.tt.is_unknown(found) && !self.tt.is_unknown(expected) {
            let (f, e) = (self.show(found), self.show(expected));
            let hint = self.mismatch_hint(found, expected);
            self.err(
                span,
                format!("{what} has type `{f}`, but `{e}` is expected{hint}"),
            );
        }
        false
    }

    fn mismatch_hint(&self, found: Ty, expected: Ty) -> String {
        let f = self.tt.kind_of(found);
        let e = self.tt.kind_of(expected);
        if let TyKind::Any(i) = e {
            let (fname, iname) = (self.show(found), self.def(*i).name.clone());
            return format!("; `{fname}` does not provide `{iname}`");
        }
        let numeric = |k: &TyKind| matches!(k, TyKind::Int(_) | TyKind::Float(_));
        if numeric(f) && numeric(e) {
            return "; convert with `to`".to_string();
        }
        if let TyKind::Option(inner) = f {
            if self.tt.kind_of(*inner) == e {
                return "; the value may be `none`: use `??` or `guard`".to_string();
            }
        }
        String::new()
    }

    /// Checks `e` against `expected` and coerces. Returns `expected` when it fits.
    pub(crate) fn check_coerce(&mut self, e: &'p Expr, expected: Ty) -> Ty {
        let found = self.check_expr(e, Exp::Is(expected));
        if self.coerce_at(found, expected, e.span, "this") {
            expected
        } else {
            found
        }
    }

    /// Combines the types of branches (`if`, `switch`, ternary).
    pub(crate) fn join(&mut self, branches: &[(Ty, Span)], exp: Exp, what: &str) -> Ty {
        let all_never = branches
            .iter()
            .all(|(t, _)| matches!(self.tt.kind_of(*t), TyKind::Never));
        match exp {
            Exp::Discard => {
                if all_never && !branches.is_empty() {
                    self.tt.never
                } else {
                    self.tt.void
                }
            }
            Exp::Is(want) => {
                for (t, span) in branches {
                    self.coerce_at(*t, want, *span, what);
                }
                if all_never && !branches.is_empty() {
                    self.tt.never
                } else {
                    want
                }
            }
            Exp::Any => {
                let first = branches
                    .iter()
                    .find(|(t, _)| !matches!(self.tt.kind_of(*t), TyKind::Never));
                let Some(&(first_ty, _)) = first else {
                    return self.tt.never;
                };
                for (t, span) in branches {
                    if !self.try_coerce(*t, first_ty)
                        && !self.tt.is_unknown(*t)
                        && !self.tt.is_unknown(first_ty)
                    {
                        let (a, b) = (self.show(*t), self.show(first_ty));
                        self.err(
                            *span,
                            format!("{what} have different types: `{b}` and `{a}`"),
                        );
                    }
                }
                first_ty
            }
        }
    }

    /// An unsolved variable that is not a number literal: we do not know anything yet.
    pub(crate) fn open_var(&self, t: Ty) -> bool {
        matches!(self.tt.kind_of(t), TyKind::Var(_))
            && self
                .tt
                .var_info(t)
                .is_none_or(|(k, _)| k == VarKind::General)
    }

    pub(crate) fn is_numeric(&self, t: Ty) -> bool {
        match self.tt.kind_of(t) {
            TyKind::Int(_) | TyKind::Float(_) | TyKind::Unknown => true,
            TyKind::Var(_) => self
                .tt
                .var_info(t)
                .is_some_and(|(k, _)| matches!(k, VarKind::Int | VarKind::Float)),
            _ => false,
        }
    }

    pub(crate) fn is_integer(&self, t: Ty) -> bool {
        match self.tt.kind_of(t) {
            TyKind::Int(_) | TyKind::Unknown => true,
            TyKind::Var(_) => self.tt.var_info(t).is_some_and(|(k, _)| k == VarKind::Int),
            _ => false,
        }
    }

    fn int_range_ty(&mut self) -> Ty {
        let i = self.tt.int;
        self.tt.mk(TyKind::Range(i))
    }

    // ---- Entry --------------------------------------------------------------------------------

    pub(crate) fn check_expr(&mut self, e: &'p Expr, exp: Exp) -> Ty {
        let t = self.check_expr_inner(e, exp);
        self.record(e, t);
        t
    }

    fn operand_exp(&self, exp: Exp) -> Exp {
        match exp {
            Exp::Is(t) => {
                let t = self.peel_option(t);
                if self.is_numeric(t) && !self.tt.is_unknown(t) {
                    Exp::Is(t)
                } else {
                    Exp::Any
                }
            }
            _ => Exp::Any,
        }
    }

    fn check_expr_inner(&mut self, e: &'p Expr, exp: Exp) -> Ty {
        match &e.kind {
            ExprKind::Int => self.int_literal(e, exp, false),
            ExprKind::Float => {
                let target = exp.ty().map(|t| self.peel_option(t));
                if let Some(t) = target {
                    if matches!(self.tt.kind_of(t), TyKind::Float(_))
                        || self
                            .tt
                            .var_info(t)
                            .is_some_and(|(k, _)| k == VarKind::Float)
                    {
                        return t;
                    }
                }
                self.tt.fresh(VarKind::Float, e.span)
            }
            ExprKind::Char => self.tt.char_,
            ExprKind::Bool(_) => self.tt.bool_,
            ExprKind::None => match exp.ty().map(|t| self.tt.shallow(t)) {
                Some(t) if matches!(self.tt.kind(t), TyKind::Option(_)) => t,
                _ => {
                    let inner = self.var(e.span);
                    self.tt.option(inner)
                }
            },
            ExprKind::Str(parts) => {
                for p in parts {
                    if let StrPart::Interp(inner) = p {
                        self.check_expr(inner, Exp::Any);
                    }
                }
                self.tt.string
            }
            ExprKind::Name(id) => match self.used(id) {
                Some(def) => self.value_of_def(def, e.span, exp),
                None => self.tt.unknown,
            },
            ExprKind::SelfValue => match self.res.use_at(self.file, e.span.start) {
                Some(d) => self.local_tys.get(&d).copied().unwrap_or(self.tt.unknown),
                None => self.tt.unknown,
            },
            ExprKind::SelfType => self.tt.unknown,
            ExprKind::Underscore => self.tt.unknown,
            ExprKind::DotName(name) => self.dot_name(e, name, exp),
            ExprKind::Paren(inner) => self.check_expr(inner, exp),
            ExprKind::Tuple(items) => self.tuple_expr(items, exp),
            ExprKind::List(items) => self.list_expr(e, items, exp),
            ExprKind::Set(items) => self.set_expr(e, items, exp),
            ExprKind::Repeat { value, count } => self.repeat_expr(value, count, exp),
            ExprKind::Map(items) => self.map_expr(e, items, exp),
            ExprKind::Builder(b) => self.builder_expr(e, b, exp),
            ExprKind::StructLit { ty, fields } => self.struct_lit(e, ty, fields, exp),
            ExprKind::Field {
                base,
                name,
                optional,
            } => self.field_expr(e, base, name, *optional, exp),
            ExprKind::TupleIndex { base, index } => self.tuple_index(e, base, *index),
            ExprKind::Index { base, index } => self.index_expr(e, base, index),
            ExprKind::Call { callee, args, mark } => self.call_expr(e, callee, args, *mark, exp),
            ExprKind::TypeApply { base, args } => self.type_apply(e, base, args),
            ExprKind::Unary { op, operand } => self.unary(e, *op, operand, exp),
            ExprKind::Copy(inner) => {
                let t = self.check_expr(inner, exp);
                if let Some(i) = self.core_iface("clone") {
                    if !self.provides(t, i) {
                        let s = self.show(t);
                        self.err(
                            e.span,
                            format!("`copy` needs a type that can be copied, but `{s}` cannot; an `obj` must provide `clone` (`impl clone for ...`)"),
                        );
                    }
                }
                t
            }
            ExprKind::Binary { op, lhs, rhs } => self.binary(e, *op, lhs, rhs, exp),
            ExprKind::In { lhs, rhs } => self.in_expr(lhs, rhs),
            ExprKind::Assign { op, lhs, rhs } => self.assign(e, *op, lhs, rhs),
            ExprKind::IncDec { operand, .. } => {
                let t = self.check_expr(operand, Exp::Any);
                if !self.is_numeric(t) {
                    let s = self.show(t);
                    self.err(
                        operand.span,
                        format!("`++` and `--` need a number, not `{s}`"),
                    );
                }
                self.tt.void
            }
            ExprKind::Range {
                start, end, step, ..
            } => self.range_expr(e, start, end, step, exp),
            ExprKind::Ternary { cond, then, else_ } => {
                self.check_coerce(cond, self.tt.bool_);
                let a = self.check_expr(then, exp);
                let b = self.check_expr(else_, exp);
                self.join(
                    &[(a, then.span), (b, else_.span)],
                    exp,
                    "the two sides of `? :`",
                )
            }
            ExprKind::Try(inner) => self.try_expr(e, inner),
            ExprKind::Is { value, pattern } => {
                let vt = self.check_expr(value, Exp::Any);
                self.check_pattern(pattern, vt);
                self.tt.bool_
            }
            ExprKind::Convert { value, ty, checked } => self.convert(e, value, ty, *checked),
            ExprKind::Closure(c) => self.closure(e, c, exp),
            ExprKind::If(i) => self.if_expr(i, exp, false),
            ExprKind::Unless(i) => self.if_expr(i, exp, true),
            ExprKind::MetaIf(i) => self.if_expr(i, exp, false),
            ExprKind::Switch(s) => self.switch_expr(e, s, exp),
            ExprKind::Select(arms) => self.select_expr(arms, exp),
            ExprKind::Scope { name, body } => {
                // The handle `s` is a library value, so `s.spawn(...)` is an ordinary method call.
                let handle = (0..self.res.defs.len() as u32).map(DefId).find(|d| {
                    let def = self.def(*d);
                    def.kind == DefKind::Struct
                        && def.name == "scope_handle"
                        && self.is_core_def(*d)
                });
                let decl = self.res.decls.get(&(self.file, name.span.start)).copied();
                if let (Some(h), Some(d)) = (handle, decl) {
                    let ht = self.own_adt_ty(h);
                    self.local_tys.insert(d, ht);
                }
                let t = self.check_block(body, Exp::Discard);
                let _ = t;
                self.tt.void
            }
            ExprKind::Block(b) | ExprKind::Unsafe(b) => self.check_block(b, exp),
            ExprKind::Loop(b) => self.loop_expr(None, b, exp),
            ExprKind::While { cond, body } => {
                self.check_coerce(cond, self.tt.bool_);
                self.looped(None, false, |c| {
                    c.check_block(body, Exp::Discard);
                });
                self.tt.void
            }
            ExprKind::DoWhile { body, cond } => {
                self.looped(None, false, |c| {
                    c.check_block(body, Exp::Discard);
                });
                self.check_coerce(cond, self.tt.bool_);
                self.tt.void
            }
            ExprKind::For(f) => self.for_expr(f),
            ExprKind::Guard { cond, else_ } => {
                self.check_coerce(cond, self.tt.bool_);
                self.check_block(else_, Exp::Discard);
                self.tt.void
            }
            ExprKind::Return(v) => self.return_expr(e, v),
            ExprKind::Yield(v) => {
                if let Some(x) = v {
                    self.check_expr(x, Exp::Any);
                }
                self.tt.void
            }
            ExprKind::Break { label, value } => {
                self.break_expr(e, label.as_ref(), value.as_deref())
            }
            ExprKind::Continue { .. } => self.tt.never,
            ExprKind::Become(call) => self.become_expr(e, call),
            ExprKind::Defer(inner) => {
                let t = self.check_expr(inner, Exp::Discard);
                if matches!(self.kind_of(t), TyKind::Result(..)) {
                    self.err(
                        inner.span,
                        "this returns a result, and `defer` has nowhere to send the error; handle it inside the deferred code",
                    );
                }
                self.tt.void
            }
            ExprKind::Meta(inner) => self.check_expr(inner, exp),
            ExprKind::Labeled { label, body } => self.labeled(label, body, exp),
            ExprKind::Error => self.tt.unknown,
        }
    }

    // ---- Literals ------------------------------------------------------------------------------

    pub(crate) fn int_literal(&mut self, e: &'p Expr, exp: Exp, negative: bool) -> Ty {
        let src = &self.program.module(self.file).src;
        let text = &src[e.span.start as usize..e.span.end as usize];
        let value = literal_value(text);
        let ty = match exp.ty().map(|t| self.peel_option(t)) {
            Some(t)
                if matches!(self.tt.kind_of(t), TyKind::Int(_))
                    || self.tt.var_info(t).is_some_and(|(k, _)| k == VarKind::Int) =>
            {
                t
            }
            _ => self.tt.fresh(VarKind::Int, e.span),
        };
        match value {
            Some(v) => self.lits.push(LitCheck {
                ty,
                magnitude: v,
                negative,
                file: self.file,
                span: e.span,
            }),
            None => self.err(e.span, "this number is too large"),
        }
        self.record(e, ty);
        ty
    }

    // ---- Names ----------------------------------------------------------------------------------

    pub(crate) fn value_of_def(&mut self, def: DefId, span: Span, exp: Exp) -> Ty {
        let d = self.def(def);
        match d.kind {
            DefKind::Local | DefKind::Param => {
                self.local_tys.get(&def).copied().unwrap_or(self.tt.unknown)
            }
            DefKind::Const | DefKind::Var => match self.nodes.get(&def) {
                Some(Node::Global { .. }) => self.global_ty(def),
                _ => self.local_tys.get(&def).copied().unwrap_or(self.tt.unknown),
            },
            DefKind::Fn => match self.fn_value(def, span) {
                Some(t) => t,
                None => self.tt.unknown,
            },
            DefKind::Variant => self.variant_value(def, exp),
            DefKind::Field => self.tt.unknown,
            DefKind::BuiltinFn => {
                let name = d.name.clone();
                self.err(
                    span,
                    format!("`{name}` is not a value; call it: `{name}(...)`"),
                );
                self.tt.unknown
            }
            DefKind::Struct
            | DefKind::Obj
            | DefKind::Enum
            | DefKind::Interface
            | DefKind::TypeAlias
            | DefKind::BuiltinType => {
                let name = d.name.clone();
                self.err(span, format!("`{name}` is a type, not a value"));
                self.tt.unknown
            }
            _ => self.tt.unknown,
        }
    }

    /// A function used as a value: `fn(params) -> ret`, generics instantiated.
    pub(crate) fn fn_value(&mut self, def: DefId, span: Span) -> Option<Ty> {
        let sig = self.fn_sig(def)?;
        if sig.has_self {
            let name = self.def(def).name.clone();
            self.err(
                span,
                format!("`{name}` is a method; call it on a value: `value.{name}(...)`"),
            );
            return Some(self.tt.unknown);
        }
        let map = self.fresh_map(&sig.generics, span);
        self.note_call(span, def, &map);
        let params: Vec<ParamTy> = sig
            .params
            .iter()
            .map(|p| ParamTy {
                mode: p.mode,
                ty: self.tt.subst(p.ty, &map),
            })
            .collect();
        let ret = self.tt.subst(sig.ret, &map);
        Some(self.tt.mk(TyKind::Fn(params, ret)))
    }

    pub(crate) fn fresh_map(&mut self, generics: &[DefId], span: Span) -> HashMap<DefId, Ty> {
        let tys: Vec<Ty> = generics.iter().map(|_| self.var(span)).collect();
        self.queue_bounds(generics, &tys, span);
        generics.iter().copied().zip(tys).collect()
    }

    /// An enum case used as a value: a constant for `empty`, a function for `circle(r)`.
    fn variant_value(&mut self, variant: DefId, exp: Exp) -> Ty {
        let Some(owner) = self.def(variant).parent else {
            return self.tt.unknown;
        };
        let fields = self.variant_fields(variant);
        let gens = self.adt_generics(owner);
        let span = self.def(variant).span;
        let map = self.fresh_map(&gens, span);
        let args: Vec<Ty> = gens.iter().map(|g| map[g]).collect();
        let enum_ty = self.tt.mk(TyKind::Adt(owner, args));
        if let Exp::Is(want) = exp {
            self.tt.unify(enum_ty, want);
        }
        if fields.is_empty() {
            return enum_ty;
        }
        let params: Vec<ParamTy> = fields
            .iter()
            .map(|(_, t)| ParamTy {
                mode: None,
                ty: self.tt.subst(*t, &map),
            })
            .collect();
        self.tt.mk(TyKind::Fn(params, enum_ty))
    }

    /// `.circle` where the enum comes from the expected type.
    fn dot_name(&mut self, e: &'p Expr, name: &Ident, exp: Exp) -> Ty {
        let want = exp.ty().map(|t| self.peel_option(t));
        let Some(want) = want else {
            self.err(
                e.span,
                "cannot tell which enum this case belongs to; write `type.case` or give a type",
            );
            return self.tt.unknown;
        };
        let TyKind::Adt(def, _) = self.kind_of(want) else {
            if !self.tt.is_unknown(want) {
                let s = self.show(want);
                self.err(
                    e.span,
                    format!(
                        "`.{}` needs an enum, but `{s}` is expected here",
                        self.text(name)
                    ),
                );
            }
            return self.tt.unknown;
        };
        let n = self.text(name);
        match self.res.member(def, n) {
            Some(v) if self.def(v).kind == DefKind::Variant => self.variant_value(v, Exp::Is(want)),
            _ => {
                let tn = self.def(def).name.clone();
                self.err(name.span, format!("`{tn}` has no case `{n}`"));
                self.tt.unknown
            }
        }
    }

    // ---- Collections ------------------------------------------------------------------------------

    fn tuple_expr(&mut self, items: &'p [Expr], exp: Exp) -> Ty {
        if items.is_empty() {
            return self.tt.void;
        }
        let want: Option<Vec<Ty>> = match exp.ty().map(|t| self.kind_of(t)) {
            Some(TyKind::Tuple(ts)) if ts.len() == items.len() => Some(ts),
            _ => None,
        };
        let tys: Vec<Ty> = items
            .iter()
            .enumerate()
            .map(|(i, x)| match &want {
                Some(w) => {
                    let t = self.check_expr(x, Exp::Is(w[i]));
                    if self.try_coerce(t, w[i]) {
                        w[i]
                    } else {
                        t
                    }
                }
                None => self.check_expr(x, Exp::Any),
            })
            .collect();
        self.tt.tuple(tys)
    }

    /// The element type a list-like literal should have, from the expected type.
    fn expected_elem(&self, exp: Exp) -> Option<Ty> {
        let t = exp.ty()?;
        match self.tt.kind_of(self.peel_option(t)) {
            TyKind::List(e) | TyKind::Array(e, _) | TyKind::Set(e) | TyKind::Iter(e) => Some(*e),
            _ => None,
        }
    }

    fn elements(&mut self, items: &'p [ListItem], want: Option<Ty>, span: Span) -> Ty {
        let elem = match want {
            Some(w) => w,
            None => self.var(span),
        };
        for it in items {
            if it.spread {
                let t = self.check_expr(&it.value, Exp::Any);
                match self.kind_of(t) {
                    TyKind::List(e)
                    | TyKind::Array(e, _)
                    | TyKind::Set(e)
                    | TyKind::Iter(e)
                    | TyKind::Range(e) => {
                        self.coerce_at(e, elem, it.value.span, "the spread items");
                    }
                    TyKind::Unknown => {}
                    _ => {
                        let s = self.show(t);
                        self.err(
                            it.value.span,
                            format!("`...` needs a list, but this is `{s}`"),
                        );
                    }
                }
            } else {
                let t = self.check_expr(&it.value, Exp::Is(elem));
                self.coerce_at(t, elem, it.value.span, "this item");
            }
        }
        elem
    }

    fn list_expr(&mut self, e: &'p Expr, items: &'p [ListItem], exp: Exp) -> Ty {
        let want = self.expected_elem(exp);
        let elem = self.elements(items, want, e.span);
        // A literal where a fixed array is expected is that array.
        if let Some(t) = exp.ty() {
            if let TyKind::Array(_, len) = self.kind_of(self.peel_option(t)) {
                if !items.iter().any(|i| i.spread) {
                    let n = items.len() as u64;
                    let ok = match len {
                        ArrayLen::Known(k) => k == n,
                        _ => true,
                    };
                    if !ok {
                        self.err(
                            e.span,
                            format!(
                                "this array has {n} items, but {} are expected",
                                match len {
                                    ArrayLen::Known(k) => k,
                                    _ => n,
                                }
                            ),
                        );
                    }
                    return self.tt.mk(TyKind::Array(elem, ArrayLen::Known(n)));
                }
            }
        }
        self.tt.list(elem)
    }

    fn set_expr(&mut self, e: &'p Expr, items: &'p [ListItem], exp: Exp) -> Ty {
        let want = self.expected_elem(exp);
        let elem = self.elements(items, want, e.span);
        self.tt.mk(TyKind::Set(elem))
    }

    fn repeat_expr(&mut self, value: &'p Expr, count: &'p Expr, exp: Exp) -> Ty {
        let want = self.expected_elem(exp);
        let t = match want {
            Some(w) => self.check_coerce(value, w),
            None => self.check_expr(value, Exp::Any),
        };
        self.check_coerce(count, self.tt.int);
        let len = match &count.kind {
            ExprKind::Int => {
                let text = &self.program.module(self.file).src
                    [count.span.start as usize..count.span.end as usize];
                literal_value(text).map_or(ArrayLen::Unknown, |v| ArrayLen::Known(v as u64))
            }
            _ => ArrayLen::Unknown,
        };
        self.tt.mk(TyKind::Array(t, len))
    }

    fn map_expr(&mut self, e: &'p Expr, items: &'p [MapItem], exp: Exp) -> Ty {
        let (wk, wv) = match exp.ty().map(|t| self.kind_of(self.peel_option(t))) {
            Some(TyKind::Map(k, v)) => (Some(k), Some(v)),
            _ => (None, None),
        };
        let k = match wk {
            Some(k) => k,
            None => self.var(e.span),
        };
        let v = match wv {
            Some(v) => v,
            None => self.var(e.span),
        };
        for it in items {
            match it {
                MapItem::Pair(key, val) => {
                    let kt = self.check_expr(key, Exp::Is(k));
                    self.coerce_at(kt, k, key.span, "this key");
                    let vt = self.check_expr(val, Exp::Is(v));
                    self.coerce_at(vt, v, val.span, "this value");
                }
                MapItem::Spread(x) => {
                    let t = self.check_expr(x, Exp::Any);
                    let want = self.tt.mk(TyKind::Map(k, v));
                    self.coerce_at(t, want, x.span, "the spread map");
                }
            }
        }
        self.tt.mk(TyKind::Map(k, v))
    }

    /// The types bound by the patterns of `for pats of/in source`.
    pub(crate) fn iteration_types(
        &mut self,
        source: Ty,
        pats: usize,
        over_keys: bool,
        span: Span,
    ) -> Vec<Ty> {
        let k = self.kind_of(source);
        let unknown = vec![self.tt.unknown; pats.max(1)];
        let one = |c: &mut Self, e: Ty| -> Vec<Ty> {
            match pats {
                1 => vec![e],
                2 => vec![c.tt.int, e],
                _ => vec![e; pats],
            }
        };
        if matches!(k, TyKind::Adt(..)) {
            if let Some(m) = self.lookup_method(source, "iter", span) {
                let r = self.tt.subst(m.sig.ret, &m.map);
                if let TyKind::Iter(e) = self.kind_of(r) {
                    self.note_call(span, m.def, &m.map);
                    return one(self, e);
                }
            }
        }
        match k {
            TyKind::List(e)
            | TyKind::Array(e, _)
            | TyKind::Iter(e)
            | TyKind::Set(e)
            | TyKind::Range(e) => one(self, e),
            TyKind::Map(key, val) => {
                if over_keys {
                    vec![key]
                } else {
                    match pats {
                        1 => vec![self.tt.tuple(vec![key, val])],
                        _ => vec![key, val],
                    }
                }
            }
            TyKind::Str | TyKind::String => {
                self.err(
                    span,
                    "text is not a list; use `.chars()` to loop over characters",
                );
                unknown
            }
            _ if self.open_var(source) || self.tt.is_unknown(source) => unknown,
            _ => {
                let s = self.show(source);
                self.err(span, format!("cannot loop over `{s}`"));
                unknown
            }
        }
    }

    fn builder_expr(&mut self, e: &'p Expr, b: &'p Builder, exp: Exp) -> Ty {
        let mut first_elem: Option<Ty> = None;
        for c in &b.clauses {
            match c {
                Clause::Of { pats, source } => {
                    let st = self.check_expr(source, Exp::Any);
                    let tys = self.iteration_types(st, pats.len(), false, source.span);
                    for (p, t) in pats.iter().zip(tys.iter()) {
                        self.check_irrefutable(p, *t);
                    }
                    if first_elem.is_none() {
                        first_elem = tys.last().copied();
                    }
                }
                Clause::If(cond) => {
                    self.check_coerce(cond, self.tt.bool_);
                }
            }
        }
        let want_elem = self.expected_elem(exp);
        let item = match &b.result {
            Some(r) => match want_elem {
                Some(w) if b.kind != BuilderKind::Map => self.check_coerce(r, w),
                _ => self.check_expr(r, Exp::Any),
            },
            None => first_elem.unwrap_or(self.tt.unknown),
        };
        match b.kind {
            BuilderKind::List => self.tt.list(item),
            BuilderKind::Set => self.tt.mk(TyKind::Set(item)),
            BuilderKind::Stream => self.tt.mk(TyKind::Iter(item)),
            BuilderKind::Map => {
                let v = match &b.value {
                    Some(v) => self.check_expr(v, Exp::Any),
                    None => self.var(e.span),
                };
                self.tt.mk(TyKind::Map(item, v))
            }
        }
    }

    // ---- Field access and indexing ---------------------------------------------------------------------

    fn tuple_index(&mut self, e: &'p Expr, base: &'p Expr, index: Span) -> Ty {
        let bt = self.check_expr(base, Exp::Any);
        let text = &self.program.module(self.file).src[index.start as usize..index.end as usize];
        let i: usize = text.parse().unwrap_or(usize::MAX);
        match self.kind_of(bt) {
            TyKind::Tuple(ts) => match ts.get(i) {
                Some(t) => *t,
                None => {
                    self.err(
                        e.span,
                        format!(
                            "this tuple has {} items; there is no position {i}",
                            ts.len()
                        ),
                    );
                    self.tt.unknown
                }
            },
            TyKind::AltTuple(ts, _) => ts
                .get(i % ts.len().max(1))
                .copied()
                .unwrap_or(self.tt.unknown),
            TyKind::Unknown | TyKind::Var(_) => self.tt.unknown,
            _ => {
                let s = self.show(bt);
                self.err(e.span, format!("`{s}` is not a tuple"));
                self.tt.unknown
            }
        }
    }

    fn index_expr(&mut self, e: &'p Expr, base: &'p Expr, index: &'p Expr) -> Ty {
        let bt = self.check_expr(base, Exp::Any);
        let bt = self.peel_boxes(bt);
        let k = self.kind_of(bt);
        match k {
            TyKind::List(elem) | TyKind::Array(elem, _) => {
                if matches!(index.kind, ExprKind::Range { .. }) {
                    let r = self.int_range_ty();
                    self.check_expr(index, Exp::Is(r));
                    return self.tt.list(elem);
                }
                self.check_coerce(index, self.tt.int);
                elem
            }
            TyKind::Map(key, val) => {
                self.check_coerce(index, key);
                val
            }
            TyKind::String | TyKind::Str => {
                if matches!(index.kind, ExprKind::Range { .. }) {
                    let r = self.int_range_ty();
                    self.check_expr(index, Exp::Is(r));
                    return self.tt.str_;
                }
                self.check_expr(index, Exp::Any);
                self.err(
                    e.span,
                    "text cannot be indexed by position; use a range like `s[0..3]` or `.chars()`",
                );
                self.tt.unknown
            }
            TyKind::Unknown | TyKind::Var(_) => {
                self.check_expr(index, Exp::Any);
                self.tt.unknown
            }
            _ => {
                self.check_expr(index, Exp::Any);
                let s = self.show(bt);
                self.err(e.span, format!("`{s}` cannot be indexed"));
                self.tt.unknown
            }
        }
    }

    fn range_expr(
        &mut self,
        e: &'p Expr,
        start: &'p Option<Box<Expr>>,
        end: &'p Option<Box<Expr>>,
        step: &'p Option<Box<Expr>>,
        exp: Exp,
    ) -> Ty {
        let want = match exp.ty().map(|t| self.kind_of(t)) {
            Some(TyKind::Range(t)) => Some(t),
            _ => None,
        };
        let operand = match want {
            Some(w) => Exp::Is(w),
            None => Exp::Any,
        };
        let mut elem: Option<Ty> = None;
        for part in [start, end].into_iter().flatten() {
            let t = self.check_expr(part, operand);
            match elem {
                None => elem = Some(t),
                Some(prev) => {
                    if !self.try_coerce(t, prev) {
                        let (a, b) = (self.show(prev), self.show(t));
                        self.err(
                            part.span,
                            format!(
                                "the two ends of a range must have the same type: `{a}` and `{b}`"
                            ),
                        );
                    }
                }
            }
        }
        let elem = match elem {
            Some(t) => t,
            None => want.unwrap_or_else(|| self.tt.fresh(VarKind::Int, e.span)),
        };
        if let Some(s) = step {
            self.check_coerce(s, elem);
        }
        self.tt.mk(TyKind::Range(elem))
    }

    // ---- Operators -------------------------------------------------------------------------------------------

    fn unary(&mut self, e: &'p Expr, op: Punct, operand: &'p Expr, exp: Exp) -> Ty {
        match op {
            Punct::Minus => {
                let t = if matches!(operand.kind, ExprKind::Int) {
                    let ex = self.operand_exp(exp);
                    self.int_literal(operand, ex, true)
                } else {
                    let ex = self.operand_exp(exp);
                    self.check_expr(operand, ex)
                };
                let k = self.kind_of(t);
                let ok = match k {
                    TyKind::Int(ik) => ik.signed(),
                    TyKind::Float(_) | TyKind::Unknown => true,
                    TyKind::Var(_) => self.is_numeric(t),
                    _ => false,
                };
                if matches!(k, TyKind::Adt(..) | TyKind::Param(_) | TyKind::Any(_)) {
                    return match self.lookup_method(t, "neg", e.span) {
                        Some(m) => {
                            self.note_call(e.span, m.def, &m.map);
                            self.tt.subst(m.sig.ret, &m.map)
                        }
                        None => {
                            let s = self.show(t);
                            self.err(
                                e.span,
                                format!("cannot negate `{s}`: it does not provide `neg`; write `impl neg for {s}`"),
                            );
                            self.tt.unknown
                        }
                    };
                }
                if !ok {
                    let s = self.show(t);
                    self.err(e.span, format!("cannot negate `{s}`"));
                }
                t
            }
            Punct::Bang => {
                self.check_coerce(operand, self.tt.bool_);
                self.tt.bool_
            }
            _ => {
                let t = self.check_expr(operand, self.operand_exp(exp));
                if !self.is_integer(t) {
                    let s = self.show(t);
                    self.err(e.span, format!("`~` needs an integer, not `{s}`"));
                }
                t
            }
        }
    }

    fn describe_op(op: Punct) -> &'static str {
        op.as_str()
    }

    fn binary(&mut self, e: &'p Expr, op: Punct, lhs: &'p Expr, rhs: &'p Expr, exp: Exp) -> Ty {
        match op {
            Punct::AndAnd | Punct::OrOr => {
                self.check_coerce(lhs, self.tt.bool_);
                self.check_coerce(rhs, self.tt.bool_);
                self.tt.bool_
            }
            Punct::EqEq | Punct::NotEq | Punct::Lt | Punct::Gt | Punct::LtEq | Punct::GtEq => {
                self.comparison(e, op, lhs, rhs)
            }
            Punct::QuestionQuestion => {
                let lt = self.check_expr(lhs, Exp::Any);
                let inner = match self.kind_of(lt) {
                    TyKind::Option(i) => i,
                    TyKind::Unknown => self.tt.unknown,
                    _ => {
                        let s = self.show(lt);
                        self.err(
                            lhs.span,
                            format!("`??` needs an option on the left, but this is `{s}`"),
                        );
                        self.tt.unknown
                    }
                };
                let rt = self.check_expr(rhs, Exp::Is(inner));
                if self.try_coerce(rt, inner) {
                    inner
                } else {
                    // `a ?? b` where `b` is an option too.
                    let opt = self.tt.option(inner);
                    self.coerce_at(rt, opt, rhs.span, "the default");
                    opt
                }
            }
            Punct::PipeGt => self.pipe(e, lhs, rhs),
            Punct::Shl | Punct::Shr => {
                let lt = self.check_expr(lhs, self.operand_exp(exp));
                let rt = self.check_expr(rhs, Exp::Any);
                if !self.is_integer(lt) || !self.is_integer(rt) {
                    self.err(e.span, "shifts work on integers");
                }
                lt
            }
            Punct::Plus
            | Punct::Minus
            | Punct::Star
            | Punct::Slash
            | Punct::Percent
            | Punct::StarStar
            | Punct::PlusPercent
            | Punct::MinusPercent
            | Punct::StarPercent
            | Punct::Amp
            | Punct::Pipe
            | Punct::Caret => self.arith(e, op, lhs, rhs, exp),
            _ => self.tt.unknown,
        }
    }

    fn arith(&mut self, e: &'p Expr, op: Punct, lhs: &'p Expr, rhs: &'p Expr, exp: Exp) -> Ty {
        let operand = self.operand_exp(exp);
        let lt = self.check_expr(lhs, operand);
        if matches!(
            self.kind_of(lt),
            TyKind::Adt(..) | TyKind::Param(_) | TyKind::Any(_)
        ) {
            return self.user_binary(e, op, lt, rhs);
        }
        // Text: `+` joins.
        if op == Punct::Plus && matches!(self.kind_of(lt), TyKind::String | TyKind::Str) {
            let rt = self.check_expr(rhs, Exp::Is(self.tt.str_));
            if !self.try_coerce(rt, self.tt.str_) {
                let s = self.show(rt);
                self.err(
                    rhs.span,
                    format!("`+` joins text with text, but this is `{s}`"),
                );
            }
            return self.tt.string;
        }
        let rt = self.check_expr(rhs, Exp::Is(lt));
        if !self.tt.unify(lt, rt) {
            let (a, b) = (self.show(lt), self.show(rt));
            if !self.tt.is_unknown(lt) && !self.tt.is_unknown(rt) {
                let hint = self.mismatch_hint(rt, lt);
                self.err(
                    e.span,
                    format!(
                        "cannot use `{}` on `{a}` and `{b}`{hint}",
                        Self::describe_op(op)
                    ),
                );
            }
            return lt;
        }
        let bitwise = matches!(op, Punct::Amp | Punct::Pipe | Punct::Caret);
        let wrapping = matches!(
            op,
            Punct::PlusPercent | Punct::MinusPercent | Punct::StarPercent
        );
        let ok = if bitwise || wrapping {
            self.is_integer(lt)
        } else {
            self.is_numeric(lt)
        };
        let skip = self.tt.is_unknown(lt)
            || self.open_var(lt)
            || matches!(self.kind_of(lt), TyKind::Adt(..) | TyKind::Param(_));
        if !ok && !skip {
            let s = self.show(lt);
            let what = if bitwise || wrapping {
                "needs integers"
            } else {
                "needs numbers"
            };
            self.err(
                e.span,
                format!("`{}` {what}, but this is `{s}`", Self::describe_op(op)),
            );
        }
        lt
    }

    fn comparison(&mut self, e: &'p Expr, op: Punct, lhs: &'p Expr, rhs: &'p Expr) -> Ty {
        let lt = self.check_expr(lhs, Exp::Any);
        let rt = self.check_expr(rhs, Exp::Is(lt));
        let text = |c: &Self, t: Ty| matches!(c.kind_of(t), TyKind::String | TyKind::Str);
        if !(text(self, lt) && text(self, rt))
            && !self.try_coerce(rt, lt)
            && !self.try_coerce(lt, rt)
        {
            if !self.tt.is_unknown(lt) && !self.tt.is_unknown(rt) {
                let (a, b) = (self.show(lt), self.show(rt));
                let hint = self.mismatch_hint(rt, lt);
                self.err(e.span, format!("cannot compare `{a}` with `{b}`{hint}"));
            }
            return self.tt.bool_;
        }
        let ordering = !matches!(op, Punct::EqEq | Punct::NotEq);
        self.settle_literal(lt);
        let k = self.kind_of(lt);
        let against_none = matches!(rhs.kind, ExprKind::None) || matches!(lhs.kind, ExprKind::None);
        if !against_none
            && matches!(
                k,
                TyKind::Adt(..)
                    | TyKind::Param(_)
                    | TyKind::Any(_)
                    | TyKind::List(_)
                    | TyKind::Array(..)
                    | TyKind::Tuple(_)
                    | TyKind::Option(_)
                    | TyKind::Result(..)
                    | TyKind::Map(..)
                    | TyKind::Set(_)
                    | TyKind::Box(_)
            )
        {
            let need = if ordering { "order" } else { "eq" };
            if let Some(i) = self.core_iface(need) {
                if !self.provides(lt, i) {
                    let s = self.show(lt);
                    let hint = self.provide_hint(lt, i);
                    let what = if ordering { "ordered" } else { "compared" };
                    self.err(
                        e.span,
                        format!(
                            "`{s}` cannot be {what} with `{}`: it does not provide `{need}`{hint}",
                            op.as_str()
                        ),
                    );
                    return self.tt.bool_;
                }
            }
        }
        if matches!(k, TyKind::Fn(..)) {
            self.err(e.span, "functions cannot be compared");
        } else if ordering
            && !matches!(
                k,
                TyKind::Int(_)
                    | TyKind::Float(_)
                    | TyKind::Char
                    | TyKind::String
                    | TyKind::Str
                    | TyKind::Var(_)
                    | TyKind::Unknown
                    | TyKind::Adt(..)
                    | TyKind::Param(_)
                    | TyKind::List(_)
                    | TyKind::Tuple(_)
                    | TyKind::Box(_)
            )
        {
            let s = self.show(lt);
            self.err(
                e.span,
                format!("`{s}` cannot be ordered with `{}`", op.as_str()),
            );
        }
        self.tt.bool_
    }

    fn in_expr(&mut self, lhs: &'p Expr, rhs: &'p Expr) -> Ty {
        let rt = self.check_expr(rhs, Exp::Any);
        let lt = self.check_expr(lhs, Exp::Any);
        let elem = match self.kind_of(rt) {
            TyKind::Range(e)
            | TyKind::List(e)
            | TyKind::Array(e, _)
            | TyKind::Set(e)
            | TyKind::Iter(e) => Some(e),
            TyKind::Map(k, _) => Some(k),
            TyKind::Str | TyKind::String => {
                self.coerce_at(lt, self.tt.str_, lhs.span, "the part");
                None
            }
            TyKind::Unknown | TyKind::Var(_) => None,
            _ => {
                let s = self.show(rt);
                self.err(
                    rhs.span,
                    format!("`in` needs a range, list, set or map, not `{s}`"),
                );
                None
            }
        };
        if let Some(e) = elem {
            self.coerce_at(lt, e, lhs.span, "the value");
        }
        self.tt.bool_
    }

    fn pipe(&mut self, e: &'p Expr, lhs: &'p Expr, rhs: &'p Expr) -> Ty {
        let lt = self.check_expr(lhs, Exp::Any);
        let ft = self.check_expr(rhs, Exp::Any);
        match self.kind_of(ft) {
            TyKind::Fn(params, ret) if params.len() == 1 => {
                self.coerce_at(lt, params[0].ty, lhs.span, "the piped value");
                ret
            }
            TyKind::Fn(params, _) => {
                self.err(
                    e.span,
                    format!(
                        "`|>` needs a function with one parameter, but this takes {}",
                        params.len()
                    ),
                );
                self.tt.unknown
            }
            TyKind::Unknown | TyKind::Var(_) => self.tt.unknown,
            _ => {
                let s = self.show(ft);
                self.err(
                    rhs.span,
                    format!("`|>` needs a function on the right, not `{s}`"),
                );
                self.tt.unknown
            }
        }
    }

    fn assign(&mut self, e: &'p Expr, op: Punct, lhs: &'p Expr, rhs: &'p Expr) -> Ty {
        // `_ = value` throws the value away.
        if matches!(lhs.kind, ExprKind::Underscore) {
            self.check_expr(rhs, Exp::Any);
            return self.tt.void;
        }
        let lt = self.check_expr(lhs, Exp::Any);
        match op {
            Punct::Eq => {
                self.check_coerce(rhs, lt);
            }
            Punct::QuestionQuestionEq => {
                let inner = match self.kind_of(lt) {
                    TyKind::Option(i) => i,
                    TyKind::Unknown => self.tt.unknown,
                    _ => {
                        let s = self.show(lt);
                        self.err(
                            lhs.span,
                            format!("`??=` needs an option on the left, but this is `{s}`"),
                        );
                        self.tt.unknown
                    }
                };
                self.check_coerce(rhs, inner);
            }
            _ => {
                // `a += b` is `a = a + b`.
                let base = match op {
                    Punct::PlusEq => Punct::Plus,
                    Punct::MinusEq => Punct::Minus,
                    Punct::StarEq => Punct::Star,
                    Punct::SlashEq => Punct::Slash,
                    Punct::PercentEq => Punct::Percent,
                    Punct::AmpEq => Punct::Amp,
                    Punct::PipeEq => Punct::Pipe,
                    Punct::CaretEq => Punct::Caret,
                    Punct::ShlEq => Punct::Shl,
                    Punct::ShrEq => Punct::Shr,
                    Punct::PlusPercentEq => Punct::PlusPercent,
                    Punct::MinusPercentEq => Punct::MinusPercent,
                    _ => Punct::StarPercent,
                };
                if matches!(
                    self.kind_of(lt),
                    TyKind::Adt(..) | TyKind::Param(_) | TyKind::Any(_)
                ) {
                    self.user_binary(e, base, lt, rhs);
                } else if base == Punct::Plus && matches!(self.kind_of(lt), TyKind::String) {
                    self.check_coerce(rhs, self.tt.str_);
                } else if matches!(base, Punct::Shl | Punct::Shr) {
                    let rt = self.check_expr(rhs, Exp::Any);
                    if !self.is_integer(rt) || !self.is_integer(lt) {
                        self.err(e.span, "shifts work on integers");
                    }
                } else {
                    let rt = self.check_expr(rhs, Exp::Is(lt));
                    if !self.tt.unify(lt, rt) && !self.tt.is_unknown(lt) && !self.tt.is_unknown(rt)
                    {
                        let (a, b) = (self.show(lt), self.show(rt));
                        let hint = self.mismatch_hint(rt, lt);
                        self.err(
                            e.span,
                            format!("cannot use `{}` on `{a}` and `{b}`{hint}", op.as_str()),
                        );
                    } else {
                        let bitwise = matches!(
                            base,
                            Punct::Amp
                                | Punct::Pipe
                                | Punct::Caret
                                | Punct::PlusPercent
                                | Punct::MinusPercent
                                | Punct::StarPercent
                        );
                        let ok = if bitwise {
                            self.is_integer(lt)
                        } else {
                            self.is_numeric(lt)
                        };
                        if !ok
                            && matches!(
                                self.kind_of(lt),
                                TyKind::Bool
                                    | TyKind::Char
                                    | TyKind::String
                                    | TyKind::Str
                                    | TyKind::List(_)
                                    | TyKind::Option(_)
                            )
                        {
                            let s = self.show(lt);
                            self.err(e.span, format!("`{}` does not work on `{s}`", op.as_str()));
                        }
                    }
                }
            }
        }
        self.tt.void
    }

    // ---- `?`, conversions ----------------------------------------------------------------------------------

    fn try_expr(&mut self, e: &'p Expr, inner: &'p Expr) -> Ty {
        let it = self.check_expr(inner, Exp::Any);
        let ret = self.fn_ctx.last().map(|c| c.ret).unwrap_or(self.tt.void);
        match self.kind_of(it) {
            TyKind::Result(ok, err) => {
                match self.kind_of(ret) {
                    TyKind::Result(_, ret_err) => {
                        // The error may widen into `error` (or, later, through `converts`).
                        let to_catch_all = matches!(self.kind_of(ret_err), TyKind::Error);
                        let direct = to_catch_all || self.tt.unify(err, ret_err);
                        let via = if direct {
                            None
                        } else {
                            self.converts_with(err, ret_err)
                        };
                        if let Some((m, map)) = &via {
                            self.note_call(e.span, *m, map);
                        }
                        if !direct && via.is_none() {
                            let (a, b) = (self.show(err), self.show(ret_err));
                            self.err(
                                e.span,
                                format!("`?` would return an error of type `{a}`, but this function returns `{b}`; write `impl converts<{b}> for {a}` or return `error`"),
                            );
                        }
                    }
                    TyKind::Unknown | TyKind::Var(_) => {}
                    _ => {
                        let s = self.show(ret);
                        self.err(e.span, format!("`?` passes an error upward, but this function returns `{s}`, not a result"));
                    }
                }
                ok
            }
            TyKind::Option(inner_ty) => {
                match self.kind_of(ret) {
                    TyKind::Option(_) | TyKind::Unknown | TyKind::Var(_) => {}
                    _ => {
                        let s = self.show(ret);
                        self.err(e.span, format!("`?` returns `none` upward, but this function returns `{s}`, not an option"));
                    }
                }
                inner_ty
            }
            _ if self.open_var(it) || self.tt.is_unknown(it) => self.tt.unknown,
            _ => {
                let s = self.show(it);
                self.err(
                    e.span,
                    format!("`?` needs a result or an option, but this is `{s}`"),
                );
                self.tt.unknown
            }
        }
    }

    /// 'i' for integers, 'f' for floats (including literal variables), 'o' for the rest.
    fn num_class(&self, t: Ty) -> char {
        match self.tt.kind_of(t) {
            TyKind::Int(_) => 'i',
            TyKind::Float(_) => 'f',
            TyKind::Var(_) => match self.tt.var_info(t) {
                Some((VarKind::Int, _)) => 'i',
                Some((VarKind::Float, _)) => 'f',
                _ => 'o',
            },
            _ => 'o',
        }
    }

    fn convert(&mut self, e: &'p Expr, value: &'p Expr, ty: &'p Type, checked: bool) -> Ty {
        let target = self.lower_type(ty);
        let vt = self.check_expr(value, Exp::Any);
        if checked {
            // `a to? dog`: a downcast, giving an option.
            if let (TyKind::Adt(from, fa), TyKind::Adt(to, ta)) =
                (self.kind_of(vt), self.kind_of(target))
            {
                if !self.is_subtype(to, &ta, from, &fa) && from != to {
                    let (a, b) = (self.show(vt), self.show(target));
                    self.err(
                        e.span,
                        format!("`{b}` is not a kind of `{a}`, so this can never succeed"),
                    );
                }
            }
            return self.tt.option(target);
        }
        if self.tt.unify(vt, target) {
            return target;
        }
        let ok = match (self.kind_of(vt), self.kind_of(target)) {
            (TyKind::Int(a), TyKind::Int(b)) => {
                (a.signed() == b.signed() && b.bits() >= a.bits())
                    || (!a.signed() && b.signed() && b.bits() > a.bits())
            }
            (TyKind::Var(_), TyKind::Int(_)) => self.is_integer(vt),
            (TyKind::Var(_), TyKind::Float(_)) => self.is_numeric(vt),
            (TyKind::Int(a), TyKind::Float(_)) => a.bits() <= 64,
            (TyKind::Float(FloatKind::F32), TyKind::Float(FloatKind::F64)) => true,
            (TyKind::Char, TyKind::Int(b)) => b.bits() >= 32,
            (TyKind::Bool, TyKind::Int(_)) => true,
            (TyKind::Str, TyKind::String) | (TyKind::String, TyKind::Str) => true,
            (_, TyKind::String) => true, // anything printable becomes text
            (TyKind::Adt(..), TyKind::Adt(..)) | (TyKind::Adt(..), TyKind::Any(_)) => {
                self.try_coerce(vt, target)
            }
            (TyKind::Unknown, _) | (_, TyKind::Unknown) => true,
            _ => false,
        };
        if !ok {
            if let Some((m, map)) = self.converts_with(vt, target) {
                self.note_call(e.span, m, &map);
                return target;
            }
        }
        if !ok {
            let (a, b) = (self.show(vt), self.show(target));
            let vk = self.num_class(vt);
            let tk = self.num_class(target);
            let lossy = matches!((vk, tk), ('f', 'i') | ('i', 'i'))
                || (vk == 'i' && matches!(self.kind_of(target), TyKind::Char));
            let hint = if lossy {
                "; this can lose information, so `to` does not allow it"
            } else {
                ""
            };
            self.err(
                e.span,
                format!("cannot convert `{a}` to `{b}` with `to`{hint}"),
            );
        }
        target
    }
}
