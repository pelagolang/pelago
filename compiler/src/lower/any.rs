//! `any interface`: a value of any type that provides the interface, in two words: the address of a heap
//! copy of the value, and the address of a table.
//!
//! The table is static data for one (type, interface) pair, built the first time it is needed:
//!
//! | slot | holds |
//! |------|-------|
//! | 0 | the function that drops the heap copy and frees it |
//! | 1 | the type's `eq` glue (two addresses, gives a bool), when the interface builds on `eq` |
//! | 2 | the type's `hash` glue (an address, gives an i64), when the interface builds on `hash` |
//! | 3.. | one function per method of the interface, in declaration order |
//!
//! A method in the table takes the address of the heap copy where the method wants `self`; it loads
//! the value first when the type passes `self` by value.

use super::expr::strip_parens;
use super::*;

const EQ_SLOT: usize = 1;
const HASH_SLOT: usize = 2;
const FIRST_METHOD: usize = 3;

pub(crate) struct AnyThunkJob {
    pub id: FuncId,
    pub target: FuncId,
    pub sig: Sig,
}

impl<'a> Lowerer<'a> {
    /// The methods of an interface in table order.
    pub(crate) fn iface_methods(&mut self, iface: DefId) -> Vec<DefId> {
        self.res
            .def(iface)
            .members
            .clone()
            .into_iter()
            .filter(|m| self.res.def(*m).kind == DefKind::Fn)
            .collect()
    }

    /// The table slot of an interface method.
    pub(crate) fn any_slot(&mut self, m: DefId) -> Option<usize> {
        let iface = self.res.def(m).parent?;
        if self.res.def(iface).kind != DefKind::Interface {
            return None;
        }
        let ms = self.iface_methods(iface);
        ms.iter().position(|x| *x == m).map(|i| FIRST_METHOD + i)
    }

    /// The data block of the table for `t` seen as `iface` (made empty on first use; the table is
    /// filled in by the first code that needs a value in it).
    fn any_table_data(&mut self, t: Ty, iface: DefId) -> u32 {
        let t = self.types.zonk(t);
        if let Some(i) = self.any_tables.get(&(t, iface)) {
            return *i;
        }
        let nslots = FIRST_METHOD + self.iface_methods(iface).len();
        let i = self.module.add_zeroed_data(nslots * 8 + 8);
        self.any_tables.insert((t, iface), i);
        i
    }

    /// `x to? sq` on an `any shape`: when the table is the one for (`sq`, `shape`), the value moves
    /// out of its heap copy into the option, and the copy is freed. A failed test leaves `x` alone.
    pub(crate) fn downcast_any(
        &mut self,
        e: &'a Expr,
        v: &'a Expr,
        dst: Value,
        t: Ty,
        iface: DefId,
    ) -> Res {
        let sp = e.span;
        let (TyKind::Option(target), Some(info)) = (self.kind(t), self.enum_info(t)) else {
            return self.unsupported(sp, "`to?` to this type");
        };
        let named = matches!(strip_parens(v).kind, ExprKind::Name(_));
        let place = if named { self.place(v) } else { None };
        let Some(p) = place else {
            return self.unsupported(sp, "`to?` on an `any` that is not a variable");
        };
        let idx = self.any_table_data(target, iface);
        let Some(want) = self.emit(Op::DataAddr(idx), Some(IrTy::Ptr), sp) else {
            return Res::Never;
        };
        let have = self.load_at(p, 8, IrTy::Ptr);
        let Some(same) = self.emit(Op::Cmp(CmpOp::Eq, have, want), Some(IrTy::Bool), sp) else {
            return Res::Never;
        };
        let yes = self.new_block(&[]);
        let no = self.new_block(&[]);
        let done = self.new_block(&[]);
        self.branch(same, yes, no);
        self.start(no);
        let zero = self.konst(info.tag, Const::Int(0), sp);
        self.store_at(dst, 0, zero);
        self.jump(done, vec![]);
        self.start(yes);
        let one = self.konst(info.tag, Const::Int(1), sp);
        self.store_at(dst, 0, one);
        let data = self.load_at(p, 0, IrTy::Ptr);
        if let Some(f) = info.variants[1].first() {
            let at = self.addr(dst, f.off);
            self.copy_mem_or_scalar(at, data, target);
        }
        self.emit(Op::Free(data), None, sp);
        self.mark_moved(v);
        self.jump(done, vec![]);
        self.start(done);
        Res::Unit
    }

    /// The address of the table for `t` seen as `iface`.
    pub(crate) fn any_vtable(&mut self, t: Ty, iface: DefId) -> Option<Value> {
        let t = self.types.zonk(t);
        let methods = self.iface_methods(iface);
        let nslots = FIRST_METHOD + methods.len();
        let sp = Span::new(0, 0);
        let idx = self.any_table_data(t, iface);
        let vt = self.emit(Op::DataAddr(idx), Some(IrTy::Ptr), sp)?;
        let flag = self.addr(vt, (nslots * 8) as u32);
        let built = self.load_at(flag, 0, IrTy::Bool);
        let go = self.new_block(&[]);
        let join = self.new_block(&[]);
        self.branch(built, join, go);
        self.start(go);
        let one = self.konst(IrTy::Bool, Const::Bool(true), sp);
        self.emit(Op::Store(flag, one), None, sp);
        if let Some(f) = self.glue_fn(GlueKind::BoxDrop, t) {
            if let Some(a) = self.emit(Op::FuncAddr(f), Some(IrTy::Ptr), sp) {
                self.store_at(vt, 0, a);
            }
        }
        let (has_eq, has_hash) = self
            .typed
            .iface_eq_hash
            .get(&iface)
            .copied()
            .unwrap_or((false, false));
        for (wanted, kind, slot) in [
            (has_eq, GlueKind::Eq, EQ_SLOT),
            (has_hash, GlueKind::Hash, HASH_SLOT),
        ] {
            if !wanted {
                continue;
            }
            if let Some(f) = self.glue_fn(kind, t) {
                if let Some(a) = self.emit(Op::FuncAddr(f), Some(IrTy::Ptr), sp) {
                    self.store_at(vt, (slot * 8) as u32, a);
                }
            }
        }
        for (i, m) in methods.iter().enumerate() {
            let name = self.res.def(*m).name.clone();
            // The type's own method, or the interface's default.
            let (real, map) = match self.find_impl_method(t, &name) {
                Some(x) => x,
                None if name == "show" && self.res.def(iface).name == "show" => {
                    // Built-in shapes show through the library.
                    if let Some(def) = self.core_fn("rt_any_show") {
                        let g = self.typed.fn_infos.get(&def).map(|i| i.generics.clone());
                        let map: HashMap<DefId, Ty> =
                            g.unwrap_or_default().into_iter().zip([t]).collect();
                        if let Some(id) = self.request_fn(def, &map) {
                            if let Some(a) = self.emit(Op::FuncAddr(id), Some(IrTy::Ptr), sp) {
                                self.store_at(vt, ((FIRST_METHOD + i) * 8) as u32, a);
                            }
                        }
                    }
                    continue;
                }
                None => {
                    let mut map = HashMap::new();
                    map.insert(iface, t);
                    (*m, map)
                }
            };
            if self
                .fns
                .get(&real)
                .is_some_and(|e| matches!(e.decl.body, FnBody::None))
                && !self.is_intrinsic(real)
            {
                continue;
            }
            let Some(target) = self.request_fn(real, &map) else {
                continue;
            };
            let Some(info) = self.typed.fn_infos.get(&real).cloned() else {
                continue;
            };
            let info = self.fn_info_for(real, &info);
            let sig = self.sig_of(&info, &map);
            let tname = self.unique_name(format!("any<{}.{}>", self.show_ty(t), name));
            let mut ir_params: Vec<IrTy> = Vec::new();
            if sig.ret_pass == RetPass::Sret {
                ir_params.push(IrTy::Ptr);
            }
            ir_params.push(IrTy::Ptr);
            let skip = usize::from(sig.ret_pass == RetPass::Sret)
                + usize::from(!matches!(sig.params.first(), Some((Pass::Skip, _, _))));
            ir_params.extend_from_slice(&sig.ir_params[skip..]);
            let mut func = Func::new(&tname, &ir_params, sig.ir_ret);
            let entry = func.entry();
            func.terminate(entry, Term::Stop("not lowered yet".to_string()));
            let id = self.module.add(func);
            self.any_thunks.push(AnyThunkJob { id, target, sig });
            if let Some(a) = self.emit(Op::FuncAddr(id), Some(IrTy::Ptr), sp) {
                self.store_at(vt, ((FIRST_METHOD + i) * 8) as u32, a);
            }
        }
        self.jump(join, vec![]);
        self.start(join);
        Some(vt)
    }

    pub(crate) fn lower_any_thunk(&mut self, job: AnyThunkJob) {
        let placeholder = self.module.funcs[job.id.0 as usize].clone();
        let mut func = Func::new(&placeholder.name, &placeholder.params, placeholder.ret);
        let entry = func.entry();
        let ps = func.block_params(entry).to_vec();
        let mut args: Vec<Value> = Vec::new();
        let mut i = 0;
        if job.sig.ret_pass == RetPass::Sret {
            args.push(ps[0]);
            i = 1;
        }
        let data = ps[i];
        i += 1;
        match job.sig.params.first() {
            Some((Pass::Val(t), _, _)) => {
                if let Some(v) = func.emit(entry, Op::Load(*t, data), Some(*t)) {
                    args.push(v);
                }
            }
            Some((Pass::Ptr, _, _)) => args.push(data),
            _ => {}
        }
        args.extend_from_slice(&ps[i..]);
        let ret_ty = match job.sig.ret_pass {
            RetPass::Val(t) => Some(t),
            _ => None,
        };
        let r = func.emit(entry, Op::Call(job.target, args), ret_ty);
        func.terminate(entry, Term::Return(r));
        self.module.funcs[job.id.0 as usize] = func;
    }

    /// A value of a concrete type used as `any iface`: copy it to the heap and pair it with its table.
    pub(crate) fn coerce_to_any(
        &mut self,
        e: &'a Expr,
        have: Ty,
        dst: Value,
        iface: DefId,
    ) -> Option<Res> {
        let sp = e.span;
        if let TyKind::Any(i) = self.kind(have) {
            if i == iface {
                return Some(self.into_exact(e, dst, have));
            }
        }
        let size = self.layout(have).size.max(1);
        let n = self.i64c(size as i64);
        let data = self.emit(Op::Alloc(n), Some(IrTy::Ptr), sp)?;
        if let Res::Never = self.into(e, data, have) {
            return Some(Res::Never);
        }
        let vt = self.any_vtable(have, iface)?;
        self.store_at(dst, 0, data);
        self.store_at(dst, 8, vt);
        Some(Res::Unit)
    }

    /// Drops an `any`: the table's drop function frees the heap copy.
    pub(crate) fn drop_any(&mut self, p: Value, _ty: Ty) {
        let sp = Span::new(0, 0);
        let data = self.load_at(p, 0, IrTy::Ptr);
        let vt = self.load_at(p, 8, IrTy::Ptr);
        let f = self.load_at(vt, 0, IrTy::Ptr);
        self.emit(
            Op::CallPtr(vec![IrTy::Ptr], IrTy::Void, f, vec![data]),
            None,
            sp,
        );
    }

    pub(crate) fn clone_any(&mut self, _s: Value, _d: Value, _ty: Ty) {
        self.terminate(Term::Stop("an `any` cannot be copied".to_string()));
        self.f.cur = Some(self.f.func.entry());
    }

    /// The glue that drops a heap copy of `ty` and frees it.
    pub(crate) fn box_drop_body(&mut self, data: Value, ty: Ty) {
        self.drop_at(data, ty);
        self.emit(Op::Free(data), None, Span::new(0, 0));
    }

    /// Compares two `any` values at `a` and `b`: equal when both hold the same type (the same table)
    /// and that type's `eq` says so. Jumps to `fail` otherwise.
    pub(crate) fn eq_any(&mut self, a: Value, b: Value, fail: BlockId) -> bool {
        let sp = Span::new(0, 0);
        let (ta, tb) = (self.load_at(a, 8, IrTy::Ptr), self.load_at(b, 8, IrTy::Ptr));
        let same = self.emit(Op::Cmp(CmpOp::Eq, ta, tb), Some(IrTy::Bool), sp);
        let Some(same) = same else { return false };
        let ok = self.new_block(&[]);
        self.branch(same, ok, fail);
        self.start(ok);
        let (da, db) = (self.load_at(a, 0, IrTy::Ptr), self.load_at(b, 0, IrTy::Ptr));
        let f = self.load_at(ta, (EQ_SLOT * 8) as u32, IrTy::Ptr);
        let Some(r) = self.emit(
            Op::CallPtr(vec![IrTy::Ptr, IrTy::Ptr], IrTy::Bool, f, vec![da, db]),
            Some(IrTy::Bool),
            sp,
        ) else {
            return false;
        };
        let next = self.new_block(&[]);
        self.branch(r, next, fail);
        self.start(next);
        true
    }

    /// The hash of an `any` at `a`: its type's `hash` glue on the heap copy.
    pub(crate) fn hash_any(&mut self, a: Value) -> Option<Value> {
        let data = self.load_at(a, 0, IrTy::Ptr);
        let vt = self.load_at(a, 8, IrTy::Ptr);
        let f = self.load_at(vt, (HASH_SLOT * 8) as u32, IrTy::Ptr);
        self.emit(
            Op::CallPtr(vec![IrTy::Ptr], IrTy::I64, f, vec![data]),
            Some(IrTy::I64),
            Span::new(0, 0),
        )
    }

    /// Prints an `any` whose interface has `show`.
    pub(crate) fn show_any(&mut self, v: Value, out: Value, iface: DefId) {
        let sp = Span::new(0, 0);
        let Some(m) = self
            .iface_methods(iface)
            .into_iter()
            .find(|m| self.res.def(*m).name == "show")
        else {
            self.unsupported(sp, "printing an `any` whose interface has no `show`");
            return;
        };
        let Some(slot) = self.any_slot(m) else { return };
        let data = self.load_at(v, 0, IrTy::Ptr);
        let vt = self.load_at(v, 8, IrTy::Ptr);
        let f = self.load_at(vt, (slot * 8) as u32, IrTy::Ptr);
        let string = self.types.string;
        let tmp = self.slot_for(string);
        self.emit(
            Op::CallPtr(vec![IrTy::Ptr, IrTy::Ptr], IrTy::Void, f, vec![tmp, data]),
            None,
            sp,
        );
        let d = self.load_at(tmp, 0, IrTy::Ptr);
        let n = self.load_at(tmp, 8, IrTy::I64);
        self.call_core("rt_str_push", vec![out, d, n]);
        self.drop_at(tmp, string);
    }

    /// Is this address not null?
    pub(crate) fn nonnull(&mut self, p: Value) -> Option<Value> {
        let sp = Span::new(0, 0);
        let n = self.emit(
            Op::Cast(CastKind::PtrToInt, p, IrTy::I64),
            Some(IrTy::I64),
            sp,
        )?;
        let zero = self.i64c(0);
        self.emit(Op::Cmp(CmpOp::Ne, n, zero), Some(IrTy::Bool), sp)
    }

    /// Drops a function value: a closure that owns its environment frees it through the environment's table.
    pub(crate) fn drop_fn_value(&mut self, p: Value) {
        let sp = Span::new(0, 0);
        let env = self.load_at(p, 8, IrTy::Ptr);
        let Some(has_env) = self.nonnull(env) else {
            return;
        };
        self.if_then_pub(has_env, |s| {
            let hdr = s.load_at(env, 0, IrTy::Ptr);
            if let Some(owns) = s.nonnull(hdr) {
                s.if_then_pub(owns, |s| {
                    s.emit(
                        Op::CallPtr(vec![IrTy::Ptr], IrTy::Void, hdr, vec![env]),
                        None,
                        sp,
                    );
                });
            }
        });
    }

    /// Copies a function value; one that owns its environment cannot be copied.
    pub(crate) fn clone_fn_value(&mut self, s: Value, d: Value) {
        let code = self.load_at(s, 0, IrTy::Ptr);
        let env = self.load_at(s, 8, IrTy::Ptr);
        if let Some(has_env) = self.nonnull(env) {
            self.if_then_pub(has_env, |s| {
                let hdr = s.load_at(env, 0, IrTy::Ptr);
                if let Some(owns) = s.nonnull(hdr) {
                    s.if_then_pub(owns, |s| {
                        s.terminate(Term::Stop(
                            "a closure that owns data cannot be copied".to_string(),
                        ));
                        let b = s.new_block(&[]);
                        s.start(b);
                    });
                }
            });
        }
        self.store_at(d, 0, code);
        self.store_at(d, 8, env);
    }
}
