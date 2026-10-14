//! Maps and sets: literals and the glue (drop, copy, equality, printing). The table itself is written in
//! the language (`rt_map_*` in the library); this file only calls it.

use super::*;

impl<'a> Lowerer<'a> {
    fn collection_args(&self, ty: Ty) -> Option<(bool, Vec<Ty>)> {
        match self.kind(ty) {
            TyKind::Map(k, v) => Some((true, vec![k, v])),
            TyKind::Set(t) => Some((false, vec![t])),
            _ => None,
        }
    }

    /// An empty table at `dst`: four zero words.
    fn empty_table(&mut self, dst: Value) {
        let z = self.i64c(0);
        let np = self
            .emit(
                Op::Cast(CastKind::IntToPtr, z, IrTy::Ptr),
                Some(IrTy::Ptr),
                Span::new(0, 0),
            )
            .unwrap_or(dst);
        self.store_at(dst, 0, np);
        for off in [8, 16] {
            let z = self.i64c(0);
            self.store_at(dst, off, z);
        }
        self.store_at(dst, 24, np);
    }

    pub(crate) fn map_lit(&mut self, e: &'a Expr, items: &'a [MapItem], dst: Value, t: Ty) -> Res {
        let TyKind::Map(k, v) = self.kind(t) else {
            return self.unsupported(e.span, "a map literal of this type");
        };
        self.empty_table(dst);
        for it in items {
            match it {
                MapItem::Pair(key, val) => {
                    let ks = self.slot_for(k);
                    if let Res::Never = self.into(key, ks, k) {
                        return Res::Never;
                    }
                    let vs = self.slot_for(v);
                    if let Res::Never = self.into(val, vs, v) {
                        return Res::Never;
                    }
                    self.call_core_g("rt_map_put", &[k, v], vec![dst, ks, vs]);
                }
                MapItem::Spread(x) => {
                    let Res::Val(src) = self.expr(x) else {
                        return Res::Never;
                    };
                    self.call_core_g("rt_map_spread", &[k, v], vec![dst, src]);
                }
            }
        }
        Res::Unit
    }

    pub(crate) fn set_lit(&mut self, e: &'a Expr, items: &'a [ListItem], dst: Value, t: Ty) -> Res {
        let TyKind::Set(el) = self.kind(t) else {
            return self.unsupported(e.span, "a set literal of this type");
        };
        self.empty_table(dst);
        for it in items {
            if it.spread {
                let Res::Val(src) = self.expr(&it.value) else {
                    return Res::Never;
                };
                self.call_core_g("rt_set_spread", &[el], vec![dst, src]);
                continue;
            }
            let ks = self.slot_for(el);
            if let Res::Never = self.into(&it.value, ks, el) {
                return Res::Never;
            }
            self.call_core_g("rt_set_put", &[el], vec![dst, ks]);
        }
        Res::Unit
    }

    pub(crate) fn drop_collection(&mut self, p: Value, ty: Ty) {
        if let Some((is_map, tys)) = self.collection_args(ty) {
            let name = if is_map { "rt_map_drop" } else { "rt_set_drop" };
            self.call_core_g(name, &tys, vec![p]);
        }
    }

    pub(crate) fn clone_collection(&mut self, s: Value, d: Value, ty: Ty) {
        if let Some((is_map, tys)) = self.collection_args(ty) {
            let name = if is_map {
                "rt_map_clone"
            } else {
                "rt_set_clone"
            };
            self.call_core_g(name, &tys, vec![s, d]);
        }
    }

    pub(crate) fn eq_collection(&mut self, a: Value, b: Value, ty: Ty, fail: BlockId) -> bool {
        let Some((is_map, tys)) = self.collection_args(ty) else {
            return false;
        };
        let name = if is_map { "rt_map_eq" } else { "rt_set_eq" };
        let Some(r) = self.call_core_g(name, &tys, vec![a, b]) else {
            return false;
        };
        let ok = self.new_block(&[]);
        self.branch(r, ok, fail);
        self.start(ok);
        true
    }

    pub(crate) fn show_collection(&mut self, v: Value, out: Value, ty: Ty) {
        if let Some((is_map, tys)) = self.collection_args(ty) {
            let name = if is_map { "rt_map_show" } else { "rt_set_show" };
            self.call_core_g(name, &tys, vec![v, out]);
        }
    }

    /// `item in set` and `key in map`: a lookup in the table.
    pub(crate) fn in_collection(
        &mut self,
        e: &'a Expr,
        l: &'a Expr,
        r: &'a Expr,
        lt: Ty,
        rt: Ty,
    ) -> Res {
        let key = match self.kind(rt) {
            TyKind::Set(t) => t,
            TyKind::Map(k, _) => k,
            _ => return self.unsupported(e.span, "`in` on this type"),
        };
        let Res::Val(h) = self.expr(r) else {
            return Res::Never;
        };
        let Some(probe) = self.probe_addr(l, lt, key) else {
            return Res::Never;
        };
        let size = self.layout(key).size;
        let sz = self.i64c(size as i64);
        let Some(eq) = self.glue_addr(GlueKind::Eq, key) else {
            return Res::Never;
        };
        let Some(hash) = self.glue_addr(GlueKind::Hash, key) else {
            return Res::Never;
        };
        match self.call_core("rt_map_has", vec![h, probe, sz, eq, hash]) {
            Some(v) => Res::Val(v),
            None => Res::Never,
        }
    }
}
