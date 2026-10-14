//! Interfaces: bounds on type parameters, `impl` completeness, the orphan rule, and what a type
//! provides (`eq`, `order`, operators, ...).
//!
//! A type **provides** an interface when one of these holds:
//!
//! - it lists the interface after `:` (`struct circle : shape_like`), or an `impl iface for type`
//!   exists, or `@derive(iface)` is written on it;
//! - it is a built-in type for which the library says so (`int` provides `eq`, `order`, `hash`,
//!   `show`, `add`, ...; a list provides `eq` when its elements do);
//! - it is a plain `struct` or `enum` whose fields provide `eq` or `clone` (T5: those two come for
//!   free; `order`, `hash` and `show` are opt-in; an `obj` needs all of them written);
//! - it is a type parameter whose bounds include the interface, or an `any` of an interface that
//!   builds on it.

use super::*;
use crate::token::Punct;

/// A bound that is checked once the types in it are known.
pub(crate) struct PendingBound {
    pub file: ModuleId,
    pub span: Span,
    pub ty: Ty,
    pub iface: DefId,
    /// The type parameter that asked for it.
    pub param: DefId,
}

impl<'p> Checker<'p> {
    // ---- Indexing ---------------------------------------------------------------------------------

    /// Remembers the bounds written on type parameters (`T: ordered + hash`, `where T: eq`).
    pub(crate) fn register_generics(
        &mut self,
        file: ModuleId,
        g: &'p Generics,
        where_: &'p [WherePred],
    ) {
        for p in &g.params {
            if p.is_meta || p.bounds.is_empty() {
                continue;
            }
            if let Some(&d) = self.res.decls.get(&(file, p.name.span.start)) {
                self.param_bounds
                    .entry(d)
                    .or_default()
                    .extend(p.bounds.iter());
            }
        }
        for w in where_ {
            if let TypeKind::Path { path, .. } = &w.ty.kind {
                let d = path
                    .segments
                    .last()
                    .and_then(|l| self.res.use_at(file, l.span.start));
                if let Some(d) = d.filter(|d| self.def(*d).kind == DefKind::GenericParam) {
                    self.param_bounds
                        .entry(d)
                        .or_default()
                        .extend(w.bounds.iter());
                }
            }
        }
    }

    pub(crate) fn register_derives(&mut self, file: ModuleId, def: DefId, attrs: &'p [Attr]) {
        for a in attrs {
            if self.text_in(file, &a.name) != "derive" {
                continue;
            }
            let src = &self.program.module(file).src;
            for arg in &a.args {
                let t = src[arg.span.start as usize..arg.span.end as usize].trim();
                self.derives.entry(def).or_default().push(t.to_string());
            }
        }
    }

    // ---- Interfaces as data -------------------------------------------------------------------------

    pub(crate) fn iface_of_type(&self, file: ModuleId, t: &Type) -> Option<DefId> {
        let TypeKind::Path { path, .. } = &t.kind else {
            return None;
        };
        let d = self.res.use_at(file, path.segments.last()?.span.start)?;
        (self.def(d).kind == DefKind::Interface).then_some(d)
    }

    /// The interfaces listed directly after `:` on a type or an interface.
    pub(crate) fn ifaces_of(&self, def: DefId) -> Vec<DefId> {
        let Some(Node::Adt { file, decl, .. }) = self.nodes.get(&def).copied() else {
            return Vec::new();
        };
        let bases = match decl {
            AdtDecl::Type(d) => &d.bases,
            AdtDecl::Enum(e) => &e.bases,
        };
        bases
            .iter()
            .filter_map(|b| self.iface_of_type(file, b))
            .collect()
    }

    /// The interface and every interface it builds on.
    pub(crate) fn iface_closure(&self, i: DefId) -> Vec<DefId> {
        let mut out = vec![i];
        let mut k = 0;
        while k < out.len() {
            for s in self.ifaces_of(out[k]) {
                if !out.contains(&s) {
                    out.push(s);
                }
            }
            k += 1;
        }
        out
    }

    fn close_all(&self, direct: impl IntoIterator<Item = DefId>) -> Vec<DefId> {
        let mut out: Vec<DefId> = Vec::new();
        for d in direct {
            for c in self.iface_closure(d) {
                if !out.contains(&c) {
                    out.push(c);
                }
            }
        }
        out
    }

    /// The interfaces written as bounds of a parameter, without the ones they build on.
    pub(crate) fn direct_bounds(&self, g: DefId) -> Vec<DefId> {
        let Some(bs) = self.param_bounds.get(&g) else {
            return Vec::new();
        };
        let file = self.def(g).file.unwrap_or(ModuleId(0));
        bs.iter()
            .filter_map(|t| self.iface_of_type(file, t))
            .collect()
    }

    /// Every interface a type parameter is known to provide. Inside an interface, `Self` is a
    /// parameter named by the interface itself.
    pub(crate) fn bounds_of(&self, g: DefId) -> Vec<DefId> {
        if self.def(g).kind == DefKind::Interface {
            return self.iface_closure(g);
        }
        self.close_all(self.direct_bounds(g))
    }

    /// A library interface by name (`eq`, `order`, `add`, ...).
    pub(crate) fn core_iface(&mut self, name: &str) -> Option<DefId> {
        if let Some(d) = self.core_ifaces.get(name) {
            return Some(*d);
        }
        let found = (0..self.res.defs.len() as u32).map(DefId).find(|d| {
            let def = self.def(*d);
            def.kind == DefKind::Interface && def.name == name && self.is_core_def(*d)
        })?;
        self.core_ifaces.insert(name.to_string(), found);
        Some(found)
    }

    pub(crate) fn is_core_def(&self, d: DefId) -> bool {
        self.def(d)
            .file
            .map(|f| self.program.package(self.program.module(f).package).name == "core")
            .unwrap_or(false)
    }

    // ---- What a type provides ---------------------------------------------------------------------

    pub(crate) fn provides(&mut self, t: Ty, iface: DefId) -> bool {
        self.provides_d(t, iface, 0)
    }

    fn provides_d(&mut self, t: Ty, iface: DefId, depth: u32) -> bool {
        if depth > 8 {
            return true;
        }
        let t = self.tt.shallow(t);
        match self.kind_of(t) {
            TyKind::Unknown | TyKind::Var(_) | TyKind::Never | TyKind::Error => true,
            TyKind::Param(g) => self.bounds_of(g).contains(&iface),
            TyKind::Any(i) => self.iface_closure(i).contains(&iface),
            TyKind::Adt(d, args) => self.adt_provides(t, d, &args, iface, depth),
            _ => self.builtin_provides(t, iface, depth) || self.impl_provides(t, iface, depth),
        }
    }

    fn builtin_provides(&mut self, t: Ty, iface: DefId, depth: u32) -> bool {
        if !self.is_core_def(iface) {
            return false;
        }
        let name = self.def(iface).name.clone();
        let name = name.as_str();
        let elems: Vec<Ty> = match self.kind_of(t) {
            TyKind::Int(k) => {
                return match name {
                    "eq" | "order" | "hash" | "show" | "clone" | "add" | "sub" | "mul" | "div"
                    | "rem" => true,
                    "neg" => k.signed(),
                    _ => false,
                }
            }
            TyKind::Float(_) => {
                return matches!(
                    name,
                    "eq" | "order"
                        | "show"
                        | "clone"
                        | "add"
                        | "sub"
                        | "mul"
                        | "div"
                        | "rem"
                        | "neg"
                )
            }
            TyKind::Bool | TyKind::Char | TyKind::String | TyKind::Str => {
                return matches!(name, "eq" | "order" | "hash" | "show" | "clone")
            }
            TyKind::Void => return matches!(name, "eq" | "show" | "clone"),
            TyKind::Fn(..) | TyKind::Iter(_) => return false,
            TyKind::List(e)
            | TyKind::Array(e, _)
            | TyKind::Option(e)
            | TyKind::Box(e)
            | TyKind::Range(e) => vec![e],
            TyKind::Set(e) => {
                if matches!(name, "order" | "hash") {
                    return false;
                }
                vec![e]
            }
            TyKind::Tuple(ts) | TyKind::AltTuple(ts, _) => ts,
            TyKind::Result(a, b) => vec![a, b],
            TyKind::Map(k, v) => {
                if matches!(name, "order" | "hash") {
                    return false;
                }
                vec![k, v]
            }
            _ => return false,
        };
        if !matches!(name, "eq" | "order" | "hash" | "show" | "clone") {
            return false;
        }
        elems
            .into_iter()
            .all(|e| self.provides_d(e, iface, depth + 1))
    }

    fn adt_provides(&mut self, t: Ty, d: DefId, args: &[Ty], iface: DefId, depth: u32) -> bool {
        // Listed after `:` (also through an `obj` base), or derived.
        if self.ifaces_of(d).contains(&iface) {
            return true;
        }
        let derived: Vec<DefId> = self
            .derives
            .get(&d)
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|n| self.core_iface(n))
            .collect();
        if derived.contains(&iface) {
            return true;
        }
        if self.impl_provides(t, iface, depth) {
            return true;
        }
        let map = self.adt_map_of(d, args);
        for b in self.bases_of(d) {
            let b = self.tt.subst(b, &map);
            if self.provides_d(b, iface, depth + 1) {
                return true;
            }
        }
        // Plain data gets `eq` and `clone` by itself.
        if self.is_core_def(iface) && matches!(self.def(iface).name.as_str(), "eq" | "clone") {
            if self.def(d).kind == DefKind::Obj {
                return false;
            }
            for ft in self.field_types_of(d, args) {
                if !self.provides_d(ft, iface, depth + 1) {
                    return false;
                }
            }
            return true;
        }
        false
    }

    pub(crate) fn adt_map_of(&mut self, d: DefId, args: &[Ty]) -> HashMap<DefId, Ty> {
        let gens = self.adt_generics(d);
        gens.iter().copied().zip(args.iter().copied()).collect()
    }

    /// The types of all fields of a struct, or of all payloads of an enum.
    fn field_types_of(&mut self, d: DefId, args: &[Ty]) -> Vec<Ty> {
        let map = self.adt_map_of(d, args);
        let mut out = Vec::new();
        if self.def(d).kind == DefKind::Enum {
            let members: Vec<DefId> = self.def(d).members.clone();
            for m in members {
                if self.def(m).kind == DefKind::Variant {
                    for (_, t) in self.variant_fields(m) {
                        out.push(self.tt.subst(t, &map));
                    }
                }
            }
        } else {
            for (_, t) in self.collect_fields(d, args) {
                out.push(t);
            }
        }
        out
    }

    fn impl_provides(&mut self, t: Ty, iface: DefId, depth: u32) -> bool {
        let Some(h) = self.head_of(t) else {
            return false;
        };
        let candidates = self.impl_by_head.get(&h).cloned().unwrap_or_default();
        for idx in candidates {
            let Some(i) = self.impls[idx].iface else {
                continue;
            };
            if i != iface {
                continue;
            }
            let span = self.def(iface).span;
            let gens = self.impls[idx].generics.clone();
            let map: HashMap<DefId, Ty> = gens.iter().map(|g| (*g, self.var(span))).collect();
            let inst = self.tt.subst(self.impls[idx].self_ty, &map);
            let mark = self.tt.snapshot();
            let mut ok = self.tt.unify(inst, t);
            if ok {
                for g in &gens {
                    for b in self.direct_bounds(*g) {
                        if !self.provides_d(map[g], b, depth + 1) {
                            ok = false;
                        }
                    }
                }
            }
            self.tt.rollback(mark);
            if ok {
                return true;
            }
        }
        false
    }

    // ---- Bounds at use ----------------------------------------------------------------------------------

    /// Queues the bounds of type parameters that now stand for `map[g]`.
    pub(crate) fn queue_bounds(&mut self, gens: &[DefId], tys: &[Ty], span: Span) {
        for (g, ty) in gens.iter().zip(tys.iter()) {
            for b in self.direct_bounds(*g) {
                self.pending_bounds.push(PendingBound {
                    file: self.file,
                    span,
                    ty: *ty,
                    iface: b,
                    param: *g,
                });
            }
        }
    }

    pub(crate) fn flush_bounds(&mut self) {
        let pending = std::mem::take(&mut self.pending_bounds);
        let mut seen: Vec<(ModuleId, u32, DefId)> = Vec::new();
        for p in pending {
            let key = (p.file, p.span.start, p.iface);
            if seen.contains(&key) || self.provides(p.ty, p.iface) {
                continue;
            }
            seen.push(key);
            let saved = self.file;
            self.file = p.file;
            let (ty, ifn, pn) = (
                self.show(p.ty),
                self.def(p.iface).name.clone(),
                self.def(p.param).name.clone(),
            );
            let hint = self.provide_hint(p.ty, p.iface);
            self.err(
                p.span,
                format!(
                    "`{ty}` does not provide `{ifn}`, which the type parameter `{pn}` needs{hint}"
                ),
            );
            self.file = saved;
        }
    }

    /// What to write so a type provides an interface.
    pub(crate) fn provide_hint(&mut self, t: Ty, iface: DefId) -> String {
        let tn = self.show(t);
        let n = self.def(iface).name.clone();
        match self.kind_of(t) {
            TyKind::Adt(..) => {
                if self.is_core_def(iface) && matches!(n.as_str(), "eq" | "order" | "hash" | "show")
                {
                    format!("; write `@derive({n})` on `{tn}` or `impl {n} for {tn}`")
                } else {
                    format!("; write `impl {n} for {tn}`")
                }
            }
            TyKind::Param(_) => format!("; add `: {n}` to the type parameter"),
            _ => String::new(),
        }
    }

    // ---- Method lookup through interfaces --------------------------------------------------------------

    /// The interfaces a type is known to provide, to look for their methods.
    fn known_ifaces(&mut self, r: Ty) -> Vec<DefId> {
        match self.kind_of(r) {
            TyKind::Param(g) => self.bounds_of(g),
            TyKind::Any(i) => self.iface_closure(i),
            TyKind::Adt(d, args) => {
                let mut out = self.close_all(self.ifaces_of(d));
                let map = self.adt_map_of(d, &args);
                for b in self.bases_of(d) {
                    let b = self.tt.subst(b, &map);
                    for i in self.known_ifaces(b) {
                        if !out.contains(&i) {
                            out.push(i);
                        }
                    }
                }
                for i in self.impl_ifaces(r) {
                    if !out.contains(&i) {
                        out.push(i);
                    }
                }
                out
            }
            _ => self.impl_ifaces(r),
        }
    }

    /// Interfaces that `impl iface for type` blocks give a type.
    fn impl_ifaces(&mut self, t: Ty) -> Vec<DefId> {
        let Some(h) = self.head_of(t) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for idx in self.impl_by_head.get(&h).cloned().unwrap_or_default() {
            let Some(i) = self.impls[idx].iface else {
                continue;
            };
            let gens = self.impls[idx].generics.clone();
            let span = self.def(i).span;
            let map: HashMap<DefId, Ty> = gens.iter().map(|g| (*g, self.var(span))).collect();
            let inst = self.tt.subst(self.impls[idx].self_ty, &map);
            let mark = self.tt.snapshot();
            let ok = self.tt.unify(inst, t);
            self.tt.rollback(mark);
            if ok {
                out.extend(self.iface_closure(i));
            }
        }
        out
    }

    /// A method found in an interface the receiver provides (a promised or a default method).
    pub(crate) fn iface_method(&mut self, r: Ty, name: &str) -> Option<call::Method> {
        for i in self.known_ifaces(r) {
            let Some(m) = self.res.member(i, name) else {
                continue;
            };
            if self.def(m).kind != DefKind::Fn {
                continue;
            }
            let sig = self.fn_sig(m)?;
            let mut map = HashMap::new();
            map.insert(i, r);
            return Some(call::Method { sig, map });
        }
        None
    }

    /// Which structs, enums and objs own something. An `obj` always does; the others do when a
    /// field or payload does (found by repeating until nothing changes).
    pub(crate) fn compute_owning(&mut self) -> std::collections::HashSet<DefId> {
        let mut adts: Vec<DefId> = self
            .nodes
            .iter()
            .filter_map(|(d, n)| matches!(n, Node::Adt { .. }).then_some(*d))
            .collect();
        adts.sort();
        let mut owning = std::collections::HashSet::new();
        for d in &adts {
            if self.def(*d).kind == DefKind::Obj {
                owning.insert(*d);
            }
        }
        loop {
            let mut changed = false;
            for d in &adts {
                if owning.contains(d) || self.def(*d).kind == DefKind::Interface {
                    continue;
                }
                let own = self.own_adt_ty(*d);
                let TyKind::Adt(_, args) = self.kind_of(own) else {
                    continue;
                };
                let fields = self.field_types_of(*d, &args);
                if fields.into_iter().any(|t| self.owns(t, &owning)) {
                    owning.insert(*d);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        owning
    }

    fn owns(&self, t: Ty, owning: &std::collections::HashSet<DefId>) -> bool {
        let t = self.tt.shallow(t);
        match self.tt.kind(t) {
            TyKind::String
            | TyKind::List(_)
            | TyKind::Map(..)
            | TyKind::Set(_)
            | TyKind::Box(_)
            | TyKind::Iter(_)
            | TyKind::Any(_) => true,
            TyKind::Array(e, _) | TyKind::Option(e) | TyKind::Range(e) => self.owns(*e, owning),
            TyKind::Tuple(ts) | TyKind::AltTuple(ts, _) => ts.iter().any(|x| self.owns(*x, owning)),
            TyKind::Result(a, b) => self.owns(*a, owning) || self.owns(*b, owning),
            TyKind::Adt(d, args) => {
                owning.contains(d) || args.iter().any(|x| self.owns(*x, owning))
            }
            _ => false,
        }
    }

    // ---- impl checks ------------------------------------------------------------------------------------

    /// Completeness of `impl iface for type` and of types that list interfaces, and the orphan rule.
    pub(crate) fn check_impls(&mut self) {
        for idx in 0..self.impls.len() {
            let (file, decl, self_ty) = {
                let im = &self.impls[idx];
                (im.file, im.decl, im.self_ty)
            };
            let saved = self.file;
            self.file = file;
            self.orphan_rule(idx);
            if let Some(i) = self.impls[idx].iface {
                self.conforms(self_ty, i, decl.target.span);
            }
            self.file = saved;
        }
        let mut adts: Vec<DefId> = self
            .nodes
            .iter()
            .filter_map(|(d, n)| matches!(n, Node::Adt { .. }).then_some(*d))
            .collect();
        adts.sort();
        for d in adts {
            if self.def(d).kind == DefKind::Interface {
                continue;
            }
            let Some(Node::Adt { file, .. }) = self.nodes.get(&d).copied() else {
                continue;
            };
            let ifaces = self.ifaces_of(d);
            if ifaces.is_empty() {
                continue;
            }
            let own = self.own_adt_ty(d);
            let saved = self.file;
            self.file = file;
            let span = self.def(d).span;
            for i in ifaces {
                self.conforms(own, i, span);
            }
            self.file = saved;
        }
    }

    fn package_of_file(&self, f: ModuleId) -> PackageId {
        self.program.module(f).package
    }

    fn orphan_rule(&mut self, idx: usize) {
        let (file, decl, self_ty, iface) = {
            let im = &self.impls[idx];
            (im.file, im.decl, im.self_ty, im.iface)
        };
        let here = self.package_of_file(file);
        if self.program.package(here).name == "core" {
            return;
        }
        let local = |c: &Self, d: DefId| c.def(d).file.map(|f| c.package_of_file(f)) == Some(here);
        let head_local = match self.kind_of(self_ty) {
            TyKind::Adt(d, _) => local(self, d),
            TyKind::Unknown => return,
            _ => false,
        };
        match iface {
            Some(i) => {
                if !local(self, i) && !head_local {
                    let (i, t) = (self.def(i).name.clone(), self.show(self_ty));
                    self.err(
                        decl.target.span,
                        format!("cannot write `impl {i} for {t}`: neither `{i}` nor `{t}` is defined in this package"),
                    );
                }
            }
            None => {
                if !head_local {
                    let t = self.show(self_ty);
                    self.err(
                        decl.target.span,
                        format!(
                            "cannot add methods to `{t}` here: it is not defined in this package"
                        ),
                    );
                }
            }
        }
    }

    /// Does the type have what the interface promises?
    fn conforms(&mut self, self_ty: Ty, iface: DefId, span: Span) {
        if self.tt.is_unknown(self_ty) {
            return;
        }
        let tn = self.show(self_ty);
        let inn = self.def(iface).name.clone();
        // Interfaces it builds on are separate promises.
        for s in self.iface_closure(iface) {
            if s != iface && !self.provides(self_ty, s) {
                let sn = self.def(s).name.clone();
                self.err(
                    span,
                    format!("`{inn}` builds on `{sn}`, so `{tn}` must provide `{sn}` too"),
                );
            }
        }
        let members: Vec<DefId> = self.def(iface).members.clone();
        let mut imap = HashMap::new();
        imap.insert(iface, self_ty);
        for m in members {
            if self.def(m).kind != DefKind::Fn {
                continue;
            }
            let Some(Node::Fn { f, .. }) = self.nodes.get(&m).copied() else {
                continue;
            };
            let name = self.def(m).name.clone();
            let promised = matches!(f.body, FnBody::None);
            let found = self.find_own_method(self_ty, &name, span);
            let Some(found) = found else {
                if promised {
                    self.err(
                        span,
                        format!("`{tn}` does not implement `{name}`, which `{inn}` promises"),
                    );
                }
                continue;
            };
            let Some(want) = self.fn_sig(m) else { continue };
            let mark = self.tt.snapshot();
            let mut ok = want.params.len() == found.sig.params.len()
                && want.has_self == found.sig.has_self
                && want.self_mode == found.sig.self_mode;
            if ok {
                for (a, b) in want.params.iter().zip(found.sig.params.iter()) {
                    let a = self.tt.subst(a.ty, &imap);
                    let b = self.tt.subst(b.ty, &found.map);
                    ok &= self.tt.unify(a, b);
                }
                let a = self.tt.subst(want.ret, &imap);
                let b = self.tt.subst(found.sig.ret, &found.map);
                ok &= self.tt.unify(a, b);
            }
            if !ok {
                let w = self.sig_text(&want, &imap);
                let g = self.sig_text(&found.sig, &found.map);
                self.err(
                    span,
                    format!("`{name}` of `{tn}` does not match `{inn}`: it promises `{w}`, but the type has `{g}`"),
                );
            }
            self.tt.rollback(mark);
        }
    }

    fn sig_text(&mut self, sig: &FnSig, map: &HashMap<DefId, Ty>) -> String {
        let mut parts = Vec::new();
        if sig.has_self {
            parts.push(match sig.self_mode {
                Some(m) => format!("{} self", m.as_str()),
                None => "self".to_string(),
            });
        }
        for p in &sig.params {
            let t = self.tt.subst(p.ty, map);
            let t = self.tt.zonk(t);
            parts.push(format!("{}: {}", p.name, self.show(t)));
        }
        let r = self.tt.subst(sig.ret, map);
        let r = self.tt.zonk(r);
        format!("fn({}) -> {}", parts.join(", "), self.show(r))
    }

    /// The interface that stands for an operator on user types.
    pub(crate) fn operator_iface(op: Punct) -> Option<&'static str> {
        Some(match op {
            Punct::Plus => "add",
            Punct::Minus => "sub",
            Punct::Star => "mul",
            Punct::Slash => "div",
            Punct::Percent => "rem",
            _ => return None,
        })
    }

    /// `a + b` where `a` is a user type or a type parameter: the method of the operator interface.
    pub(crate) fn user_binary(&mut self, e: &'p Expr, op: Punct, lt: Ty, rhs: &'p Expr) -> Ty {
        let tn = self.show(lt);
        let Some(name) = Self::operator_iface(op) else {
            self.check_expr(rhs, Exp::Any);
            self.err(
                e.span,
                format!("`{}` cannot be used on `{tn}`", op.as_str()),
            );
            return self.tt.unknown;
        };
        match self.lookup_method(lt, name, e.span) {
            Some(m) if m.sig.params.len() == 1 => {
                let other = self.tt.subst(m.sig.params[0].ty, &m.map);
                self.check_coerce(rhs, other);
                self.tt.subst(m.sig.ret, &m.map)
            }
            Some(_) => self.tt.unknown,
            None => {
                self.check_expr(rhs, Exp::Any);
                self.err(
                    e.span,
                    format!(
                        "`{}` does not work on `{tn}`: it does not provide `{name}`; write `impl {name} for {tn}`",
                        op.as_str()
                    ),
                );
                self.tt.unknown
            }
        }
    }
}
