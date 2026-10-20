//! Objects (`obj`): a heap block with a table pointer first, then the fields (the base's fields first).
//!
//! The value of an `obj` type is the address of the block. The table is static data, built the first
//! time it is needed; its slots are:
//!
//! | slot | holds |
//! |------|-------|
//! | 0 | the function that drops the object (runs `deinit`, drops the fields, frees the block) |
//! | 1 | the function that copies it (the type's own `clone`) |
//! | 2 | the address of the list of the type's ancestors, for `is` and `to?` |
//! | 3 | (reserved) |
//! | 4.. | the `open` and `override` methods, in the order the hierarchy declares them |

use super::*;

pub(crate) const OBJ_HEADER: u32 = 8;
const FIRST_VIRTUAL: usize = 4;

impl<'a> Lowerer<'a> {
    pub(crate) fn obj_def(&mut self, t: Ty) -> Option<DefId> {
        match self.kind(t) {
            TyKind::Adt(d, _) if self.res.def(d).kind == DefKind::Obj => Some(d),
            _ => None,
        }
    }

    /// The type itself, then its base, then that one's base...
    pub(crate) fn obj_chain(&mut self, t: Ty) -> Vec<Ty> {
        let mut out = vec![t];
        let mut cur = t;
        while let Some(d) = self.obj_def(cur) {
            let Some(base) = self.typed.adts.get(&d).and_then(|a| a.base) else {
                break;
            };
            let TyKind::Adt(_, args) = self.kind(cur) else {
                break;
            };
            let map = self.adt_subst(d, &args);
            let base = self.types.subst(base, &map);
            let base = self.types.zonk(base);
            out.push(base);
            cur = base;
        }
        out
    }

    fn is_virtual(&self, m: DefId) -> bool {
        self.fns
            .get(&m)
            .is_some_and(|e| e.mods.is_open || e.mods.is_override || e.mods.is_abstract)
    }

    /// The names of the overridable methods of a hierarchy, root first.
    pub(crate) fn virtual_names(&mut self, t: Ty) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for c in self.obj_chain(t).into_iter().rev() {
            let Some(d) = self.obj_def(c) else { continue };
            let members = self.res.def(d).members.clone();
            for m in members {
                if self.res.def(m).kind == DefKind::Fn && self.is_virtual(m) {
                    let n = self.res.def(m).name.clone();
                    if !names.contains(&n) {
                        names.push(n);
                    }
                }
            }
        }
        names
    }

    /// The table slot of a method that is looked up at run time, with the type that declares it.
    pub(crate) fn virtual_slot(&mut self, m: DefId) -> Option<usize> {
        if !self.is_virtual(m) {
            return None;
        }
        let owner = self.res.def(m).parent?;
        if self.res.def(owner).kind != DefKind::Obj {
            return None;
        }
        let ty = self.types.mk(TyKind::Adt(owner, vec![]));
        let name = self.res.def(m).name.clone();
        let names = self.virtual_names(ty);
        names
            .iter()
            .position(|n| *n == name)
            .map(|i| FIRST_VIRTUAL + i)
    }

    pub(crate) fn obj_type_id(&mut self, t: Ty) -> i64 {
        let t = self.types.zonk(t);
        if let Some(i) = self.obj_ids.get(&t) {
            return *i;
        }
        let id = self.obj_ids.len() as i64 + 1;
        self.obj_ids.insert(t, id);
        id
    }

    /// Bytes of the block for an object of this type.
    pub(crate) fn obj_size(&mut self, t: Ty) -> u32 {
        let fs = self.struct_fields(t);
        let l = self.fields_layout(&fs);
        OBJ_HEADER + align_up(l.size, 8)
    }

    /// The address of the type's table, built on first use.
    pub(crate) fn vtable_ptr(&mut self, t: Ty) -> Option<Value> {
        let t = self.types.zonk(t);
        let names = self.virtual_names(t);
        let nslots = FIRST_VIRTUAL + names.len();
        let sp = Span::new(0, 0);
        let idx = match self.vtables.get(&t) {
            Some(i) => *i,
            None => {
                let i = self.module.add_zeroed_data(nslots * 8 + 8);
                self.vtables.insert(t, i);
                i
            }
        };
        let vt = self.emit(Op::DataAddr(idx), Some(IrTy::Ptr), sp)?;
        let flag = self.addr(vt, (nslots * 8) as u32);
        let built = self.load_at(flag, 0, IrTy::Bool);
        let go = self.new_block(&[]);
        let join = self.new_block(&[]);
        self.branch(built, join, go);
        self.start(go);
        let one = self.konst(IrTy::Bool, Const::Bool(true), sp);
        self.emit(Op::Store(flag, one), None, sp);
        // 0 and 1: drop and copy.
        if let Some(f) = self.glue_fn(GlueKind::ObjDrop, t) {
            if let Some(a) = self.emit(Op::FuncAddr(f), Some(IrTy::Ptr), sp) {
                self.store_at(vt, 0, a);
            }
        }
        if let Some(f) = self.glue_fn(GlueKind::ObjClone, t) {
            if let Some(a) = self.emit(Op::FuncAddr(f), Some(IrTy::Ptr), sp) {
                self.store_at(vt, 8, a);
            }
        }
        // 2: the ancestors.
        let chain = self.obj_chain(t);
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(&(chain.len() as i64).to_le_bytes());
        for c in &chain {
            let id = self.obj_type_id(*c);
            bytes.extend_from_slice(&id.to_le_bytes());
        }
        let di = self.module.add_data(&bytes);
        if let Some(a) = self.emit(Op::DataAddr(di), Some(IrTy::Ptr), sp) {
            self.store_at(vt, 16, a);
        }
        // 4..: the methods, as the most derived class defines them.
        for (i, name) in names.iter().enumerate() {
            let mut found = None;
            for c in &chain {
                let Some(d) = self.obj_def(*c) else { continue };
                if let Some(m) = self.res.member(d, name) {
                    if self.res.def(m).kind == DefKind::Fn
                        && self
                            .fns
                            .get(&m)
                            .is_some_and(|e| !matches!(e.decl.body, FnBody::None))
                    {
                        let TyKind::Adt(_, args) = self.kind(*c) else {
                            continue;
                        };
                        let map = self.adt_subst(d, &args);
                        found = self.request_fn(m, &map);
                        break;
                    }
                }
            }
            if let Some(f) = found {
                if let Some(a) = self.emit(Op::FuncAddr(f), Some(IrTy::Ptr), sp) {
                    self.store_at(vt, ((FIRST_VIRTUAL + i) * 8) as u32, a);
                }
            }
        }
        self.jump(join, vec![]);
        self.start(join);
        Some(vt)
    }

    /// A new, zeroed block of this object type with its table pointer set.
    pub(crate) fn obj_new(&mut self, t: Ty) -> Option<Value> {
        let sp = Span::new(0, 0);
        let size = self.obj_size(t);
        let n = self.i64c(size as i64);
        let p = self.emit(Op::Alloc(n), Some(IrTy::Ptr), sp)?;
        // Fields that are never written (a failed `init`) must be safe to look at.
        let zero = self.i64c(0);
        let mut off = 0;
        while off < size {
            let q = self.addr(p, off);
            self.emit(Op::Store(q, zero), None, sp);
            off += 8;
        }
        let vt = self.vtable_ptr(t)?;
        self.store_at(p, 0, vt);
        Some(p)
    }

    /// `dog { name: "rex" }`
    pub(crate) fn obj_literal_value(&mut self, e: &'a Expr, inits: &'a [StructInit], t: Ty) -> Res {
        let Some(p) = self.obj_new(t) else {
            return Res::Never;
        };
        let body = self.addr(p, OBJ_HEADER);
        if let Res::Never = self.fill_struct(e, inits, body, t) {
            return Res::Never;
        }
        self.fresh_scalar(p, t);
        Res::Val(p)
    }

    /// `dog(args)`: allocate, apply the defaults, run `init`.
    pub(crate) fn obj_construct(
        &mut self,
        e: &'a Expr,
        init: DefId,
        map: HashMap<DefId, Ty>,
        t: Ty,
        args: &'a [Arg],
    ) -> Res {
        let Some(p) = self.obj_new(t) else {
            return Res::Never;
        };
        let body = self.addr(p, OBJ_HEADER);
        self.fill_defaults(body, t);
        let Some(info) = self.typed.fn_infos.get(&init).cloned() else {
            return self.unsupported(e.span, "an `init` without a signature");
        };
        let info = self.fn_info_for(init, &info);
        let mut srcs: Vec<super::call::ArgSource<'a>> = vec![super::call::ArgSource::Val(p)];
        match self.match_args(&info, init, args, e.span) {
            Some(mut v) => srcs.append(&mut v),
            None => return Res::Never,
        }
        match self.call_with(e, init, &map, srcs, None) {
            Res::Never => Res::Never,
            _ => {
                self.fresh_scalar(p, t);
                Res::Val(p)
            }
        }
    }

    /// Is the object at `o` an instance of `target` (or of something that extends it)?
    pub(crate) fn obj_is(&mut self, o: Value, target: Ty) -> Option<Value> {
        let id = self.obj_type_id(target);
        let want = self.i64c(id);
        self.call_core("rt_obj_is", vec![o, want])
    }

    /// `x to? dog`: the object moves into the option when it is a `dog`.
    pub(crate) fn downcast(&mut self, e: &'a Expr, v: &'a Expr, dst: Value, t: Ty) -> Res {
        let sp = e.span;
        if let Some(TyKind::Any(iface)) = self.ty_of(v).map(|s| self.kind(s)) {
            return self.downcast_any(e, v, dst, t, iface);
        }
        let TyKind::Option(target) = self.kind(t) else {
            return self.unsupported(sp, "`to?` to this type");
        };
        let Some(info) = self.enum_info(t) else {
            return self.unsupported(sp, "`to?` to this type");
        };
        let Some(o) = self.scalar(v) else {
            return Res::Never;
        };
        let Some(ok) = self.obj_is(o, target) else {
            return Res::Never;
        };
        let yes = self.new_block(&[]);
        let no = self.new_block(&[]);
        let done = self.new_block(&[]);
        self.branch(ok, yes, no);
        self.start(no);
        let zero = self.konst(info.tag, Const::Int(0), sp);
        self.store_at(dst, 0, zero);
        self.jump(done, vec![]);
        self.start(yes);
        let one = self.konst(info.tag, Const::Int(1), sp);
        self.store_at(dst, 0, one);
        if let Some(f) = info.variants[1].first() {
            self.store_at(dst, f.off, o);
        }
        self.mark_moved(v);
        self.jump(done, vec![]);
        self.start(done);
        Res::Unit
    }
}
