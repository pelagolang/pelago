//! Lists, text and printing: literals, loops over lists, joining and comparing text, `print`.
//!
//! The work is done by library functions written in the language (`rt_str_push`, `rt_show_int`, ...)
//! that the lowering calls by name.

use super::*;

impl<'a> Lowerer<'a> {
    // ---- Calling the library by name --------------------------------------------------------------------------------

    pub(crate) fn core_fn(&mut self, name: &str) -> Option<DefId> {
        if let Some(d) = self.core_fns.get(name) {
            return Some(*d);
        }
        let found = self.fns.iter().find_map(|(d, e)| {
            let in_core = self
                .program
                .package(self.program.module(e.file).package)
                .name
                == "core";
            (in_core && self.res.def(*d).parent.is_none() && self.res.def(*d).name == name)
                .then_some(*d)
        })?;
        self.core_fns.insert(name.to_string(), found);
        Some(found)
    }

    /// Calls a library function whose parameters are all scalars; gives its result, if any.
    pub(crate) fn call_core(&mut self, name: &str, args: Vec<Value>) -> Option<Value> {
        let Some(def) = self.core_fn(name) else {
            self.unsupported(Span::new(0, 0), &format!("the library function `{name}`"));
            return None;
        };
        let fid = self.request_fn(def, &HashMap::new())?;
        let ret = self.module.funcs[fid.0 as usize].ret;
        let r = if ret == IrTy::Void { None } else { Some(ret) };
        self.emit(Op::Call(fid, args), r, Span::new(0, 0))
    }

    /// Calls a generic library function with its type arguments (in declaration order).
    pub(crate) fn call_core_g(
        &mut self,
        name: &str,
        tys: &[Ty],
        args: Vec<Value>,
    ) -> Option<Value> {
        let Some(def) = self.core_fn(name) else {
            self.unsupported(Span::new(0, 0), &format!("the library function `{name}`"));
            return None;
        };
        let gens = self
            .typed
            .fn_infos
            .get(&def)
            .map(|i| i.generics.clone())
            .unwrap_or_default();
        let map: HashMap<DefId, Ty> = gens.iter().copied().zip(tys.iter().copied()).collect();
        let fid = self.request_fn(def, &map)?;
        let ret = self.module.funcs[fid.0 as usize].ret;
        let r = if ret == IrTy::Void { None } else { Some(ret) };
        self.emit(Op::Call(fid, args), r, Span::new(0, 0))
    }

    // ---- Text ----------------------------------------------------------------------------------------------------------

    /// The bytes of a string literal part, with escapes resolved.
    fn unescape(&self, text: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cs = text.chars().peekable();
        while let Some(c) = cs.next() {
            if c != '\\' {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                continue;
            }
            match cs.next() {
                Some('n') => out.push(b'\n'),
                Some('t') => out.push(b'\t'),
                Some('r') => out.push(b'\r'),
                Some('0') => out.push(0),
                Some('u') => {
                    let mut hex = String::new();
                    if cs.peek() == Some(&'{') {
                        cs.next();
                        for h in cs.by_ref() {
                            if h == '}' {
                                break;
                            }
                            hex.push(h);
                        }
                    }
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    }
                }
                Some(c) => {
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
                None => out.push(b'\\'),
            }
        }
        out
    }

    /// The address and length of static bytes.
    pub(crate) fn static_bytes(&mut self, bytes: &[u8]) -> (Value, Value) {
        let d = self.module.add_data(bytes);
        let p = self
            .emit(Op::DataAddr(d), Some(IrTy::Ptr), Span::new(0, 0))
            .unwrap_or(Value(0));
        let n = self.i64c(bytes.len() as i64);
        (p, n)
    }

    pub(crate) fn push_static(&mut self, out: Value, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let (p, n) = self.static_bytes(bytes);
        self.call_core("rt_str_push", vec![out, p, n]);
    }

    pub(crate) fn string_lit(
        &mut self,
        e: &'a Expr,
        parts: &'a [StrPart],
        dst: Value,
        t: Ty,
    ) -> Res {
        let interpolated = parts.iter().any(|p| matches!(p, StrPart::Interp(_)));
        if !interpolated {
            let mut bytes = Vec::new();
            for p in parts {
                if let StrPart::Text(sp) = p {
                    bytes.extend(self.unescape(self.src_text(*sp)));
                }
            }
            let (p, n) = self.static_bytes(&bytes);
            self.store_at(dst, 0, p);
            self.store_at(dst, 8, n);
            if matches!(self.kind(t), TyKind::String) {
                let z = self.i64c(0);
                self.store_at(dst, 16, z);
            }
            return Res::Unit;
        }
        // Interpolation builds a new string piece by piece.
        let zp = self.i64c(0);
        let nullp = self
            .emit(
                Op::Cast(CastKind::IntToPtr, zp, IrTy::Ptr),
                Some(IrTy::Ptr),
                e.span,
            )
            .unwrap_or(dst);
        self.store_at(dst, 0, nullp);
        let z1 = self.i64c(0);
        self.store_at(dst, 8, z1);
        let z2 = self.i64c(0);
        self.store_at(dst, 16, z2);
        // If a part stops the program, nothing leaks that matters; otherwise own the buffer now.
        for p in parts {
            match p {
                StrPart::Text(sp) => {
                    let bytes = self.unescape(self.src_text(*sp));
                    self.push_static(dst, &bytes);
                }
                StrPart::Interp(x) => {
                    if !self.show_expr(x, dst) {
                        return Res::Never;
                    }
                }
            }
        }
        Res::Unit
    }

    /// Appends the text of an expression's value to the string at `out`.
    pub(crate) fn show_expr(&mut self, x: &'a Expr, out: Value) -> bool {
        let Some(t) = self.ty_of(x) else {
            self.unsupported(x.span, "printing a value of unknown type");
            return false;
        };
        match self.repr(t) {
            Repr::Scalar(_) => {
                let Some(v) = self.scalar(x) else {
                    return false;
                };
                self.show_scalar(v, t, out)
            }
            Repr::Mem(_) => {
                let Res::Val(p) = self.expr(x) else {
                    return false;
                };
                self.show_at(p, t, out, false)
            }
            Repr::Zst => {
                self.expr(x);
                true
            }
        }
    }

    /// Appends the text of a scalar value.
    pub(crate) fn show_scalar(&mut self, v: Value, t: Ty, out: Value) -> bool {
        match self.kind(t) {
            TyKind::Int(k) => {
                let signed = k.signed();
                let w = self.resize(v, IrTy::I64, signed, Span::new(0, 0));
                self.call_core("rt_show_int", vec![out, w]);
                true
            }
            TyKind::Float(_) => {
                let w = if self.vty(v) == IrTy::F32 {
                    self.emit(
                        Op::Cast(CastKind::FloatResize, v, IrTy::F64),
                        Some(IrTy::F64),
                        Span::new(0, 0),
                    )
                    .unwrap_or(v)
                } else {
                    v
                };
                self.call_core("rt_show_float", vec![out, w]);
                true
            }
            TyKind::Bool => {
                self.call_core("rt_show_bool", vec![out, v]);
                true
            }
            TyKind::Char => {
                self.call_core("rt_push_char", vec![out, v]);
                true
            }
            _ => {
                // Boxes, objects and raw pointers go through the glue (they need an address).
                let slot = self.slot_for(t);
                self.store_scalar_ty(slot, v);
                self.show_at(slot, t, out, false)
            }
        }
    }

    /// Appends the text of the value at `p` (a scalar is loaded). `quoted` wraps text in quotes, as
    /// inside a list.
    pub(crate) fn show_at(&mut self, p: Value, t: Ty, out: Value, quoted: bool) -> bool {
        match self.kind(t) {
            TyKind::String | TyKind::Str => {
                if quoted {
                    self.push_static(out, b"\"");
                }
                let d = self.load_at(p, 0, IrTy::Ptr);
                let n = self.load_at(p, 8, IrTy::I64);
                self.call_core("rt_str_push", vec![out, d, n]);
                if quoted {
                    self.push_static(out, b"\"");
                }
                true
            }
            TyKind::Char if quoted => {
                self.push_static(out, b"'");
                let c = self.load_at(p, 0, IrTy::Int(32));
                self.call_core("rt_push_char", vec![out, c]);
                self.push_static(out, b"'");
                true
            }
            TyKind::Int(_) | TyKind::Float(_) | TyKind::Bool | TyKind::Char => {
                let s = self.scalar_of(t).unwrap_or(IrTy::I64);
                let v = self.load_at(p, 0, s);
                self.show_scalar(v, t, out)
            }
            TyKind::Void | TyKind::Never => {
                self.push_static(out, b"()");
                true
            }
            _ => match self.glue_fn(GlueKind::Show, t) {
                Some(g) => {
                    self.emit(Op::Call(g, vec![p, out]), None, Span::new(0, 0));
                    true
                }
                None => {
                    self.unsupported(Span::new(0, 0), "printing this type");
                    false
                }
            },
        }
    }

    pub(crate) fn print_call(&mut self, e: &'a Expr, args: &'a [Arg]) -> Res {
        // Each value is shown into one line.
        let line = self.slot_bytes(24, 8);
        let z = self.i64c(0);
        let nullp = self
            .emit(
                Op::Cast(CastKind::IntToPtr, z, IrTy::Ptr),
                Some(IrTy::Ptr),
                e.span,
            )
            .unwrap_or(line);
        self.store_at(line, 0, nullp);
        let z1 = self.i64c(0);
        self.store_at(line, 8, z1);
        let z2 = self.i64c(0);
        self.store_at(line, 16, z2);
        let string = self.types.string;
        self.own(line, string, false);
        for (i, a) in args.iter().enumerate() {
            if i > 0 {
                self.push_static(line, b" ");
            }
            if !self.show_expr(&a.value, line) {
                return Res::Never;
            }
        }
        self.push_static(line, b"\n");
        let d = self.load_at(line, 0, IrTy::Ptr);
        let n = self.load_at(line, 8, IrTy::I64);
        self.emit(Op::Print(d, n), None, e.span);
        Res::Unit
    }

    pub(crate) fn concat(&mut self, e: &'a Expr, l: &'a Expr, r: &'a Expr, dst: Value) -> Res {
        let _ = e;
        let Res::Val(a) = self.expr(l) else {
            return Res::Never;
        };
        let Res::Val(b) = self.expr(r) else {
            return Res::Never;
        };
        self.call_core("rt_str_concat", vec![dst, a, b]);
        Res::Unit
    }

    /// `s = s + a + b`, where `s` is a local string and the operands are simple, is `s += a; s += b`:
    /// appending in place keeps the buffer's spare capacity, where a fresh copy per round makes a loop
    /// quadratic (and, for big strings, one `mmap` and `munmap` per round). `None` when the pattern
    /// does not apply and the ordinary assignment should run.
    ///
    /// The operands must not read `s` and must not run code that could change it, so each is limited to
    /// literals, other names, fields, indexing, operators and `#(...)` parts made of the same.
    pub(crate) fn append_chain(
        &mut self,
        p: Value,
        lhs: &'a Expr,
        rhs: &'a Expr,
        lt: Ty,
    ) -> Option<Res> {
        use super::expr::strip_parens;
        if !matches!(self.kind(lt), TyKind::String) {
            return None;
        }
        let (def, path) = self.place_path(lhs)?;
        if !self.f.locals.contains_key(&def) {
            return None;
        }
        // Flatten `((s + a) + b)` into the operands `a`, `b`.
        let mut ops: Vec<&'a Expr> = Vec::new();
        let mut cur = strip_parens(rhs);
        while let ExprKind::Binary {
            op: Punct::Plus,
            lhs: l,
            rhs: r,
        } = &cur.kind
        {
            ops.push(r);
            cur = strip_parens(l);
        }
        if ops.is_empty() || self.place_path(cur) != Some((def, path.clone())) {
            return None;
        }
        ops.reverse();
        for o in &ops {
            let ot = self.ty_of(o)?;
            if !self.is_text(ot) || !self.simple_operand(o, def, &path) {
                return None;
            }
        }
        for o in ops {
            let Res::Val(b) = self.expr(o) else {
                return Some(Res::Never);
            };
            let d = self.load_at(b, 0, IrTy::Ptr);
            let n = self.load_at(b, 8, IrTy::I64);
            self.call_core("rt_str_push", vec![p, d, n]);
        }
        Some(Res::Unit)
    }

    /// The variable and the field names `e` spells out, for `s` and `s.a.b`.
    fn place_path(&self, e: &Expr) -> Option<(DefId, Vec<String>)> {
        use super::expr::strip_parens;
        match &strip_parens(e).kind {
            ExprKind::Name(id) => Some((self.res.use_at(self.f.file, id.span.start)?, Vec::new())),
            ExprKind::Field {
                base,
                name,
                optional: false,
            } => {
                let (d, mut p) = self.place_path(base)?;
                p.push(self.src_text(name.span).to_string());
                Some((d, p))
            }
            _ => None,
        }
    }

    /// Does `e` avoid the place `def.path` (and anything inside or around it), and any closure or
    /// assignment? Calls are allowed when they name a plain function and pass plain values.
    fn simple_operand(&self, e: &Expr, def: DefId, path: &[String]) -> bool {
        use super::expr::strip_parens;
        if let Some((d, p)) = self.place_path(e) {
            let n = p.len().min(path.len());
            return d != def || p[..n] != path[..n];
        }
        match &strip_parens(e).kind {
            ExprKind::Int | ExprKind::Float | ExprKind::Char | ExprKind::Bool(_) => true,
            ExprKind::Str(parts) => parts.iter().all(|part| match part {
                StrPart::Text(_) => true,
                StrPart::Interp(x) => self.simple_operand(x, def, path),
            }),
            ExprKind::Field { base, .. } | ExprKind::TupleIndex { base, .. } => {
                self.simple_operand(base, def, path)
            }
            ExprKind::Index { base, index } => {
                self.simple_operand(base, def, path) && self.simple_operand(index, def, path)
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                self.simple_operand(lhs, def, path) && self.simple_operand(rhs, def, path)
            }
            ExprKind::Unary { operand, .. } => self.simple_operand(operand, def, path),
            ExprKind::Call {
                callee,
                args,
                mark: CallMark::None,
            } => {
                let ExprKind::Name(id) = &strip_parens(callee).kind else {
                    return false;
                };
                let Some(d) = self.res.use_at(self.f.file, id.span.start) else {
                    return false;
                };
                !self.f.locals.contains_key(&d)
                    && args.iter().all(|a| {
                        a.mode.is_none() && !a.spread && self.simple_operand(&a.value, def, path)
                    })
            }
            _ => false,
        }
    }

    /// `s += text`
    pub(crate) fn append_assign(
        &mut self,
        e: &'a Expr,
        op: Punct,
        p: Value,
        t: Ty,
        rhs: &'a Expr,
    ) -> Res {
        if op == Punct::PlusEq && matches!(self.kind(t), TyKind::String) {
            let Res::Val(b) = self.expr(rhs) else {
                return Res::Never;
            };
            let d = self.load_at(b, 0, IrTy::Ptr);
            let n = self.load_at(b, 8, IrTy::I64);
            self.call_core("rt_str_push", vec![p, d, n]);
            return Res::Unit;
        }
        self.unsupported(e.span, "this assignment")
    }

    /// Is the text at `ptr` equal to a literal? (For patterns.)
    pub(crate) fn text_equals_literal(
        &mut self,
        ptr: Value,
        ty: Ty,
        lit: &'a Expr,
    ) -> Option<Value> {
        let _ = ty;
        let Res::Val(l) = self.expr(lit) else {
            return None;
        };
        self.call_core("rt_str_eq", vec![ptr, l])
    }

    // ---- Comparison of memory values ----------------------------------------------------------------------------------------

    pub(crate) fn compare_mem(
        &mut self,
        e: &'a Expr,
        op: Punct,
        lhs: &'a Expr,
        rhs: &'a Expr,
        lt: Ty,
    ) -> Res {
        // The right side may need converting to the left's type (a value into an option).
        let rt = self.ty_of(rhs).unwrap_or(lt);
        let Some(a) = self.operand_addr(lhs, lt) else {
            return Res::Never;
        };
        let b = if rt == lt || self.is_text(rt) && self.is_text(lt) {
            match self.operand_addr(rhs, rt) {
                Some(b) => b,
                _ => return Res::Never,
            }
        } else {
            let tmp = self.slot_for(lt);
            if let Res::Never = self.into(rhs, tmp, lt) {
                return Res::Never;
            }
            self.own(tmp, lt, false);
            tmp
        };
        let ordering = !matches!(op, Punct::EqEq | Punct::NotEq);
        let text = self.is_text(lt);
        let cmp_val = if ordering || text && false {
            None
        } else {
            Some(())
        };
        let _ = cmp_val;
        if !ordering {
            let eq = if text {
                self.call_core("rt_str_eq", vec![a, b])
            } else {
                match self.glue_fn(GlueKind::Eq, lt) {
                    Some(g) => self.emit(Op::Call(g, vec![a, b]), Some(IrTy::Bool), e.span),
                    None => return self.unsupported(e.span, "comparing this type"),
                }
            };
            let Some(eq) = eq else { return Res::Never };
            if op == Punct::NotEq {
                return self.wrap(Op::Not(eq), IrTy::Bool, e.span);
            }
            return Res::Val(eq);
        }
        let c = if text {
            self.call_core("rt_str_cmp", vec![a, b])
        } else {
            match self.glue_fn(GlueKind::Cmp, lt) {
                Some(g) => self.emit(Op::Call(g, vec![a, b]), Some(IrTy::I64), e.span),
                None => return self.unsupported(e.span, "ordering this type"),
            }
        };
        let Some(c) = c else { return Res::Never };
        let zero = self.i64c(0);
        let o = match op {
            Punct::Lt => CmpOp::Lt,
            Punct::LtEq => CmpOp::Le,
            Punct::Gt => CmpOp::Gt,
            _ => CmpOp::Ge,
        };
        self.wrap(Op::Cmp(o, c, zero), IrTy::Bool, e.span)
    }

    /// The address of an operand's value; a scalar (a box) is first stored in a temporary.
    fn operand_addr(&mut self, e: &'a Expr, t: Ty) -> Option<Value> {
        if let Repr::Scalar(_) = self.repr(t) {
            let v = self.scalar(e)?;
            let slot = self.slot_for(t);
            self.store_scalar_ty(slot, v);
            return Some(slot);
        }
        match self.expr(e) {
            Res::Val(p) => Some(p),
            _ => None,
        }
    }

    pub(crate) fn is_text(&self, t: Ty) -> bool {
        matches!(self.types.kind_of(t), TyKind::String | TyKind::Str)
    }

    // ---- Lists ----------------------------------------------------------------------------------------------------------------------

    pub(crate) fn list_lit(
        &mut self,
        e: &'a Expr,
        items: &'a [ListItem],
        dst: Value,
        t: Ty,
    ) -> Res {
        if let TyKind::Array(elem, _) = self.kind(t) {
            let esize = self.layout(elem).size;
            for (i, it) in items.iter().enumerate() {
                let p = self.addr(dst, i as u32 * esize);
                if let Res::Never = self.into(&it.value, p, elem) {
                    return Res::Never;
                }
            }
            return Res::Unit;
        }
        let elem = match self.kind(t) {
            TyKind::List(x) | TyKind::Iter(x) => x,
            _ => return self.unsupported(e.span, "a list literal of this type"),
        };
        if items.iter().any(|i| i.spread) {
            return self.list_with_spreads(e, items, dst, t, elem);
        }
        let exprs: Vec<&'a Expr> = items.iter().map(|i| &i.value).collect();
        self.list_from(e.span, &exprs, dst, t)
    }

    /// A list of exactly these values.
    pub(crate) fn list_from(&mut self, span: Span, items: &[&'a Expr], dst: Value, t: Ty) -> Res {
        let elem = match self.kind(t) {
            TyKind::List(x) | TyKind::Iter(x) => x,
            _ => return self.unsupported(span, "a list of this type"),
        };
        let n = items.len() as i64;
        let esize = self.layout(elem).size as i64;
        let (buf, cap) = if n == 0 {
            let z = self.i64c(0);
            let np = self
                .emit(
                    Op::Cast(CastKind::IntToPtr, z, IrTy::Ptr),
                    Some(IrTy::Ptr),
                    span,
                )
                .unwrap_or(dst);
            (np, self.i64c(0))
        } else {
            let bytes = self.i64c((n * esize).max(1));
            let b = self
                .emit(Op::Alloc(bytes), Some(IrTy::Ptr), span)
                .unwrap_or(dst);
            (b, self.i64c(n))
        };
        // The list is complete (and owned) only at the end; elements that fail midway are not tracked.
        for (i, it) in items.iter().enumerate() {
            let p = self.addr(buf, (i as i64 * esize) as u32);
            if let Res::Never = self.into(it, p, elem) {
                return Res::Never;
            }
        }
        let len = self.i64c(n);
        self.store_at(dst, 0, buf);
        self.store_at(dst, 8, len);
        self.store_at(dst, 16, cap);
        Res::Unit
    }

    /// `[a, ...xs, b]`: an empty list that grows.
    fn list_with_spreads(
        &mut self,
        e: &'a Expr,
        items: &'a [ListItem],
        dst: Value,
        _t: Ty,
        elem: Ty,
    ) -> Res {
        let z = self.i64c(0);
        let np = self
            .emit(
                Op::Cast(CastKind::IntToPtr, z, IrTy::Ptr),
                Some(IrTy::Ptr),
                e.span,
            )
            .unwrap_or(dst);
        self.store_at(dst, 0, np);
        let z1 = self.i64c(0);
        self.store_at(dst, 8, z1);
        let z2 = self.i64c(0);
        self.store_at(dst, 16, z2);
        let esize = self.layout(elem).size as i64;
        for it in items {
            if it.spread {
                let Res::Val(src) = self.expr(&it.value) else {
                    return Res::Never;
                };
                self.call_core_g("rt_list_spread", &[elem], vec![dst, src]);
                continue;
            }
            // Push one element.
            let len = self.load_at(dst, 8, IrTy::I64);
            let one = self.i64c(1);
            let need = self
                .emit(Op::Bin(BinOp::Add, len, one), Some(IrTy::I64), e.span)
                .unwrap_or(len);
            let sz = self.i64c(esize);
            self.call_core("rt_grow", vec![dst, sz, need]);
            let data = self.load_at(dst, 0, IrTy::Ptr);
            let off = self
                .emit(Op::Bin(BinOp::Mul, len, sz), Some(IrTy::I64), e.span)
                .unwrap_or(len);
            let p = self
                .emit(Op::PtrAdd(data, off), Some(IrTy::Ptr), e.span)
                .unwrap_or(data);
            if let Res::Never = self.into(&it.value, p, elem) {
                return Res::Never;
            }
            self.store_at(dst, 8, need);
        }
        Res::Unit
    }

    pub(crate) fn repeat_lit(
        &mut self,
        e: &'a Expr,
        value: &'a Expr,
        count: &'a Expr,
        dst: Value,
        t: Ty,
    ) -> Res {
        if let TyKind::Array(elem, len) = self.kind(t) {
            // An array lives in place: the value is cloned into each slot.
            let tmp = self.slot_for(elem);
            if let Res::Never = self.into(value, tmp, elem) {
                return Res::Never;
            }
            let n = self.array_len(len);
            let nv = self.i64c(n);
            let esize = self.layout(elem).size;
            self.each_elem(dst, nv, esize, |s, p, _| s.clone_into(tmp, p, elem));
            self.drop_at(tmp, elem);
            return Res::Unit;
        }
        let elem = match self.kind(t) {
            TyKind::List(x) => x,
            _ => return self.unsupported(e.span, "a repeated list of this type"),
        };
        let Some(n) = self.scalar(count) else {
            return Res::Never;
        };
        let esize = self.layout(elem).size as i64;
        let sz = self.i64c(esize.max(1));
        let bytes = self
            .emit(Op::Bin(BinOp::Mul, n, sz), Some(IrTy::I64), e.span)
            .unwrap_or(n);
        let buf = self
            .emit(Op::Alloc(bytes), Some(IrTy::Ptr), e.span)
            .unwrap_or(dst);
        // The value is built once and cloned into each place.
        let tmp = self.slot_for(elem);
        if let Res::Never = self.into(value, tmp, elem) {
            return Res::Never;
        }
        let head = self.new_block(&[IrTy::I64]);
        let body = self.new_block(&[]);
        let done = self.new_block(&[]);
        let zero = self.i64c(0);
        self.jump(head, vec![zero]);
        self.start(head);
        let i = self.f.func.block_params(head)[0];
        if let Some(c) = self.emit(Op::Cmp(CmpOp::Lt, i, n), Some(IrTy::Bool), e.span) {
            self.branch(c, body, done);
        }
        self.start(body);
        let off = self
            .emit(Op::Bin(BinOp::Mul, i, sz), Some(IrTy::I64), e.span)
            .unwrap_or(i);
        let p = self
            .emit(Op::PtrAdd(buf, off), Some(IrTy::Ptr), e.span)
            .unwrap_or(buf);
        self.clone_into(tmp, p, elem);
        let one = self.i64c(1);
        let next = self
            .emit(Op::Bin(BinOp::Add, i, one), Some(IrTy::I64), e.span)
            .unwrap_or(i);
        self.jump(head, vec![next]);
        self.start(done);
        // The template itself is dropped.
        self.drop_at(tmp, elem);
        self.store_at(dst, 0, buf);
        self.store_at(dst, 8, n);
        self.store_at(dst, 16, n);
        Res::Unit
    }

    /// `xs[a..b]` and `s[a..b]`: a view of part of a list or of text.
    pub(crate) fn slice_expr(&mut self, e: &'a Expr, dst: Value, t: Ty) -> Res {
        let ExprKind::Index { base, index } = &e.kind else {
            return self.unsupported(e.span, "this slice");
        };
        let ExprKind::Range {
            start,
            end,
            inclusive,
            ..
        } = &index.kind
        else {
            return self.unsupported(e.span, "this slice");
        };
        let Some(bt) = self.ty_of(base) else {
            return self.unsupported(e.span, "slicing a value of unknown type");
        };
        let esize = match self.kind(bt) {
            TyKind::String | TyKind::Str => 1,
            TyKind::List(x) | TyKind::Iter(x) => self.layout(x).size as i64,
            _ => return self.unsupported(e.span, "slicing this type"),
        };
        let Res::Val(h) = self.expr(base) else {
            return Res::Never;
        };
        let len = self.load_at(h, 8, IrTy::I64);
        let lo = match start {
            Some(s) => match self.scalar(s) {
                Some(v) => v,
                None => return Res::Never,
            },
            None => self.i64c(0),
        };
        let hi = match end {
            Some(x) => match self.scalar(x) {
                Some(v) => {
                    if *inclusive {
                        let one = self.i64c(1);
                        self.emit(Op::Bin(BinOp::Add, v, one), Some(IrTy::I64), e.span)
                            .unwrap_or(v)
                    } else {
                        v
                    }
                }
                None => return Res::Never,
            },
            None => len,
        };
        // 0 <= lo <= hi <= len, compared as unsigned numbers.
        if let Some(bad) = self.emit(Op::Cmp(CmpOp::UGt, hi, len), Some(IrTy::Bool), e.span) {
            self.stop_if(bad, "slice out of range");
        }
        if let Some(bad) = self.emit(Op::Cmp(CmpOp::UGt, lo, hi), Some(IrTy::Bool), e.span) {
            self.stop_if(bad, "slice out of range");
        }
        let data = self.load_at(h, 0, IrTy::Ptr);
        let sz = self.i64c(esize);
        let off = self
            .emit(Op::Bin(BinOp::Mul, lo, sz), Some(IrTy::I64), e.span)
            .unwrap_or(lo);
        let p = self
            .emit(Op::PtrAdd(data, off), Some(IrTy::Ptr), e.span)
            .unwrap_or(data);
        let n = self
            .emit(Op::Bin(BinOp::Sub, hi, lo), Some(IrTy::I64), e.span)
            .unwrap_or(hi);
        self.store_at(dst, 0, p);
        self.store_at(dst, 8, n);
        // A list slice is a list that owns nothing; text slices are `str`.
        if self.fields_len(t) == 3 {
            let z = self.i64c(0);
            self.store_at(dst, 16, z);
        }
        Res::Unit
    }

    fn fields_len(&mut self, t: Ty) -> usize {
        self.struct_fields(t).len()
    }

    // ---- Loops over lists --------------------------------------------------------------------------------------------------------------

    pub(crate) fn for_each(
        &mut self,
        label: Option<String>,
        pats: &'a [Pattern],
        iter: &'a Expr,
        body: &mut LoopBody<'a, '_>,
        keys_only: bool,
    ) -> Res {
        let Some(it) = self.ty_of(iter) else {
            return self.unsupported(iter.span, "looping over a value of unknown type");
        };
        let mut val_ty = None;
        let mut array_len = None;
        let mut soa_elem = None;
        let elem = match self.kind(it) {
            TyKind::Soa(x, _) => {
                soa_elem = Some(x);
                x
            }
            TyKind::List(x) | TyKind::Iter(x) | TyKind::Set(x) => x,
            TyKind::Array(x, len) => {
                array_len = Some(self.array_len(len));
                x
            }
            TyKind::Map(k, v) => {
                val_ty = Some(v);
                k
            }
            _ => return self.unsupported(iter.span, "looping over this"),
        };
        self.push_scope();
        let Res::Val(h) = self.expr(iter) else {
            self.f.scopes.pop();
            return Res::Never;
        };
        let esize = self.layout(elem).size as i64;
        let (len, data) = match array_len {
            Some(n) => (self.i64c(n), h),
            None => (self.load_at(h, 8, IrTy::I64), self.load_at(h, 0, IrTy::Ptr)),
        };
        let head = self.new_block(&[IrTy::I64]);
        let body_b = self.new_block(&[]);
        let step = self.new_block(&[]);
        let exit = self.new_block(&[]);
        let zero = self.i64c(0);
        self.jump(head, vec![zero]);
        self.start(head);
        let i = self.f.func.block_params(head)[0];
        if let Some(c) = self.emit(Op::Cmp(CmpOp::Lt, i, len), Some(IrTy::Bool), iter.span) {
            self.branch(c, body_b, exit);
        }
        self.start(body_b);
        let sz = self.i64c(esize);
        let off = self
            .emit(Op::Bin(BinOp::Mul, i, sz), Some(IrTy::I64), iter.span)
            .unwrap_or(i);
        let mut ep = self
            .emit(Op::PtrAdd(data, off), Some(IrTy::Ptr), iter.span)
            .unwrap_or(data);
        // A `soa` element is gathered from the columns for the length of one turn.
        let soa_tmp = soa_elem.map(|x| self.slot_for(x));
        if let (Some(x), Some(tmp)) = (soa_elem, soa_tmp) {
            self.soa_gather(h, i, tmp, x);
            ep = tmp;
        }
        let int = self.types.int;
        let bind_fail = self.stop_block("pattern did not match");
        let depth = self.f.scopes.len();
        // A map's values live after its keys, in the same block.
        let vp = match val_ty {
            Some(vt) if !keys_only => {
                let ksz = self.i64c(esize);
                let cap = self.load_at(h, 16, IrTy::I64);
                let span_ = iter.span;
                let raw = self
                    .emit(Op::Bin(BinOp::Mul, cap, ksz), Some(IrTy::I64), span_)
                    .unwrap_or(cap);
                let seven = self.i64c(7);
                let up = self
                    .emit(Op::Bin(BinOp::Add, raw, seven), Some(IrTy::I64), span_)
                    .unwrap_or(raw);
                let mask = self.i64c(-8);
                let base_off = self
                    .emit(Op::Bin(BinOp::And, up, mask), Some(IrTy::I64), span_)
                    .unwrap_or(up);
                let vsize = self.layout(vt).size as i64;
                let vsz = self.i64c(vsize);
                let voff = self
                    .emit(Op::Bin(BinOp::Mul, i, vsz), Some(IrTy::I64), span_)
                    .unwrap_or(i);
                let tot = self
                    .emit(Op::Bin(BinOp::Add, base_off, voff), Some(IrTy::I64), span_)
                    .unwrap_or(voff);
                let p = self
                    .emit(Op::PtrAdd(data, tot), Some(IrTy::Ptr), span_)
                    .unwrap_or(data);
                Some((p, vt))
            }
            _ => None,
        };
        match (pats, vp) {
            ([p], None) => {
                if !self.pat_test(p, ep, elem, bind_fail) {
                    return Res::Never;
                }
            }
            ([p], Some((vp, vt))) => {
                // The pair `(key, value)` as one tuple value that owns nothing.
                let tt = self.types.tuple(vec![elem, vt]);
                let slot = self.slot_for(tt);
                let fs = self.struct_fields(tt);
                let (k0, v0) = (self.at(slot, fs[0].off), self.at(slot, fs[1].off));
                self.copy_mem(k0, ep, elem);
                self.copy_mem(v0, vp, vt);
                if !self.pat_test(p, slot, tt, bind_fail) {
                    return Res::Never;
                }
            }
            ([pk, pv], Some((vp, vt))) => {
                if !self.pat_test(pk, ep, elem, bind_fail) || !self.pat_test(pv, vp, vt, bind_fail)
                {
                    return Res::Never;
                }
            }
            ([pi, pe], None) => {
                // `for i, x of xs`: the position, then the element.
                let slot = self.slot_for(int);
                self.store_scalar_ty(slot, i);
                if !self.pat_test(pi, slot, int, bind_fail)
                    || !self.pat_test(pe, ep, elem, bind_fail)
                {
                    return Res::Never;
                }
            }
            _ => return self.unsupported(iter.span, "this `for` pattern"),
        }
        self.f.loops.push(LoopCtx {
            label,
            cont: step,
            exit,
            break_ty: None,
            break_dst: None,
            scope_depth: depth,
        });
        let void = self.types.void;
        let r = self.run_body(body, void);
        if !matches!(r, Res::Never) {
            self.jump(step, vec![]);
        }
        self.f.loops.pop();
        self.start(step);
        // `for inout p of points`: what the turn changed goes back into the columns.
        if let (Some(x), Some(tmp)) = (soa_elem, soa_tmp) {
            if pats.iter().any(|p| {
                matches!(
                    p.kind,
                    PatternKind::Binding {
                        mode: Some(Mode::Inout),
                        ..
                    }
                )
            }) {
                self.soa_scatter(h, i, tmp, x);
            }
        }
        let one = self.i64c(1);
        if let Some(next) = self.emit(Op::Bin(BinOp::Add, i, one), Some(IrTy::I64), iter.span) {
            self.jump(head, vec![next]);
        }
        self.start(exit);
        self.pop_scope();
        Res::Unit
    }
}
