//! `meta expr` (card A-05, stage M3): code that runs while compiling.
//!
//! The expression becomes a small function of its own; everything it reaches is lowered, the module
//! built so far is run in the IR interpreter under a step budget, and the value left in memory is read
//! back into a tree of constants ([`Cv`]). That tree is then written into the program as code that
//! builds the value (numbers are constants, text points at static data), so the result is baked in and
//! no compile-time code is left in the output.
//!
//! Bakeable: numbers, `bool`, `char`, `string`, `str`, fixed arrays, lists, tuples, structs, enums,
//! `option` and `result`, `box`, maps and sets of bakeable values. Maps and sets are rebuilt by
//! inserting the entries when the code runs (the hash table itself is not static data). Not bakeable:
//! closures, `obj`, `any`, tasks, raw pointers.

use super::*;
use crate::ir::Val;

/// A value read back from the interpreter's memory.
#[derive(Clone, Debug)]
pub(crate) enum Cv {
    Unit,
    Int(i128),
    Float(f64),
    Bool(bool),
    Text(Vec<u8>),
    List(Vec<Cv>),
    /// A struct, tuple or array: the fields in order.
    Fields(Vec<Cv>),
    /// An enum, `option` or `result`: the case and its fields.
    Case(usize, Vec<Cv>),
    /// A `box`: the value it holds.
    Boxed(Box<Cv>),
    /// A map (key and value) or set (key only): the entries in order.
    Entries(Vec<(Cv, Option<Cv>)>),
}

pub(crate) struct MetaJob<'a> {
    pub id: FuncId,
    pub expr: &'a Expr,
    pub file: ModuleId,
    pub subst: HashMap<DefId, Ty>,
    pub ty: Ty,
}

/// The default step budget of one compile-time run.
const DEFAULT_LIMIT: u64 = 10_000_000;

impl<'a> Lowerer<'a> {
    fn meta_error(&mut self, span: Span, msg: String) -> Res {
        self.diags.push(Diagnostic {
            module: self.f.file,
            span,
            message: msg,
            warning: false,
        });
        self.unsupported(span, "an error in compile-time code")
    }

    /// `meta e` where `e` has type `t`: evaluated now, baked into `dst` (memory values) or returned.
    pub(crate) fn meta_expr(&mut self, e: &'a Expr, inner: &'a Expr, dst: Option<Value>) -> Res {
        let Some(t) = self.ty_of(e) else {
            return self.unsupported(e.span, "a `meta` value of unknown type");
        };
        let key = (self.f.file, e.span.start, e.span.end);
        let cv = if let (true, Some(c)) = (self.f.subst.is_empty(), self.meta_cache.get(&key)) {
            c.clone()
        } else {
            if self.meta_stack.contains(&key) {
                return self.meta_error(e.span, "this constant needs itself".to_string());
            }
            self.meta_stack.push(key);
            let r = self.meta_run(e, inner, t);
            self.meta_stack.pop();
            let Some(cv) = r else {
                return Res::Never;
            };
            let cv = Rc::new(cv);
            if self.f.subst.is_empty() {
                self.meta_cache.insert(key, cv.clone());
            }
            cv
        };
        match self.repr(t) {
            Repr::Scalar(s) => match self.cv_scalar(&cv, s, e.span) {
                Some(v) => Res::Val(v),
                None => Res::Never,
            },
            Repr::Zst => Res::Unit,
            Repr::Mem(_) => {
                let d = match dst {
                    Some(d) => d,
                    None => {
                        let d = self.slot_for(t);
                        self.emit_cv(&cv, d, t, e.span);
                        self.own(d, t, false);
                        return Res::Val(d);
                    }
                };
                self.emit_cv(&cv, d, t, e.span);
                Res::Unit
            }
        }
    }

    fn job_id(j: &Job<'a>) -> FuncId {
        match j {
            Job::Fn { id, .. } | Job::Glue { id, .. } | Job::EnvDrop { id, .. } => *id,
            Job::Closure(c) => c.id,
            Job::Meta(m) => m.id,
        }
    }

    /// The functions that can be reached from `start` by calls and function addresses.
    fn reachable_from(&self, start: FuncId) -> std::collections::HashSet<FuncId> {
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![start];
        while let Some(f) = stack.pop() {
            if !seen.insert(f) {
                continue;
            }
            let func = &self.module.funcs[f.0 as usize];
            for b in &func.blocks {
                for &i in &b.insts {
                    match &func.insts[i].op {
                        Op::Call(c, _) | Op::FuncAddr(c) => stack.push(*c),
                        _ => {}
                    }
                }
            }
        }
        seen
    }

    /// Runs the expression and reads the result back.
    fn meta_run(&mut self, e: &'a Expr, inner: &'a Expr, t: Ty) -> Option<Cv> {
        let repr = self.repr(t);
        let (params, ret) = match repr {
            Repr::Mem(_) => (vec![IrTy::Ptr], IrTy::Void),
            Repr::Scalar(s) => (vec![], s),
            Repr::Zst => (vec![], IrTy::Void),
        };
        let name = self.unique_name("meta".to_string());
        let mut func = Func::new(&name, &params, ret);
        let entry = func.entry();
        func.terminate(entry, Term::Stop("not lowered yet".to_string()));
        let id = self.module.add(func);
        self.jobs.push(Job::Meta(MetaJob {
            id,
            expr: inner,
            file: self.f.file,
            subst: self.f.subst.clone(),
            ty: t,
        }));
        // Lower everything the expression reaches (and nothing else), without losing the function
        // being lowered.
        let void = self.types.void;
        let saved = std::mem::replace(&mut self.f, FnState::empty(void));
        let entry_job = self.jobs.pop();
        let mut waiting = std::mem::take(&mut self.jobs);
        self.jobs.extend(entry_job);
        loop {
            self.run_jobs();
            let reached = self.reachable_from(id);
            let (needed, rest): (Vec<_>, Vec<_>) = waiting
                .into_iter()
                .partition(|j| reached.contains(&Self::job_id(j)));
            waiting = rest;
            if needed.is_empty() {
                break;
            }
            self.jobs.extend(needed);
        }
        self.jobs.extend(waiting);
        self.f = saved;
        let limit = self.meta_limit(e.span).unwrap_or(DEFAULT_LIMIT);
        let size = match repr {
            Repr::Mem(l) => Some(l.size as usize),
            _ => None,
        };
        let cap = crate::ir::run_capture(&self.module, &name, size, limit);
        let panicked = matches!(&cap.outcome.result, Err(m) if m == "panic");
        let lines: Vec<&str> = cap.outcome.output.lines().collect();
        // A `panic` (and so `meta_error`) prints its message; that message is the error.
        let shown = if panicked {
            &lines[..lines.len().saturating_sub(1)]
        } else {
            &lines[..]
        };
        for line in shown {
            self.diags.push(Diagnostic {
                module: self.f.file,
                span: e.span,
                message: line.to_string(),
                warning: true,
            });
        }
        if let Err(msg) = &cap.outcome.result {
            let what = if panicked {
                lines.last().map_or_else(
                    || "compile-time code panicked".to_string(),
                    |l| l.to_string(),
                )
            } else if msg.contains("step limit") {
                format!("compile-time code ran too long (more than {limit} steps)")
            } else {
                format!("compile-time code stopped: {msg}")
            };
            // Where it was running: the call chain, innermost first, without the library's own helpers.
            let chain: Vec<&str> = cap
                .outcome
                .trace
                .iter()
                .map(String::as_str)
                .filter(|n| *n != name && !n.starts_with("rt_"))
                .collect();
            let what = if chain.is_empty() {
                what
            } else {
                format!("{what} (in {})", chain.join(" <- "))
            };
            self.meta_error(e.span, what);
            return None;
        }
        // What the result itself holds is still allocated when the run ends; only other leaks count.
        if cap.outcome.leaked > 0 && !self.needs_drop(t) {
            self.meta_error(
                e.span,
                "compile-time code leaked memory; free what it allocates".to_string(),
            );
            return None;
        }
        let at = match repr {
            Repr::Mem(_) => cap.ret_addr,
            _ => 0,
        };
        let value = match repr {
            Repr::Scalar(s) => match cap.outcome.result {
                Ok(v) => self.scalar_cv(v, s),
                Err(_) => None,
            },
            _ => self.read_cv(&cap.image, at, t),
        };
        match value {
            Some(v) => Some(v),
            None => {
                let s = self.typed.show(t);
                self.meta_error(
                    e.span,
                    format!("a value of type `{s}` cannot be computed at compile time (closures, objects and `any` cannot be baked in)"),
                );
                None
            }
        }
    }

    /// `@meta_limit(n)` on the declaration that holds this `meta`, when written.
    fn meta_limit(&self, span: Span) -> Option<u64> {
        self.meta_limits
            .iter()
            .find(|(f, item, _)| {
                *f == self.f.file && item.start <= span.start && span.end <= item.end
            })
            .map(|(_, _, n)| *n)
    }

    fn scalar_cv(&self, v: Val, s: IrTy) -> Option<Cv> {
        match (v, s) {
            (Val::Int(n), IrTy::Int(bits)) => {
                let _ = bits;
                Some(Cv::Int(n as i128))
            }
            (Val::Bool(b), _) => Some(Cv::Bool(b)),
            (Val::Float(x), _) => Some(Cv::Float(x)),
            (Val::Unit, _) => Some(Cv::Unit),
            _ => None,
        }
    }

    // ---- Reading memory back ----------------------------------------------------------------------------

    fn read_n(img: &[u8], at: usize, n: usize) -> Option<u128> {
        let b = img.get(at..at.checked_add(n)?)?;
        let mut v: u128 = 0;
        for (i, x) in b.iter().enumerate() {
            v |= (*x as u128) << (8 * i);
        }
        Some(v)
    }

    fn read_cv(&mut self, img: &[u8], at: usize, t: Ty) -> Option<Cv> {
        let t = self.types.zonk(t);
        match self.kind(t) {
            TyKind::Void => Some(Cv::Unit),
            TyKind::Bool => Some(Cv::Bool(Self::read_n(img, at, 1)? != 0)),
            TyKind::Char => Some(Cv::Int(Self::read_n(img, at, 4)? as i128)),
            TyKind::Int(k) => {
                let bytes = (k.bits() / 8) as usize;
                let raw = Self::read_n(img, at, bytes)?;
                let v = if k.signed() && bytes < 16 {
                    let shift = 128 - 8 * bytes as u32;
                    ((raw << shift) as i128) >> shift
                } else {
                    raw as i128
                };
                Some(Cv::Int(v))
            }
            TyKind::Float(FloatKind::F64) => {
                Some(Cv::Float(f64::from_bits(Self::read_n(img, at, 8)? as u64)))
            }
            TyKind::Float(FloatKind::F32) => Some(Cv::Float(f32::from_bits(
                Self::read_n(img, at, 4)? as u32,
            ) as f64)),
            TyKind::String | TyKind::Str => {
                let p = Self::read_n(img, at, 8)? as usize;
                let n = Self::read_n(img, at + 8, 8)? as usize;
                Some(Cv::Text(img.get(p..p.checked_add(n)?)?.to_vec()))
            }
            TyKind::List(e) => {
                let p = Self::read_n(img, at, 8)? as usize;
                let n = Self::read_n(img, at + 8, 8)? as usize;
                let size = self.layout(e).size as usize;
                let mut out = Vec::new();
                for i in 0..n {
                    out.push(self.read_cv(img, p + i * size, e)?);
                }
                Some(Cv::List(out))
            }
            TyKind::Array(e, len) => {
                let n = self.array_len(len) as usize;
                let size = self.layout(e).size as usize;
                let mut out = Vec::new();
                for i in 0..n {
                    out.push(self.read_cv(img, at + i * size, e)?);
                }
                Some(Cv::Fields(out))
            }
            TyKind::Tuple(_) => self.read_fields(img, at, t),
            TyKind::Box(e) => {
                let p = Self::read_n(img, at, 8)? as usize;
                Some(Cv::Boxed(Box::new(self.read_cv(img, p, e)?)))
            }
            TyKind::Map(k, v) => {
                let p = Self::read_n(img, at, 8)? as usize;
                let n = Self::read_n(img, at + 8, 8)? as usize;
                let cap = Self::read_n(img, at + 16, 8)? as usize;
                let (ks, vs) = (self.layout(k).size as usize, self.layout(v).size as usize);
                let vals = (cap * ks).div_ceil(8) * 8;
                let mut out = Vec::new();
                for i in 0..n {
                    let key = self.read_cv(img, p + i * ks, k)?;
                    let val = self.read_cv(img, p + vals + i * vs, v)?;
                    out.push((key, Some(val)));
                }
                Some(Cv::Entries(out))
            }
            TyKind::Set(e) => {
                let p = Self::read_n(img, at, 8)? as usize;
                let n = Self::read_n(img, at + 8, 8)? as usize;
                let ks = self.layout(e).size as usize;
                let mut out = Vec::new();
                for i in 0..n {
                    out.push((self.read_cv(img, p + i * ks, e)?, None));
                }
                Some(Cv::Entries(out))
            }
            TyKind::Adt(d, _) => match self.res.def(d).kind {
                DefKind::Struct => {
                    if self.res.member(d, "deinit").is_some() {
                        return None;
                    }
                    self.read_fields(img, at, t)
                }
                DefKind::Enum => self.read_case(img, at, t),
                _ => None,
            },
            TyKind::Option(_) | TyKind::Result(..) => self.read_case(img, at, t),
            _ => None,
        }
    }

    fn read_fields(&mut self, img: &[u8], at: usize, t: Ty) -> Option<Cv> {
        let fields = self.struct_fields(t);
        let mut out = Vec::new();
        for f in fields {
            out.push(self.read_cv(img, at + f.off as usize, f.ty)?);
        }
        Some(Cv::Fields(out))
    }

    fn read_case(&mut self, img: &[u8], at: usize, t: Ty) -> Option<Cv> {
        let info = self.enum_info(t)?;
        let tag = Self::read_n(img, at, info.tag.size() as usize)? as usize;
        let fields = info.variants.get(tag)?.clone();
        let mut out = Vec::new();
        for f in fields {
            out.push(self.read_cv(img, at + f.off as usize, f.ty)?);
        }
        Some(Cv::Case(tag, out))
    }

    // ---- Writing the constants into the program ---------------------------------------------------------

    fn cv_scalar(&mut self, cv: &Cv, s: IrTy, span: Span) -> Option<Value> {
        let c = match cv {
            Cv::Int(n) => Const::Int(*n),
            Cv::Float(x) => Const::Float(*x),
            Cv::Bool(b) => Const::Bool(*b),
            _ => return None,
        };
        Some(self.konst(s, c, span))
    }

    /// Builds the value `cv` of type `t` at `dst`.
    fn emit_cv(&mut self, cv: &Cv, dst: Value, t: Ty, span: Span) {
        let t = self.types.zonk(t);
        match (cv, self.kind(t)) {
            (Cv::Unit, _) => {}
            (Cv::Int(_) | Cv::Float(_) | Cv::Bool(_), _) => {
                if let Repr::Scalar(s) = self.repr(t) {
                    if let Some(v) = self.cv_scalar(cv, s, span) {
                        self.store_scalar_ty(dst, v);
                    }
                }
            }
            (Cv::Text(bytes), TyKind::String | TyKind::Str) => {
                let is_string = matches!(self.kind(t), TyKind::String);
                let d = self.module.add_data(bytes);
                let p = self
                    .emit(Op::DataAddr(d), Some(IrTy::Ptr), span)
                    .unwrap_or(dst);
                self.store_at(dst, 0, p);
                let n = self.i64c(bytes.len() as i64);
                self.store_at(dst, 8, n);
                if is_string {
                    // Capacity 0: the text is static and is never freed.
                    let zero = self.i64c(0);
                    self.store_at(dst, 16, zero);
                }
            }
            (Cv::List(items), TyKind::List(e)) => {
                let size = self.layout(e).size as i64;
                if items.is_empty() {
                    let zero = self.i64c(0);
                    let null = self
                        .emit(
                            Op::Cast(CastKind::IntToPtr, zero, IrTy::Ptr),
                            Some(IrTy::Ptr),
                            span,
                        )
                        .unwrap_or(dst);
                    self.store_at(dst, 0, null);
                    self.store_at(dst, 8, zero);
                    self.store_at(dst, 16, zero);
                    return;
                }
                let bytes = self.i64c(size.max(1) * items.len() as i64);
                let buf = self
                    .emit(Op::Alloc(bytes), Some(IrTy::Ptr), span)
                    .unwrap_or(dst);
                for (i, it) in items.iter().enumerate() {
                    let p = self.addr(buf, (i as i64 * size) as u32);
                    self.emit_cv(it, p, e, span);
                }
                self.store_at(dst, 0, buf);
                let n = self.i64c(items.len() as i64);
                self.store_at(dst, 8, n);
                self.store_at(dst, 16, n);
            }
            (Cv::Fields(items), TyKind::Array(e, _)) => {
                let size = self.layout(e).size;
                for (i, it) in items.iter().enumerate() {
                    let p = self.addr(dst, i as u32 * size);
                    self.emit_cv(it, p, e, span);
                }
            }
            (Cv::Fields(items), _) => {
                let fields = self.struct_fields(t);
                for (f, it) in fields.iter().zip(items) {
                    let p = self.addr(dst, f.off);
                    self.emit_cv(it, p, f.ty, span);
                }
            }
            (Cv::Boxed(inner), TyKind::Box(e)) => {
                let size = self.layout(e).size.max(1) as i64;
                let n = self.i64c(size);
                if let Some(p) = self.emit(Op::Alloc(n), Some(IrTy::Ptr), span) {
                    self.emit_cv(inner, p, e, span);
                    self.store_at(dst, 0, p);
                }
            }
            (Cv::Entries(items), TyKind::Map(..) | TyKind::Set(_)) => {
                let zero = self.i64c(0);
                if let Some(null) = self.emit(
                    Op::Cast(CastKind::IntToPtr, zero, IrTy::Ptr),
                    Some(IrTy::Ptr),
                    span,
                ) {
                    self.store_at(dst, 0, null);
                    self.store_at(dst, 24, null);
                }
                self.store_at(dst, 8, zero);
                self.store_at(dst, 16, zero);
                let (k, v) = match self.kind(t) {
                    TyKind::Map(k, v) => (k, Some(v)),
                    TyKind::Set(k) => (k, None),
                    _ => return,
                };
                for (key, val) in items {
                    let kt = self.slot_for(k);
                    self.emit_cv(key, kt, k, span);
                    match (v, val) {
                        (Some(v), Some(val)) => {
                            let vt = self.slot_for(v);
                            self.emit_cv(val, vt, v, span);
                            self.call_core_g("rt_map_put", &[k, v], vec![dst, kt, vt]);
                        }
                        _ => {
                            self.call_core_g("rt_set_put", &[k], vec![dst, kt]);
                        }
                    }
                }
            }
            (Cv::Case(tag, items), _) => {
                let Some(info) = self.enum_info(t) else {
                    return;
                };
                let tg = self.konst(info.tag, Const::Int(*tag as i128), span);
                self.store_scalar_ty(dst, tg);
                if let Some(fields) = info.variants.get(*tag) {
                    for (f, it) in fields.clone().iter().zip(items) {
                        let p = self.addr(dst, f.off);
                        self.emit_cv(it, p, f.ty, span);
                    }
                }
            }
            _ => {}
        }
    }

    pub(crate) fn lower_meta_job(&mut self, job: MetaJob<'a>) {
        let name = self.module.funcs[job.id.0 as usize].name.clone();
        let placeholder = self.module.funcs[job.id.0 as usize].clone();
        let void = self.types.void;
        let mut state = FnState::empty(void);
        state.func = Func::new(&name, &placeholder.params, placeholder.ret);
        state.file = job.file;
        state.cur = Some(state.func.entry());
        state.subst = job.subst.clone();
        state.ret_ty = job.ty;
        self.f = state;
        let entry = self.f.func.entry();
        let ps = self.f.func.block_params(entry).to_vec();
        self.push_scope();
        match self.repr(job.ty) {
            Repr::Mem(_) => {
                self.f.sret = Some(ps[0]);
                if !matches!(self.into(job.expr, ps[0], job.ty), Res::Never) {
                    self.pop_scope();
                    self.terminate(Term::Return(None));
                }
            }
            Repr::Scalar(_) => {
                if let Some(v) = self.consume_scalar(job.expr) {
                    self.pop_scope();
                    self.terminate(Term::Return(Some(v)));
                }
            }
            Repr::Zst => {
                self.expr(job.expr);
                self.pop_scope();
                self.terminate(Term::Return(None));
            }
        }
        let st = std::mem::replace(&mut self.f, FnState::empty(void));
        if let Some((span, msg)) = st.fail {
            self.diags.push(Diagnostic {
                module: job.file,
                span,
                message: format!("not lowered yet: {msg} (in compile-time code)"),
                warning: true,
            });
        } else {
            let mut func = st.func;
            func.compact();
            self.module.funcs[job.id.0 as usize] = func;
        }
    }
}
