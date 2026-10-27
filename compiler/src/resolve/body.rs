//! Phase 3: walking function bodies, blocks, patterns and types, looking every name up.

use super::*;
use crate::keyword::foreign_keyword_hint;
use crate::token::Punct;

impl<'p> Resolver<'p> {
    // ---- Entry --------------------------------------------------------------------------

    pub(super) fn resolve_all_bodies(&mut self) {
        let n = self.scopes.len();
        // File modules; inline modules are handled when their `mod` item is met.
        let files = self.program.modules.len();
        for idx in 0..files.min(n) {
            self.cur_scope = idx;
            self.cur_file = self.scopes[idx].file;
            self.frames.clear();
            let stmts = self.scopes[idx].stmts;
            self.resolve_stmts_toplevel(stmts);
        }
    }

    fn resolve_stmts_toplevel(&mut self, stmts: &'p [Stmt]) {
        for s in stmts {
            match &s.kind {
                StmtKind::Item(i) => self.resolve_item(i),
                StmtKind::Decl(d) => self.resolve_decl_uses(d),
                StmtKind::Expr(e) => self.resolve_expr(e),
            }
        }
    }

    /// A module-level declaration: its names exist already; resolve the type and the value.
    fn resolve_decl_uses(&mut self, d: &'p Decl) {
        if let Some(t) = &d.ty {
            self.resolve_type(t);
        }
        if let Some(e) = &d.init {
            self.resolve_expr(e);
        }
        self.resolve_pattern_uses(&d.pattern);
    }

    /// Walks a pattern that binds nothing new (module-level `const (a, b) = ...`), resolving the
    /// paths and expressions inside it.
    fn resolve_pattern_uses(&mut self, p: &'p Pattern) {
        self.frames.push(Frame {
            names: HashMap::new(),
            barrier: false,
        });
        self.keep_defs = true;
        self.bind_pattern(p, DefKind::Local, false);
        self.keep_defs = false;
        self.frames.pop();
    }

    // ---- Scopes -------------------------------------------------------------------------

    fn push_frame(&mut self, barrier: bool) {
        self.frames.push(Frame {
            names: HashMap::new(),
            barrier,
        });
    }

    fn pop_frame(&mut self) {
        self.frames.pop();
    }

    /// Defines a local name in the innermost frame.
    fn define_local(&mut self, id: &Ident, kind: DefKind, allow_dup: bool) -> DefId {
        if self.keep_defs {
            if let Some(&d) = self.decls.get(&(self.cur_file, id.span.start)) {
                return d;
            }
        }
        let name = self.text(self.cur_file, id).to_string();
        if let Some(frame) = self.frames.last() {
            if let Some(&existing) = frame.names.get(&name) {
                if allow_dup {
                    self.uses.insert((self.cur_file, id.span.start), existing);
                    return existing;
                }
                self.error(
                    id.span,
                    format!("`{name}` is already defined in this scope; pick another name"),
                );
            }
        }
        let def = self.new_def(
            kind,
            &name,
            Some(self.cur_scope),
            id.span,
            Vis::Default,
            None,
        );
        if let Some(frame) = self.frames.last_mut() {
            frame.names.insert(name, def);
        }
        def
    }

    fn define_named(&mut self, name: &str, span: Span, kind: DefKind) -> DefId {
        let def = self.new_def(kind, name, Some(self.cur_scope), span, Vis::Default, None);
        if let Some(frame) = self.frames.last_mut() {
            frame.names.insert(name.to_string(), def);
        }
        def
    }

    fn record_use(&mut self, id: &Ident, def: DefId) {
        self.uses.insert((self.cur_file, id.span.start), def);
    }

    /// Looks a name up. `Err(Some(def))` means a local of an enclosing function (not usable here).
    fn lookup(&self, name: &str) -> Result<Option<DefId>, DefId> {
        let found = self.lookup_exact(name)?;
        if found.is_some() {
            return Ok(found);
        }
        match crate::naming::other_style(name) {
            Some(alt) => self.lookup_exact(&alt),
            None => Ok(None),
        }
    }

    fn lookup_exact(&self, name: &str) -> Result<Option<DefId>, DefId> {
        let mut crossed = false;
        for f in self.frames.iter().rev() {
            if let Some(&d) = f.names.get(name) {
                if crossed && self.defs[d.0 as usize].kind.is_local() {
                    return Err(d);
                }
                return Ok(Some(d));
            }
            if f.barrier {
                crossed = true;
            }
        }
        if let Some(e) = self.scopes[self.cur_scope].names.get(name) {
            return Ok(Some(e.def));
        }
        Ok(self.prelude.get(name).copied())
    }

    fn visible_names(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for f in &self.frames {
            v.extend(f.names.keys().cloned());
        }
        v.extend(self.scopes[self.cur_scope].names.keys().cloned());
        v.extend(self.prelude.keys().cloned());
        v
    }

    fn unknown_name(&mut self, id: &Ident, what: &str) {
        let name = self.text(self.cur_file, id);
        let mut msg = format!("cannot find {what} `{name}` here");
        let names = self.visible_names();
        if let Some(s) = self.suggest(name, names.iter().map(|s| s.as_str())) {
            msg.push_str(&format!("; did you mean `{s}`?"));
        } else if let Some(hint) = foreign_keyword_hint(name) {
            msg.push_str(&format!(": {hint}"));
        }
        self.error(id.span, msg);
    }

    /// Resolves a name used as a value or a type. Records the use.
    fn resolve_name(&mut self, id: &Ident, what: &str) -> Option<DefId> {
        let name = self.text(self.cur_file, id);
        match self.lookup(name) {
            Ok(Some(def)) => {
                // An ambiguous import is an error at the use.
                if let Some(e) = self.scopes[self.cur_scope].names.get(name) {
                    if e.def == def && !e.clash.is_empty() {
                        let mut all = vec![e.def];
                        all.extend(e.clash.iter().copied());
                        let list = all
                            .iter()
                            .map(|d| format!("`{}`", self.def_path(*d)))
                            .collect::<Vec<_>>()
                            .join(" and ");
                        self.error(
                            id.span,
                            format!("`{name}` could be {list}; write the module name in front to choose"),
                        );
                    }
                }
                self.record_use(id, def);
                Some(def)
            }
            Ok(None) => {
                self.unknown_name(id, what);
                None
            }
            Err(outer) => {
                let _ = outer;
                self.error(
                    id.span,
                    format!(
                        "`{name}` belongs to the enclosing function; a nested `fn` cannot use it (use a closure instead)"
                    ),
                );
                None
            }
        }
    }

    fn def_path(&self, d: DefId) -> String {
        let def = &self.defs[d.0 as usize];
        match def.module {
            Some(m) => format!("{}.{}", self.scope_path(m), def.name),
            None => def.name.clone(),
        }
    }

    // ---- Items --------------------------------------------------------------------------

    pub(super) fn resolve_item(&mut self, item: &'p Item) {
        let file = self.cur_file;
        let def = self.item_defs.get(&(file, item.span.start)).copied();
        match &item.kind {
            ItemKind::Mod {
                body: Some(body), ..
            } => {
                if let Some(d) = def {
                    if let Some(inner) = self.defs[d.0 as usize].scope {
                        let saved = (self.cur_scope, std::mem::take(&mut self.frames));
                        self.cur_scope = inner;
                        self.resolve_stmts_toplevel(body);
                        self.cur_scope = saved.0;
                        self.frames = saved.1;
                    }
                }
            }
            ItemKind::Mod { .. } | ItemKind::Import(_) => {}
            ItemKind::Decl(d) => self.resolve_decl_uses(d),
            ItemKind::Fn(f) => self.resolve_fn(f, None),
            ItemKind::Struct(d) | ItemKind::Obj(d) | ItemKind::Interface(d) => {
                self.resolve_type_decl(d, def);
            }
            ItemKind::Enum(e) => self.resolve_enum(e, def),
            ItemKind::Impl(im) => self.resolve_impl(im),
            ItemKind::TypeAlias { generics, ty, .. } => {
                self.push_frame(false);
                self.define_generics(generics);
                self.resolve_type(ty);
                self.pop_frame();
            }
            ItemKind::Test { body, .. } => {
                self.resolve_block(body);
            }
        }
    }

    fn define_generics(&mut self, g: &'p Generics) {
        for p in &g.params {
            self.define_local(&p.name, DefKind::GenericParam, false);
        }
        for p in &g.params {
            for b in &p.bounds {
                self.resolve_bound(b);
            }
        }
    }

    /// A bound is an interface, or the word `linear` (the type parameter may hold a `linear` value).
    fn resolve_bound(&mut self, b: &'p Type) {
        if let TypeKind::Path { path, args } = &b.kind {
            if args.is_empty() && path.segments.len() == 1 {
                let id = &path.segments[0];
                if self.text(self.cur_file, id) == "linear" {
                    return;
                }
            }
        }
        self.resolve_type(b);
    }

    fn resolve_where(&mut self, w: &'p [WherePred]) {
        for p in w {
            self.resolve_type(&p.ty);
            for b in &p.bounds {
                self.resolve_bound(b);
            }
        }
    }

    fn resolve_fn(&mut self, f: &'p FnDecl, _owner: Option<DefId>) {
        self.push_frame(false);
        self.define_generics(&f.generics);
        for p in &f.params {
            if let Some(t) = &p.ty {
                self.resolve_type(t);
            }
            if let Some(d) = &p.default {
                self.resolve_expr(d);
            }
        }
        if let Some(r) = &f.ret {
            self.resolve_type(r);
        }
        self.resolve_where(&f.where_);
        // Parameters and the body: a barrier, so a nested fn cannot reach these.
        self.push_frame(true);
        let mut self_def = None;
        if f.kind != FnKind::Fn {
            // `init` and `deinit` work on the value being made or dropped, called `self`.
            self_def = Some(self.define_named("self", f.name.span, DefKind::Param));
        }
        for p in &f.params {
            if p.is_self {
                let d = self.define_named("self", p.name.span, DefKind::Param);
                self_def = Some(d);
            } else {
                self.define_local(&p.name, DefKind::Param, false);
            }
        }
        self.self_param.push(self_def);
        match &f.body {
            FnBody::Block(b) => self.resolve_block(b),
            FnBody::Expr(e) => self.resolve_expr(e),
            FnBody::None => {}
        }
        self.self_param.pop();
        self.pop_frame();
        self.pop_frame();
    }

    fn resolve_type_decl(&mut self, d: &'p TypeDecl, def: Option<DefId>) {
        self.self_ty.push(def);
        self.push_frame(false);
        self.define_generics(&d.generics);
        for b in &d.bases {
            self.resolve_type(b);
        }
        self.resolve_where(&d.where_);
        self.resolve_members(&d.members);
        self.pop_frame();
        self.self_ty.pop();
    }

    fn resolve_members(&mut self, members: &'p [Member]) {
        for m in members {
            match m {
                Member::Field(f) => {
                    self.resolve_type(&f.ty);
                    if let Some(d) = &f.default {
                        self.resolve_expr(d);
                    }
                }
                Member::Item(i) => self.resolve_member_item(i),
            }
        }
    }

    fn resolve_member_item(&mut self, i: &'p Item) {
        match &i.kind {
            ItemKind::Fn(f) => self.resolve_fn(f, None),
            ItemKind::Decl(d) => self.resolve_decl_uses(d),
            _ => self.resolve_item(i),
        }
    }

    fn resolve_enum(&mut self, e: &'p EnumDecl, def: Option<DefId>) {
        self.self_ty.push(def);
        self.push_frame(false);
        self.define_generics(&e.generics);
        for b in &e.bases {
            self.resolve_type(b);
        }
        self.resolve_where(&e.where_);
        for v in &e.variants {
            for f in &v.fields {
                self.resolve_type(&f.ty);
            }
            if let Some(d) = &v.discriminant {
                self.resolve_expr(d);
            }
        }
        for it in &e.items {
            self.resolve_member_item(it);
        }
        self.pop_frame();
        self.self_ty.pop();
    }

    fn resolve_impl(&mut self, im: &'p ImplDecl) {
        self.push_frame(false);
        self.define_generics(&im.generics);
        let target = self.resolve_type(&im.target);
        let for_ty = im.for_type.as_ref().map(|t| self.resolve_type(t));
        let self_def = match for_ty {
            Some(d) => d,
            None => target,
        };
        self.self_ty.push(self_def);
        self.resolve_where(&im.where_);
        self.resolve_members(&im.members);
        self.self_ty.pop();
        self.pop_frame();
    }

    // ---- Types --------------------------------------------------------------------------

    /// Resolves a type. Returns the definition when it is a plain named type.
    pub(super) fn resolve_type(&mut self, t: &'p Type) -> Option<DefId> {
        match &t.kind {
            TypeKind::Path { path, args } => {
                let def = self.resolve_type_path(path);
                for a in args {
                    match a {
                        GenericArg::Type(t) => {
                            self.resolve_type(t);
                        }
                        GenericArg::Const(e) => self.resolve_expr(e),
                    }
                }
                def
            }
            TypeKind::SelfType => match self.self_ty.last().copied().flatten() {
                Some(d) => Some(d),
                None => {
                    if self.self_ty.is_empty() {
                        self.error(
                            t.span,
                            "`Self` is only available inside a type or an `impl`",
                        );
                    }
                    None
                }
            },
            TypeKind::List(e) | TypeKind::Option(e) | TypeKind::Any(e) => {
                self.resolve_type(e);
                None
            }
            TypeKind::Array(e, n) => {
                self.resolve_type(e);
                self.resolve_expr(n);
                None
            }
            TypeKind::Tuple(ts) => {
                for t in ts {
                    self.resolve_type(t);
                }
                None
            }
            TypeKind::FixedTuple(ts, n) => {
                for t in ts {
                    self.resolve_type(t);
                }
                self.resolve_expr(n);
                None
            }
            TypeKind::Result(a, b) => {
                self.resolve_type(a);
                self.resolve_type(b);
                None
            }
            TypeKind::Fn { params, ret, .. } => {
                for p in params {
                    self.resolve_type(&p.ty);
                }
                if let Some(r) = ret {
                    self.resolve_type(r);
                }
                None
            }
            TypeKind::Error => None,
        }
    }

    fn resolve_type_path(&mut self, path: &'p Path) -> Option<DefId> {
        let first = path.segments.first()?;
        let mut cur = self.resolve_name(first, "type")?;
        for seg in &path.segments[1..] {
            cur = self.resolve_member_path(cur, seg)?;
        }
        let def = &self.defs[cur.0 as usize];
        if !def.kind.is_type() && def.kind != DefKind::Module {
            let msg = format!("`{}` is {}, not a type", def.name, def.kind.describe());
            self.error(path.span, msg);
            return None;
        }
        if def.kind == DefKind::Module {
            let msg = format!("`{}` is a module, not a type", def.name);
            self.error(path.span, msg);
            return None;
        }
        Some(cur)
    }

    /// `module.name` or `type.member`: one step along a dotted path.
    fn resolve_member_path(&mut self, base: DefId, seg: &Ident) -> Option<DefId> {
        let want = self.text(self.cur_file, seg);
        let base_def = &self.defs[base.0 as usize];
        if let Some(scope) = base_def.scope {
            // A module: its exports, as seen from here.
            let exports = self.exports_of(scope, self.cur_scope);
            return match exports.iter().find(|(n, _)| n == want) {
                Some((_, d)) => {
                    let d = *d;
                    self.record_use(seg, d);
                    Some(d)
                }
                None => {
                    let in_module = self.scopes[scope].names.contains_key(want);
                    let path = self.scope_path(scope);
                    let mut msg = if in_module {
                        format!("`{want}` is private to `{path}`")
                    } else {
                        format!("`{path}` has no `{want}`")
                    };
                    if !in_module {
                        if let Some(s) = self.suggest(want, exports.iter().map(|(n, _)| n.as_str()))
                        {
                            msg.push_str(&format!("; did you mean `{s}`?"));
                        }
                    }
                    self.error(seg.span, msg);
                    None
                }
            };
        }
        if base_def.kind.is_type() {
            if let Some(&m) = self.members.get(&(base, want.to_string())) {
                self.record_use(seg, m);
                return Some(m);
            }
        }
        None
    }

    // ---- Blocks and statements ----------------------------------------------------------

    pub(super) fn resolve_block(&mut self, b: &'p Block) {
        self.push_frame(false);
        self.resolve_block_stmts(b);
        self.pop_frame();
    }

    /// The statements of a block in the current frame (for `switch` arms, which already have one).
    fn resolve_block_stmts(&mut self, b: &'p Block) {
        // Local declarations (fn, struct, ...) can be used before they appear.
        for s in &b.stmts {
            if let StmtKind::Item(i) = &s.kind {
                if matches!(
                    i.kind,
                    ItemKind::Decl(_) | ItemKind::Import(_) | ItemKind::Mod { .. }
                ) {
                    continue;
                }
                for (name, def, span) in self.define_item(self.cur_scope, i) {
                    let dup = self
                        .frames
                        .last()
                        .is_some_and(|f| f.names.contains_key(&name));
                    if dup {
                        self.error(
                            span,
                            format!("`{name}` is already defined in this scope; pick another name"),
                        );
                    } else if let Some(frame) = self.frames.last_mut() {
                        frame.names.insert(name, def);
                    }
                }
            }
        }
        for s in &b.stmts {
            self.resolve_stmt(s);
        }
        if let Some(t) = &b.tail {
            self.resolve_stmt_expr(t);
        }
    }

    fn resolve_stmt(&mut self, s: &'p Stmt) {
        match &s.kind {
            StmtKind::Decl(d) => self.resolve_local_decl(d),
            StmtKind::Item(i) => {
                if let ItemKind::Decl(d) = &i.kind {
                    self.resolve_local_decl(d);
                } else {
                    self.resolve_item(i);
                }
            }
            StmtKind::Expr(e) => self.resolve_stmt_expr(e),
        }
    }

    /// An expression in statement position: a `guard` leaves its bindings in the block.
    fn resolve_stmt_expr(&mut self, e: &'p Expr) {
        if let ExprKind::Guard { cond, else_ } = &e.kind {
            let bound = self.resolve_cond(cond);
            self.resolve_block(else_);
            for (name, def) in bound {
                if let Some(frame) = self.frames.last_mut() {
                    frame.names.insert(name, def);
                }
            }
        } else {
            self.resolve_expr(e);
        }
    }

    fn resolve_local_decl(&mut self, d: &'p Decl) {
        if let Some(t) = &d.ty {
            self.resolve_type(t);
        }
        // The value is resolved before the names exist: `const x = x + 1` uses the outer `x`.
        if let Some(e) = &d.init {
            self.resolve_expr(e);
        }
        let kind = DefKind::Local;
        self.bind_pattern(&d.pattern, kind, false);
    }

    // ---- Patterns -----------------------------------------------------------------------

    /// Defines the names a pattern binds in the innermost frame and resolves what it mentions.
    /// `alternative`: a later alternative of a `case`, which reuses the names of the first.
    pub(super) fn bind_pattern(&mut self, p: &'p Pattern, kind: DefKind, alternative: bool) {
        match &p.kind {
            PatternKind::Wildcard | PatternKind::None | PatternKind::Error => {}
            PatternKind::Binding { name, .. } => {
                self.define_local(name, kind, alternative);
            }
            PatternKind::As { name, pattern } => {
                self.define_local(name, kind, alternative);
                self.bind_pattern(pattern, kind, alternative);
            }
            PatternKind::Literal(e) => self.resolve_expr(e),
            PatternKind::Range { start, end, .. } => {
                if let Some(e) = start {
                    self.resolve_expr(e);
                }
                if let Some(e) = end {
                    self.resolve_expr(e);
                }
            }
            PatternKind::Tuple(ps) => {
                for p in ps {
                    self.bind_pattern(p, kind, alternative);
                }
            }
            PatternKind::List(items) => {
                for i in items {
                    match i {
                        ListPatItem::Pattern(p) => self.bind_pattern(p, kind, alternative),
                        ListPatItem::Rest { name: Some(n), .. } => {
                            self.define_local(n, kind, alternative);
                        }
                        ListPatItem::Rest { .. } => {}
                    }
                }
            }
            PatternKind::Variant {
                leading_dot,
                path,
                args,
            } => {
                self.resolve_variant_path(*leading_dot, path);
                for a in args.iter().flatten() {
                    self.bind_pattern(&a.pattern, kind, alternative);
                }
            }
            PatternKind::TypeTest(path) => {
                self.resolve_type_path(path);
            }
            PatternKind::Struct { path, fields, .. } => {
                self.resolve_type_path(path);
                for f in fields {
                    match &f.pattern {
                        Some(p) => self.bind_pattern(p, kind, alternative),
                        None => {
                            self.define_local(&f.name, kind, alternative);
                        }
                    }
                }
            }
        }
    }

    fn resolve_variant_path(&mut self, leading_dot: bool, path: &'p Path) {
        if leading_dot {
            return; // the type comes from context: the type checker's job
        }
        let Some(first) = path.segments.first() else {
            return;
        };
        if path.segments.len() == 1 {
            let name = self.text(self.cur_file, first);
            match self.lookup(name) {
                Ok(Some(d)) => {
                    let kind = self.defs[d.0 as usize].kind;
                    if matches!(
                        kind,
                        DefKind::BuiltinFn | DefKind::BuiltinCase | DefKind::Variant
                    ) {
                        self.record_use(first, d);
                    } else {
                        self.error(
                            first.span,
                            format!("`{name}` is not an enum case; write cases as `.{name}` or `type.{name}`"),
                        );
                    }
                }
                _ => {
                    self.error(
                        first.span,
                        format!("unknown case `{name}`; write cases as `.{name}` or `type.{name}`"),
                    );
                }
            }
            return;
        }
        let Some(mut cur) = self.resolve_name(first, "type") else {
            return;
        };
        for seg in &path.segments[1..] {
            match self.resolve_member_path(cur, seg) {
                Some(d) => cur = d,
                None => {
                    // A missing case of a known enum is an error; unknown types were reported already.
                    let base = &self.defs[cur.0 as usize];
                    if base.kind == DefKind::Enum {
                        let want = self.text(self.cur_file, seg);
                        let msg = format!("`{}` has no case `{want}`", base.name);
                        self.error(seg.span, msg);
                    }
                    return;
                }
            }
        }
    }

    // ---- Conditions: names bound by `is` flow into what the condition guards -----------------

    /// Resolves a condition and returns the names its `is` patterns bind, for the code that runs
    /// when the condition holds. `a && b`: `b` already sees what `a` bound.
    fn resolve_cond(&mut self, e: &'p Expr) -> Vec<(String, DefId)> {
        match &e.kind {
            ExprKind::Paren(inner) => self.resolve_cond(inner),
            ExprKind::Is { value, pattern } => {
                self.resolve_expr(value);
                self.push_frame(false);
                self.bind_pattern(pattern, DefKind::Local, false);
                let frame = self.frames.pop().map(|f| f.names).unwrap_or_default();
                frame.into_iter().collect()
            }
            ExprKind::Binary {
                op: Punct::AndAnd,
                lhs,
                rhs,
            } => {
                let mut left = self.resolve_cond(lhs);
                self.push_frame(false);
                if let Some(f) = self.frames.last_mut() {
                    for (n, d) in &left {
                        f.names.insert(n.clone(), *d);
                    }
                }
                let right = self.resolve_cond(rhs);
                self.pop_frame();
                left.extend(right);
                left
            }
            _ => {
                self.resolve_expr(e);
                Vec::new()
            }
        }
    }

    /// Resolves `body` in a new frame that holds `bound`.
    fn with_bindings(&mut self, bound: Vec<(String, DefId)>, f: impl FnOnce(&mut Self)) {
        self.push_frame(false);
        if let Some(fr) = self.frames.last_mut() {
            for (n, d) in bound {
                fr.names.insert(n, d);
            }
        }
        f(self);
        self.pop_frame();
    }

    // ---- Expressions --------------------------------------------------------------------

    pub(super) fn resolve_expr(&mut self, e: &'p Expr) {
        let res = self.resolve_expr_res(e);
        if let Some(d) = res {
            let kind = self.defs[d.0 as usize].kind;
            if kind == DefKind::Module {
                let name = self.defs[d.0 as usize].name.clone();
                self.error(
                    e.span,
                    format!("`{name}` is a module, not a value; use `{name}.something`"),
                );
            } else if kind == DefKind::BuiltinCase {
                self.error(
                    e.span,
                    "`some` is only a pattern; write the value itself, or `none`",
                );
            }
        }
    }

    /// Resolves an expression; for a name or a module path it returns what it names.
    fn resolve_expr_res(&mut self, e: &'p Expr) -> Option<DefId> {
        match &e.kind {
            ExprKind::Int
            | ExprKind::Float
            | ExprKind::Char
            | ExprKind::Bool(_)
            | ExprKind::None
            | ExprKind::Underscore
            | ExprKind::DotName(_)
            | ExprKind::Continue { .. }
            | ExprKind::Error => None,
            ExprKind::Str(parts) => {
                for p in parts {
                    if let StrPart::Interp(inner) = p {
                        self.resolve_expr(inner);
                    }
                }
                None
            }
            ExprKind::Name(id) => self.resolve_name(id, "value"),
            ExprKind::SelfValue => {
                match self.self_param.last().copied().flatten() {
                    Some(d) => {
                        self.uses.insert((self.cur_file, e.span.start), d);
                    }
                    None => self.error(
                        e.span,
                        "`self` is only available inside a method that takes `self`",
                    ),
                }
                None
            }
            ExprKind::SelfType => {
                match self.self_ty.last().copied().flatten() {
                    Some(d) => {
                        self.uses.insert((self.cur_file, e.span.start), d);
                        return Some(d);
                    }
                    None => self.error(
                        e.span,
                        "`Self` is only available inside a type or an `impl`",
                    ),
                }
                None
            }
            ExprKind::Paren(inner) => self.resolve_expr_res(inner),
            ExprKind::Tuple(es) => {
                for x in es {
                    self.resolve_expr(x);
                }
                None
            }
            ExprKind::List(items) | ExprKind::Set(items) | ExprKind::Soa(items) => {
                for i in items {
                    self.resolve_expr(&i.value);
                }
                None
            }
            ExprKind::Repeat { value, count } => {
                self.resolve_expr(value);
                self.resolve_expr(count);
                None
            }
            ExprKind::Map(items) => {
                for i in items {
                    match i {
                        MapItem::Pair(k, v) => {
                            self.resolve_expr(k);
                            self.resolve_expr(v);
                        }
                        MapItem::Spread(x) => self.resolve_expr(x),
                    }
                }
                None
            }
            ExprKind::Builder(b) => {
                self.push_frame(false);
                for c in &b.clauses {
                    match c {
                        Clause::Of { pats, source } => {
                            self.resolve_expr(source);
                            for p in pats {
                                self.bind_pattern(p, DefKind::Local, false);
                            }
                        }
                        Clause::If(cond) => {
                            let bound = self.resolve_cond(cond);
                            if let Some(fr) = self.frames.last_mut() {
                                for (n, d) in bound {
                                    fr.names.insert(n, d);
                                }
                            }
                        }
                    }
                }
                if let Some(r) = &b.result {
                    self.resolve_expr(r);
                }
                if let Some(v) = &b.value {
                    self.resolve_expr(v);
                }
                self.pop_frame();
                None
            }
            ExprKind::StructLit { ty, fields } => {
                self.resolve_type(ty);
                for f in fields {
                    match f {
                        StructInit::Field(f) => match &f.value {
                            Some(v) => self.resolve_expr(v),
                            // `point { x, y }`: the field name is also a variable.
                            None => {
                                self.resolve_name(&f.name, "value");
                            }
                        },
                        StructInit::Spread(x) => self.resolve_expr(x),
                    }
                }
                None
            }
            ExprKind::Field { base, name, .. } => {
                let b = self.resolve_expr_res(base)?;
                let def = &self.defs[b.0 as usize];
                if def.scope.is_some() || def.kind.is_type() {
                    self.resolve_member_path(b, name)
                } else {
                    None
                }
            }
            ExprKind::TupleIndex { base, .. } => {
                self.resolve_expr(base);
                None
            }
            ExprKind::Index { base, index } => {
                self.resolve_expr(base);
                self.resolve_expr(index);
                None
            }
            ExprKind::Call { callee, args, .. } => {
                self.resolve_expr(callee);
                for a in args {
                    self.resolve_expr(&a.value);
                }
                None
            }
            ExprKind::TypeApply { base, args } => {
                let r = self.resolve_expr_res(base);
                for a in args {
                    match a {
                        GenericArg::Type(t) => {
                            self.resolve_type(t);
                        }
                        GenericArg::Const(x) => self.resolve_expr(x),
                    }
                }
                r
            }
            ExprKind::Unary { operand, .. }
            | ExprKind::IncDec { operand, .. }
            | ExprKind::Copy(operand)
            | ExprKind::Try(operand)
            | ExprKind::Become(operand)
            | ExprKind::Defer(operand)
            | ExprKind::Meta(operand) => {
                self.resolve_expr(operand);
                None
            }
            ExprKind::Binary {
                op: Punct::AndAnd, ..
            }
            | ExprKind::Is { .. } => {
                // Names bound inside only live inside this expression.
                self.push_frame(false);
                self.resolve_cond(e);
                self.pop_frame();
                None
            }
            ExprKind::Binary { lhs, rhs, .. }
            | ExprKind::In { lhs, rhs }
            | ExprKind::Assign { lhs, rhs, .. } => {
                self.resolve_expr(lhs);
                self.resolve_expr(rhs);
                None
            }
            ExprKind::Range {
                start, end, step, ..
            } => {
                for x in [start, end, step].into_iter().flatten() {
                    self.resolve_expr(x);
                }
                None
            }
            ExprKind::Ternary { cond, then, else_ } => {
                let bound = self.resolve_cond(cond);
                self.with_bindings(bound, |r| r.resolve_expr(then));
                self.resolve_expr(else_);
                None
            }
            ExprKind::Convert { value, ty, .. } => {
                self.resolve_expr(value);
                self.resolve_type(ty);
                None
            }
            ExprKind::Closure(c) => {
                self.push_frame(false);
                for cap in &c.captures {
                    self.resolve_name(&cap.name, "value");
                }
                for p in &c.params {
                    if let Some(t) = &p.ty {
                        self.resolve_type(t);
                    }
                    self.bind_pattern(&p.pattern, DefKind::Param, false);
                }
                if c.params.is_empty() {
                    let it = self.define_named(
                        "it",
                        Span::new(e.span.start, e.span.start),
                        DefKind::Param,
                    );
                    self.closure_it.insert((self.cur_file, e.span.start), it);
                }
                if let Some(r) = &c.ret {
                    self.resolve_type(r);
                }
                self.resolve_expr(&c.body);
                self.pop_frame();
                None
            }
            ExprKind::MetaIf(i) => {
                let src = &self.program.module(self.cur_file).src;
                let then = match crate::meta::eval_cond(&i.cond, src, &self.program.target) {
                    Ok(b) => b,
                    Err((span, msg)) => {
                        self.error(span, msg);
                        true
                    }
                };
                // At module level the chosen branch holds declarations that are already defined.
                let top = self.frames.is_empty();
                if then {
                    if top {
                        self.resolve_stmts_toplevel(&i.then.stmts);
                    } else {
                        self.resolve_block(&i.then);
                    }
                } else if let Some(el) = &i.else_ {
                    match (&el.kind, top) {
                        (ExprKind::Block(b), true) => self.resolve_stmts_toplevel(&b.stmts),
                        _ => self.resolve_expr(el),
                    }
                }
                None
            }
            ExprKind::If(i) => {
                let bound = self.resolve_cond(&i.cond);
                self.with_bindings(bound, |r| r.resolve_block(&i.then));
                if let Some(el) = &i.else_ {
                    self.resolve_expr(el);
                }
                None
            }
            ExprKind::Unless(i) => {
                let bound = self.resolve_cond(&i.cond);
                self.resolve_block(&i.then);
                if let Some(el) = &i.else_ {
                    self.with_bindings(bound, |r| r.resolve_expr(el));
                }
                None
            }
            ExprKind::Switch(s) => {
                self.resolve_expr(&s.scrutinee);
                for arm in &s.arms {
                    self.push_frame(false);
                    for (k, p) in arm.patterns.iter().enumerate() {
                        self.bind_pattern(p, DefKind::Local, k > 0);
                    }
                    if let Some(g) = &arm.guard {
                        let bound = self.resolve_cond(g);
                        if let Some(fr) = self.frames.last_mut() {
                            for (n, d) in bound {
                                fr.names.insert(n, d);
                            }
                        }
                    }
                    self.resolve_block_stmts(&arm.body);
                    self.pop_frame();
                }
                None
            }
            ExprKind::Select(arms) => {
                for arm in arms {
                    self.push_frame(false);
                    if let Some(op) = &arm.op {
                        self.resolve_expr(op);
                    }
                    if let Some(b) = &arm.bind {
                        self.define_local(b, DefKind::Local, false);
                    }
                    self.resolve_block(&arm.body);
                    self.pop_frame();
                }
                None
            }
            ExprKind::Scope { name, body } => {
                self.push_frame(false);
                self.define_local(name, DefKind::Local, false);
                self.resolve_block(body);
                self.pop_frame();
                None
            }
            ExprKind::Block(b) | ExprKind::Unsafe(b) | ExprKind::Loop(b) => {
                self.resolve_block(b);
                None
            }
            ExprKind::While { cond, body } => {
                let bound = self.resolve_cond(cond);
                self.with_bindings(bound, |r| r.resolve_block(body));
                None
            }
            ExprKind::DoWhile { body, cond } => {
                self.resolve_block(body);
                self.resolve_expr(cond);
                None
            }
            ExprKind::For(f) => {
                match &f.head {
                    ForHead::Of { pats, iter } => {
                        self.resolve_expr(iter);
                        self.push_frame(false);
                        for p in pats {
                            self.bind_pattern(p, DefKind::Local, false);
                        }
                    }
                    ForHead::In { pat, iter } => {
                        self.resolve_expr(iter);
                        self.push_frame(false);
                        self.bind_pattern(pat, DefKind::Local, false);
                    }
                    ForHead::C { init, cond, step } => {
                        self.push_frame(false);
                        if let Some(s) = init {
                            self.resolve_stmt(s);
                        }
                        if let Some(c) = cond {
                            self.resolve_expr(c);
                        }
                        if let Some(s) = step {
                            self.resolve_expr(s);
                        }
                    }
                }
                self.resolve_block(&f.body);
                self.pop_frame();
                None
            }
            ExprKind::Guard { cond, else_ } => {
                // Outside statement position the bindings have nowhere to go.
                self.push_frame(false);
                self.resolve_cond(cond);
                self.pop_frame();
                self.resolve_block(else_);
                None
            }
            ExprKind::Return(v) | ExprKind::Yield(v) => {
                if let Some(x) = v {
                    self.resolve_expr(x);
                }
                None
            }
            ExprKind::Break { value, .. } => {
                if let Some(x) = value {
                    self.resolve_expr(x);
                }
                None
            }
            ExprKind::Labeled { body, .. } => {
                self.resolve_expr(body);
                None
            }
        }
    }
}
