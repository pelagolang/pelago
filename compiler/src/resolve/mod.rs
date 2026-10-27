//! Name resolution: what every name in the program refers to.
//!
//! Input is a [`Program`] (parsed modules). Output is [`Resolved`]: a table of definitions and a map
//! from each use of a name to its definition, plus diagnostics. The type checker builds on it.
//!
//! Steps:
//!
//! 1. **Collect.** Every module gets a scope with its declarations (functions, types, constants,
//!    inline `mod`s). Types also get their members (fields, methods, enum cases). Order does not
//!    matter, so this finishes before anything is looked up.
//! 2. **Imports.** `import` forms are resolved repeatedly until nothing changes, so chains of
//!    imports and re-exports (`pub import`) work in any order.
//! 3. **Bodies.** Functions, closures, patterns and blocks are walked with a stack of local scopes;
//!    each name is looked up in locals, then the module's scope, then the prelude.
//!
//! Left to the type checker: anything that needs a type, such as `p.x`, `p.len()` and the leading-dot
//! cases (`.circle`).
//!
//! Rules enforced here: no name twice in a module (no overloading); no name twice in one local scope;
//! nested `fn`s cannot use the enclosing function's locals; privacy (`priv`, `pub pkg`, `pub`)
//! across modules; a module is not a value; `self` only in methods; ambiguous imports are an error
//! at the use.

mod body;
#[cfg(test)]
mod tests;

use crate::ast::*;
use crate::keyword::{PRELUDE_FUNCTIONS, PRELUDE_TYPES};
use crate::package::{Diagnostic, ModuleId, PackageId, Program};
use crate::span::Span;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefKind {
    Module,
    Fn,
    Const,
    Var,
    Struct,
    Obj,
    Enum,
    Interface,
    Variant,
    TypeAlias,
    Field,
    GenericParam,
    Param,
    Local,
    /// `int`, `option`, `string`, ...
    BuiltinType,
    /// `print`, `ok`, `err`, ...
    BuiltinFn,
    /// `some`: only a pattern.
    BuiltinCase,
}

impl DefKind {
    pub fn is_type(self) -> bool {
        matches!(
            self,
            DefKind::Struct
                | DefKind::Obj
                | DefKind::Enum
                | DefKind::Interface
                | DefKind::TypeAlias
                | DefKind::GenericParam
                | DefKind::BuiltinType
        )
    }

    fn is_local(self) -> bool {
        matches!(self, DefKind::Param | DefKind::Local)
    }

    pub fn describe(self) -> &'static str {
        match self {
            DefKind::Module => "a module",
            DefKind::Fn | DefKind::BuiltinFn => "a function",
            DefKind::Const => "a constant",
            DefKind::Var => "a variable",
            DefKind::Struct | DefKind::Obj => "a type",
            DefKind::Enum => "an enum",
            DefKind::Interface => "an interface",
            DefKind::Variant | DefKind::BuiltinCase => "an enum case",
            DefKind::TypeAlias => "a type",
            DefKind::Field => "a field",
            DefKind::GenericParam => "a type parameter",
            DefKind::Param | DefKind::Local => "a variable",
            DefKind::BuiltinType => "a type",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Def {
    pub kind: DefKind,
    pub name: String,
    /// The module (scope) that declares it; `None` for built-ins.
    pub module: Option<usize>,
    /// The file the span belongs to.
    pub file: Option<ModuleId>,
    pub span: Span,
    pub vis: Vis,
    /// The type a member belongs to.
    pub parent: Option<DefId>,
    /// For types: fields, methods and enum cases.
    pub members: Vec<DefId>,
    /// For modules: the scope index.
    pub scope: Option<usize>,
}

/// The result of resolving a program.
pub struct Resolved {
    pub defs: Vec<Def>,
    /// Each resolved use of a name, keyed by the file and the start of the name.
    pub uses: HashMap<(ModuleId, u32), DefId>,
    /// Each declaration of a name, keyed the same way.
    pub decls: HashMap<(ModuleId, u32), DefId>,
    pub diagnostics: Vec<Diagnostic>,
    /// The definition made for each function, type, module or constant item, keyed by the file
    /// and the start of the item.
    pub item_defs: HashMap<(ModuleId, u32), DefId>,
    /// The implicit `it` of each closure without parameters, keyed by the closure's start.
    pub closure_it: HashMap<(ModuleId, u32), DefId>,
    members: HashMap<(DefId, String), DefId>,
    scope_parent: Vec<Option<usize>>,
    scope_package: Vec<PackageId>,
}

impl Resolved {
    pub fn def(&self, id: DefId) -> &Def {
        &self.defs[id.0 as usize]
    }

    pub fn use_at(&self, file: ModuleId, start: u32) -> Option<DefId> {
        self.uses.get(&(file, start)).copied()
    }

    /// Is something declared with `vis` in scope `owner` visible from scope `from`?
    pub fn visible(&self, vis: Vis, owner: usize, from: usize) -> bool {
        match vis {
            Vis::Pub | Vis::Export => true,
            Vis::PubPkg => self.scope_package[owner] == self.scope_package[from],
            Vis::Priv | Vis::Default => {
                let mut cur = Some(from);
                while let Some(i) = cur {
                    if i == owner {
                        return true;
                    }
                    cur = self.scope_parent[i];
                }
                false
            }
        }
    }

    /// A field, method or enum case of a type, by name.
    pub fn member(&self, owner: DefId, name: &str) -> Option<DefId> {
        self.members.get(&(owner, name.to_string())).copied()
    }
}

// ---- Scopes of modules ------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Origin {
    Glob,
    Import,
    Local,
}

#[derive(Clone)]
struct Entry {
    def: DefId,
    origin: Origin,
    vis: Vis,
    /// Other definitions that claim the same name with the same strength.
    clash: Vec<DefId>,
}

struct Scope<'p> {
    /// The file that holds this module (an inline `mod` shares its file's id).
    file: ModuleId,
    package: PackageId,
    name: String,
    parent: Option<usize>,
    /// Submodules by name: files below this one, and inline `mod`s.
    children: HashMap<String, usize>,
    names: HashMap<String, Entry>,
    stmts: &'p [Stmt],
    def: DefId,
}

struct PendingImport<'p> {
    scope: usize,
    imp: &'p Import,
    vis: Vis,
    span: Span,
}

pub(crate) struct Frame {
    names: HashMap<String, DefId>,
    /// A function body: names of the enclosing function cannot be used past this.
    barrier: bool,
}

/// What a path of segments can name.
#[derive(Clone, Copy)]
enum Target {
    Scope(usize),
    Item(DefId),
}

pub(crate) struct Resolver<'p> {
    program: &'p Program,
    defs: Vec<Def>,
    scopes: Vec<Scope<'p>>,
    /// Scope index of each package's root pseudo-module.
    package_roots: Vec<usize>,
    prelude: HashMap<String, DefId>,
    members: HashMap<(DefId, String), DefId>,
    item_defs: HashMap<(ModuleId, u32), DefId>,
    closure_it: HashMap<(ModuleId, u32), DefId>,
    uses: HashMap<(ModuleId, u32), DefId>,
    decls: HashMap<(ModuleId, u32), DefId>,
    diags: Vec<Diagnostic>,
    pending: Vec<PendingImport<'p>>,
    // Body state.
    cur_scope: usize,
    cur_file: ModuleId,
    frames: Vec<Frame>,
    self_ty: Vec<Option<DefId>>,
    self_param: Vec<Option<DefId>>,
    /// Inside a `meta if`: duplicate names between branches are allowed for now.
    in_meta: u32,
    /// Walking a pattern whose names already have definitions (module-level `const`).
    keep_defs: bool,
}

/// Resolves every module of the program.
pub fn resolve(program: &Program) -> Resolved {
    let mut r = Resolver::new(program);
    r.collect_all();
    r.fill_prelude_from_core();
    r.resolve_imports();
    r.resolve_all_bodies();
    r.finish()
}

pub(crate) fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        prev = cur;
    }
    prev[b.len()]
}

impl<'p> Resolver<'p> {
    fn new(program: &'p Program) -> Resolver<'p> {
        let mut r = Resolver {
            program,
            defs: Vec::new(),
            scopes: Vec::new(),
            package_roots: Vec::new(),
            prelude: HashMap::new(),
            members: HashMap::new(),
            item_defs: HashMap::new(),
            closure_it: HashMap::new(),
            uses: HashMap::new(),
            decls: HashMap::new(),
            diags: Vec::new(),
            pending: Vec::new(),
            cur_scope: 0,
            cur_file: ModuleId(0),
            frames: Vec::new(),
            self_ty: Vec::new(),
            self_param: Vec::new(),
            in_meta: 0,
            keep_defs: false,
        };
        r.build_prelude();
        r
    }

    fn finish(self) -> Resolved {
        let mut diagnostics = self.program.diagnostics.clone();
        diagnostics.extend(self.diags);
        Resolved {
            defs: self.defs,
            uses: self.uses,
            decls: self.decls,
            diagnostics,
            item_defs: self.item_defs,
            closure_it: self.closure_it,
            members: self.members,
            scope_parent: self.scopes.iter().map(|s| s.parent).collect(),
            scope_package: self.scopes.iter().map(|s| s.package).collect(),
        }
    }

    fn new_def(
        &mut self,
        kind: DefKind,
        name: &str,
        scope: Option<usize>,
        span: Span,
        vis: Vis,
        parent: Option<DefId>,
    ) -> DefId {
        let id = DefId(self.defs.len() as u32);
        let file = scope.map(|s| self.scopes[s].file);
        self.defs.push(Def {
            kind,
            name: name.to_string(),
            module: scope,
            file,
            span,
            vis,
            parent,
            members: Vec::new(),
            scope: None,
        });
        if let (Some(f), false) = (file, span.is_empty()) {
            self.decls.insert((f, span.start), id);
        }
        id
    }

    fn build_prelude(&mut self) {
        let add = |r: &mut Resolver<'p>, name: &str, kind: DefKind| {
            let id = r.new_def(kind, name, None, Span::new(0, 0), Vis::Pub, None);
            r.prelude.insert(name.to_string(), id);
        };
        for t in PRELUDE_TYPES.iter().chain(["list", "iter"].iter()) {
            add(self, t, DefKind::BuiltinType);
        }
        for f in PRELUDE_FUNCTIONS.iter().chain(["ok", "err"].iter()) {
            add(self, f, DefKind::BuiltinFn);
        }
        add(self, "some", DefKind::BuiltinCase);
    }

    /// The `pub` names of every module of the `core` package become part of the prelude.
    fn fill_prelude_from_core(&mut self) {
        let Some(core) = self.program.find_package("core") else {
            return;
        };
        let root = self.package_roots[core.0 as usize];
        let Some(&index) = self.scopes[root].children.get("core") else {
            return;
        };
        // The index module `core` and everything below it.
        let mut mods = vec![index];
        let mut at = 0;
        while at < mods.len() {
            let kids: Vec<usize> = self.scopes[mods[at]].children.values().copied().collect();
            mods.extend(kids);
            at += 1;
        }
        let mut names: Vec<(String, DefId)> = Vec::new();
        for m in mods {
            names.extend(
                self.scopes[m]
                    .names
                    .iter()
                    .filter(|(_, e)| {
                        e.origin == Origin::Local && matches!(e.vis, Vis::Pub | Vis::Export)
                    })
                    .map(|(n, e)| (n.clone(), e.def)),
            );
        }
        for (n, d) in names {
            self.prelude.insert(n, d);
        }
    }

    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.diags.push(Diagnostic {
            module: self.cur_file,
            span,
            message: message.into(),
            warning: false,
        });
    }

    fn error_in(&mut self, file: ModuleId, span: Span, message: impl Into<String>) {
        self.diags.push(Diagnostic {
            module: file,
            span,
            message: message.into(),
            warning: false,
        });
    }

    fn text(&self, file: ModuleId, id: &Ident) -> &'p str {
        id.text(&self.program.module(file).src)
    }

    // ---- Phase 1: collect ---------------------------------------------------------------

    fn collect_all(&mut self) {
        let program = self.program;
        // One root pseudo-module per package, then one scope per file module (same index as the
        // module id, so the roots come after).
        for m in &program.modules {
            let def = self.new_def(
                DefKind::Module,
                &m.name,
                None,
                Span::new(0, 0),
                Vis::Pub,
                None,
            );
            self.scopes.push(Scope {
                file: m.id,
                package: m.package,
                name: m.name.clone(),
                parent: None,
                children: HashMap::new(),
                names: HashMap::new(),
                stmts: &m.file.stmts,
                def,
            });
            let idx = self.scopes.len() - 1;
            self.defs[def.0 as usize].scope = Some(idx);
            self.defs[def.0 as usize].module = Some(idx);
        }
        for p in &program.packages {
            let def = self.new_def(
                DefKind::Module,
                &p.name,
                None,
                Span::new(0, 0),
                Vis::Pub,
                None,
            );
            self.scopes.push(Scope {
                file: ModuleId(0),
                package: p.id,
                name: p.name.clone(),
                parent: None,
                children: HashMap::new(),
                names: HashMap::new(),
                stmts: &[],
                def,
            });
            let idx = self.scopes.len() - 1;
            self.defs[def.0 as usize].scope = Some(idx);
            self.package_roots.push(idx);
        }
        for m in &program.modules {
            let idx = m.id.0 as usize;
            let parent = match m.parent {
                Some(p) => p.0 as usize,
                None => self.package_roots[m.package.0 as usize],
            };
            self.scopes[idx].parent = Some(parent);
            self.scopes[parent].children.insert(m.name.clone(), idx);
        }
        // Declarations. File modules first, in order.
        let n = program.modules.len();
        for idx in 0..n {
            self.cur_scope = idx;
            self.cur_file = ModuleId(idx as u32);
            let stmts = self.scopes[idx].stmts;
            self.check_mod_line(idx, stmts);
            self.collect_stmts(idx, stmts);
        }
    }

    /// A `mod name` line at the top of a file must match the file name.
    fn check_mod_line(&mut self, idx: usize, stmts: &'p [Stmt]) {
        for s in stmts {
            if let StmtKind::Item(i) = &s.kind {
                if let ItemKind::Mod { path, body: None } = &i.kind {
                    let file = self.scopes[idx].file;
                    let written = path
                        .segments
                        .iter()
                        .map(|s| self.text(file, s))
                        .collect::<Vec<_>>()
                        .join(".");
                    let name = self.scopes[idx].name.clone();
                    if written != name {
                        self.error_in(
                            file,
                            path.span,
                            format!("this file is the module `{name}`, but the `mod` line says `{written}`"),
                        );
                    }
                }
            }
        }
    }

    fn collect_stmts(&mut self, scope: usize, stmts: &'p [Stmt]) {
        for s in stmts {
            match &s.kind {
                StmtKind::Item(i) => self.collect_item(scope, i),
                StmtKind::Decl(d) => self.collect_decl(scope, d, Vis::Default, s.span),
                StmtKind::Expr(e) => self.collect_expr(scope, e),
            }
        }
    }

    /// `meta if` blocks at module level hold declarations.
    fn collect_expr(&mut self, scope: usize, e: &'p Expr) {
        if let ExprKind::MetaIf(i) = &e.kind {
            self.in_meta += 1;
            if self.meta_choice(scope, i) {
                self.collect_stmts(scope, &i.then.stmts);
            } else if let Some(el) = &i.else_ {
                self.collect_expr(scope, el);
            }
            self.in_meta -= 1;
        } else if let ExprKind::Block(b) = &e.kind {
            if self.in_meta > 0 {
                self.collect_stmts(scope, &b.stmts);
            }
        }
    }

    /// The branch of a `meta if` that is taken (true: `then`). A condition that cannot be decided
    /// is reported.
    fn meta_choice(&mut self, scope: usize, i: &'p IfExpr) -> bool {
        let file = self.scopes[scope].file;
        let src = &self.program.module(file).src;
        match crate::meta::eval_cond(&i.cond, src, &self.program.target) {
            Ok(b) => b,
            Err((span, msg)) => {
                self.error_in(file, span, msg);
                true
            }
        }
    }

    fn collect_decl(&mut self, scope: usize, d: &'p Decl, vis: Vis, _span: Span) {
        let kind = if d.kind == DeclKind::Const {
            DefKind::Const
        } else {
            DefKind::Var
        };
        let file = self.scopes[scope].file;
        let mut names = Vec::new();
        collect_pattern_names(&d.pattern, &mut names);
        for (id, _mode) in names {
            let name = self.text(file, &id).to_string();
            let def = self.new_def(kind, &name, Some(scope), id.span, vis, None);
            self.add_local_name(scope, &name, def, id.span);
        }
    }

    fn collect_item(&mut self, scope: usize, item: &'p Item) {
        if let ItemKind::Decl(d) = &item.kind {
            self.collect_decl(scope, d, item.mods.vis, item.span);
            return;
        }
        if let ItemKind::Import(imp) = &item.kind {
            self.pending.push(PendingImport {
                scope,
                imp,
                vis: item.mods.vis,
                span: item.span,
            });
            return;
        }
        if let ItemKind::Mod {
            path,
            body: Some(body),
        } = &item.kind
        {
            self.collect_inline_mod(scope, item, path, body);
            return;
        }
        for (name, def, span) in self.define_item(scope, item) {
            self.add_local_name(scope, &name, def, span);
        }
    }

    fn collect_inline_mod(
        &mut self,
        scope: usize,
        item: &'p Item,
        path: &'p Path,
        body: &'p [Stmt],
    ) {
        let file = self.scopes[scope].file;
        let Some(last) = path.segments.last() else {
            return;
        };
        let name = self.text(file, last).to_string();
        let def = self.new_def(
            DefKind::Module,
            &name,
            Some(scope),
            last.span,
            item.mods.vis,
            None,
        );
        let package = self.scopes[scope].package;
        self.scopes.push(Scope {
            file,
            package,
            name: name.clone(),
            parent: Some(scope),
            children: HashMap::new(),
            names: HashMap::new(),
            stmts: body,
            def,
        });
        let idx = self.scopes.len() - 1;
        self.defs[def.0 as usize].scope = Some(idx);
        self.scopes[scope].children.insert(name.clone(), idx);
        self.item_defs.insert((file, item.span.start), def);
        self.add_local_name(scope, &name, def, last.span);
        let saved = (self.cur_scope, self.cur_file);
        self.cur_scope = idx;
        self.collect_stmts(idx, body);
        self.cur_scope = saved.0;
        self.cur_file = saved.1;
    }

    /// Creates the definitions for one declaration (and its members). Returns the names it
    /// introduces in the surrounding scope.
    fn define_item(&mut self, scope: usize, item: &'p Item) -> Vec<(String, DefId, Span)> {
        let file = self.scopes[scope].file;
        let vis = item.mods.vis;
        let mut out = Vec::new();
        let mut simple = |r: &mut Resolver<'p>, kind: DefKind, id: &Ident| -> DefId {
            let name = r.text(file, id).to_string();
            let def = r.new_def(kind, &name, Some(scope), id.span, vis, None);
            r.item_defs.insert((file, item.span.start), def);
            out.push((name, def, id.span));
            def
        };
        match &item.kind {
            ItemKind::Fn(f) => {
                simple(self, DefKind::Fn, &f.name);
            }
            ItemKind::TypeAlias { name, .. } => {
                simple(self, DefKind::TypeAlias, name);
            }
            ItemKind::Struct(d) | ItemKind::Obj(d) | ItemKind::Interface(d) => {
                let kind = match &item.kind {
                    ItemKind::Struct(_) => DefKind::Struct,
                    ItemKind::Obj(_) => DefKind::Obj,
                    _ => DefKind::Interface,
                };
                let def = simple(self, kind, &d.name);
                self.define_members(scope, def, &d.members);
            }
            ItemKind::Impl(im) => {
                // Methods of an `impl` belong to no type definition; the checker finds them
                // through the impl.
                for m in &im.members {
                    if let Member::Item(i) = m {
                        if let ItemKind::Fn(f) = &i.kind {
                            let name = self.text(file, &f.name).to_string();
                            let d = self.new_def(
                                DefKind::Fn,
                                &name,
                                Some(scope),
                                f.name.span,
                                i.mods.vis,
                                None,
                            );
                            self.item_defs.insert((file, i.span.start), d);
                        }
                    }
                }
            }
            ItemKind::Enum(e) => {
                let def = simple(self, DefKind::Enum, &e.name);
                for v in &e.variants {
                    let name = self.text(file, &v.name).to_string();
                    let vd = self.new_def(
                        DefKind::Variant,
                        &name,
                        Some(scope),
                        v.name.span,
                        Vis::Pub,
                        Some(def),
                    );
                    self.add_member(def, &name, vd, v.name.span);
                }
                for it in &e.items {
                    self.define_member_item(scope, def, it);
                }
            }
            _ => {}
        }
        out
    }

    fn define_members(&mut self, scope: usize, owner: DefId, members: &'p [Member]) {
        let file = self.scopes[scope].file;
        for m in members {
            match m {
                Member::Field(f) => {
                    let name = self.text(file, &f.name).to_string();
                    let d = self.new_def(
                        DefKind::Field,
                        &name,
                        Some(scope),
                        f.name.span,
                        f.vis,
                        Some(owner),
                    );
                    self.add_member(owner, &name, d, f.name.span);
                }
                Member::Item(i) => self.define_member_item(scope, owner, i),
            }
        }
    }

    fn define_member_item(&mut self, scope: usize, owner: DefId, item: &'p Item) {
        let file = self.scopes[scope].file;
        match &item.kind {
            ItemKind::Fn(f) => {
                let name = self.text(file, &f.name).to_string();
                let d = self.new_def(
                    DefKind::Fn,
                    &name,
                    Some(scope),
                    f.name.span,
                    item.mods.vis,
                    Some(owner),
                );
                self.item_defs.insert((file, item.span.start), d);
                self.add_member(owner, &name, d, f.name.span);
            }
            ItemKind::Decl(decl) => {
                let kind = if decl.kind == DeclKind::Const {
                    DefKind::Const
                } else {
                    DefKind::Var
                };
                let mut names = Vec::new();
                collect_pattern_names(&decl.pattern, &mut names);
                for (id, _) in names {
                    let name = self.text(file, &id).to_string();
                    let d = self.new_def(
                        kind,
                        &name,
                        Some(scope),
                        id.span,
                        item.mods.vis,
                        Some(owner),
                    );
                    self.add_member(owner, &name, d, id.span);
                }
            }
            _ => {}
        }
    }

    fn add_member(&mut self, owner: DefId, name: &str, def: DefId, span: Span) {
        if self.members.contains_key(&(owner, name.to_string())) {
            let owner_name = self.defs[owner.0 as usize].name.clone();
            self.error(span, format!("`{name}` is defined twice in `{owner_name}`"));
            return;
        }
        self.members.insert((owner, name.to_string()), def);
        self.defs[owner.0 as usize].members.push(def);
    }

    fn add_local_name(&mut self, scope: usize, name: &str, def: DefId, span: Span) {
        let file = self.scopes[scope].file;
        if let Some(existing) = self.scopes[scope].names.get(name) {
            if existing.origin == Origin::Local {
                if self.in_meta == 0 {
                    self.error_in(
                        file,
                        span,
                        format!("`{name}` is defined twice in this module"),
                    );
                }
                return;
            }
        }
        // A local definition beats any import of the same name.
        self.scopes[scope].names.insert(
            name.to_string(),
            Entry {
                def,
                origin: Origin::Local,
                vis: self.defs[def.0 as usize].vis,
                clash: Vec::new(),
            },
        );
    }

    // ---- Phase 2: imports ----------------------------------------------------------------

    /// Adds `name` to a module's scope. Returns true when the scope changed.
    fn add_name(&mut self, scope: usize, name: &str, def: DefId, origin: Origin, vis: Vis) -> bool {
        match self.scopes[scope].names.get_mut(name) {
            None => {
                self.scopes[scope].names.insert(
                    name.to_string(),
                    Entry {
                        def,
                        origin,
                        vis,
                        clash: Vec::new(),
                    },
                );
                true
            }
            Some(e) if e.def == def => false,
            Some(e) if e.origin > origin => false,
            Some(e) if e.origin < origin => {
                *e = Entry {
                    def,
                    origin,
                    vis,
                    clash: Vec::new(),
                };
                true
            }
            Some(e) => {
                if e.clash.contains(&def) {
                    false
                } else {
                    e.clash.push(def);
                    true
                }
            }
        }
    }

    fn resolve_imports(&mut self) {
        // Repeat until nothing new appears; then once more to report what is still missing.
        for _ in 0..64 {
            if !self.run_imports(false) {
                break;
            }
        }
        self.run_imports(true);
    }

    fn run_imports(&mut self, report: bool) -> bool {
        let mut changed = false;
        for k in 0..self.pending.len() {
            let (scope, imp, vis, span) = {
                let p = &self.pending[k];
                (p.scope, p.imp, p.vis, p.span)
            };
            changed |= self.run_import(scope, imp, vis, span, report);
        }
        changed
    }

    fn run_import(
        &mut self,
        scope: usize,
        imp: &'p Import,
        vis: Vis,
        span: Span,
        report: bool,
    ) -> bool {
        let file = self.scopes[scope].file;
        let target = match self.walk_import_path(scope, imp, report) {
            Ok(t) => t,
            Err(()) => return false,
        };
        let mut changed = false;
        let last = imp.path.segments.last().copied();
        match &imp.kind {
            ImportKind::Module | ImportKind::Alias(_) => {
                let def = match target {
                    Target::Scope(s) => self.scopes[s].def,
                    Target::Item(d) => d,
                };
                let name = match (&imp.kind, last) {
                    (ImportKind::Alias(a), _) => self.text(file, a).to_string(),
                    (_, Some(l)) => self.text(file, &l).to_string(),
                    _ => return false,
                };
                changed |= self.add_name(scope, &name, def, Origin::Import, vis);
            }
            ImportKind::Names { names, glob } => {
                let Target::Scope(m) = target else {
                    if report {
                        self.error_in(
                            file,
                            imp.path.span,
                            "this path names a single item; `{ ... } =` needs a module",
                        );
                    }
                    return false;
                };
                let exports = self.exports_of(m, scope);
                for n in names {
                    let want = self.text(file, &n.name);
                    match exports.iter().find(|(name, _)| name == want) {
                        Some((_, def)) => {
                            // The name in the import list is a use (hover, rename and references).
                            self.uses.insert((file, n.name.span.start), *def);
                            let local = n.alias.as_ref().map_or(want, |a| self.text(file, a));
                            changed |= self.add_name(scope, local, *def, Origin::Import, vis);
                        }
                        None if report => {
                            let mut msg =
                                format!("`{want}` is not exported by `{}`", self.scope_path(m));
                            if let Some(s) =
                                self.suggest(want, exports.iter().map(|(n, _)| n.as_str()))
                            {
                                msg.push_str(&format!("; did you mean `{s}`?"));
                            }
                            self.error_in(file, n.name.span, msg);
                        }
                        None => {}
                    }
                }
                if *glob {
                    for (name, def) in &exports {
                        changed |= self.add_name(scope, name, *def, Origin::Glob, vis);
                    }
                }
            }
        }
        let _ = span;
        changed
    }

    fn scope_path(&self, s: usize) -> String {
        let mut parts = Vec::new();
        let mut cur = Some(s);
        while let Some(i) = cur {
            parts.push(self.scopes[i].name.clone());
            cur = self.scopes[i].parent;
        }
        parts.reverse();
        parts.join(".")
    }

    /// Is a member declared with `vis` in scope `owner` visible from scope `from`?
    pub(crate) fn visible(&self, vis: Vis, owner: usize, from: usize) -> bool {
        match vis {
            Vis::Pub | Vis::Export => true,
            Vis::PubPkg => self.scopes[owner].package == self.scopes[from].package,
            Vis::Priv | Vis::Default => {
                let mut cur = Some(from);
                while let Some(i) = cur {
                    if i == owner {
                        return true;
                    }
                    cur = self.scopes[i].parent;
                }
                false
            }
        }
    }

    /// What module `m` makes available to module `from`: visible names and submodules.
    fn exports_of(&self, m: usize, from: usize) -> Vec<(String, DefId)> {
        let mut out: Vec<(String, DefId)> = Vec::new();
        for (name, e) in &self.scopes[m].names {
            if e.clash.is_empty() && self.visible(e.vis, m, from) {
                out.push((name.clone(), e.def));
            }
        }
        for (name, &child) in &self.scopes[m].children {
            if !out.iter().any(|(n, _)| n == name) {
                out.push((name.clone(), self.scopes[child].def));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn walk_import_path(
        &mut self,
        scope: usize,
        imp: &'p Import,
        report: bool,
    ) -> Result<Target, ()> {
        let file = self.scopes[scope].file;
        let segs = &imp.path.segments;
        if segs.is_empty() {
            return Err(());
        }
        let fail = |r: &mut Resolver<'p>, span: Span, msg: String| -> Result<Target, ()> {
            if report {
                r.error_in(file, span, msg);
            }
            Err(())
        };
        let mut cur: usize;
        let first;
        if imp.levels == 0 {
            let own = self.package_roots[self.scopes[scope].package.0 as usize];
            let want = self.text(file, &segs[0]);
            if let Some(&c) = self.scopes[scope].children.get(want) {
                // A module declared inside this one (`mod inner { ... }`).
                cur = c;
                first = 1;
            } else if let Some(&c) = self.scopes[own].children.get(want) {
                cur = c;
                first = 1;
            } else if let Some(p) = self.program.find_package(want) {
                cur = self.package_roots[p.0 as usize];
                first = 1;
            } else {
                let names: Vec<&str> = self.scopes[own]
                    .children
                    .keys()
                    .map(|s| s.as_str())
                    .collect();
                let mut msg = format!("there is no module or package `{want}`");
                if let Some(s) = self.suggest(want, names.into_iter()) {
                    msg.push_str(&format!("; did you mean `{s}`?"));
                }
                return fail(self, segs[0].span, msg);
            }
        } else {
            // The directory of the importing file is its parent module.
            let mut base = scope;
            while self.scopes[base].parent.is_some() && self.is_inline(base) {
                base = self.scopes[base].parent.unwrap_or(base);
            }
            let Some(mut dir) = self.scopes[base].parent else {
                return fail(
                    self,
                    imp.path.span,
                    "this module has no directory to be relative to".to_string(),
                );
            };
            for _ in 1..imp.levels {
                match self.scopes[dir].parent {
                    Some(p) => dir = p,
                    None => {
                        return fail(
                            self,
                            imp.path.span,
                            "too many leading dots: no such directory".to_string(),
                        )
                    }
                }
            }
            let want = self.text(file, &segs[0]);
            match self.scopes[dir].children.get(want) {
                Some(&c) => cur = c,
                None => {
                    let names: Vec<&str> = self.scopes[dir]
                        .children
                        .keys()
                        .map(|s| s.as_str())
                        .collect();
                    let mut msg = format!("there is no module `{want}` there");
                    if let Some(s) = self.suggest(want, names.into_iter()) {
                        msg.push_str(&format!("; did you mean `{s}`?"));
                    }
                    return fail(self, segs[0].span, msg);
                }
            }
            first = 1;
        }
        for (k, seg) in segs.iter().enumerate().skip(first) {
            let want = self.text(file, seg);
            if let Some(&c) = self.scopes[cur].children.get(want) {
                cur = c;
                continue;
            }
            // The last segment may name an item.
            let exports = self.exports_of(cur, scope);
            if let Some((_, def)) = exports.iter().find(|(n, _)| n == want) {
                if k + 1 == segs.len() && !matches!(imp.kind, ImportKind::Names { .. }) {
                    return Ok(Target::Item(*def));
                }
                if let Some(s) = self.defs[def.0 as usize].scope {
                    cur = s;
                    continue;
                }
            }
            let mut msg = format!("`{}` has no module or item `{want}`", self.scope_path(cur));
            if let Some(s) = self.suggest(want, exports.iter().map(|(n, _)| n.as_str())) {
                msg.push_str(&format!("; did you mean `{s}`?"));
            }
            return fail(self, seg.span, msg);
        }
        Ok(Target::Scope(cur))
    }

    fn is_inline(&self, scope: usize) -> bool {
        scope >= self.program.modules.len() + self.program.packages.len()
    }

    /// The closest of `candidates` to `want`, when it is close enough to be a typo.
    fn suggest<'a>(&self, want: &str, candidates: impl Iterator<Item = &'a str>) -> Option<String> {
        if want.chars().count() < 3 {
            return None;
        }
        let limit = if want.chars().count() <= 4 { 1 } else { 2 };
        candidates
            .map(|c| (edit_distance(want, c), c))
            .filter(|(d, c)| *d > 0 && *d <= limit && *c != want)
            .min_by_key(|(d, _)| *d)
            .map(|(_, c)| c.to_string())
    }
}

/// Names bound by a declaration pattern, with their modes.
pub(crate) fn collect_pattern_names(p: &Pattern, out: &mut Vec<(Ident, Option<Mode>)>) {
    match &p.kind {
        PatternKind::Binding { mode, name } => out.push((*name, *mode)),
        PatternKind::As { name, pattern } => {
            out.push((*name, None));
            collect_pattern_names(pattern, out);
        }
        PatternKind::Tuple(ps) => ps.iter().for_each(|p| collect_pattern_names(p, out)),
        PatternKind::List(items) => {
            for i in items {
                match i {
                    ListPatItem::Pattern(p) => collect_pattern_names(p, out),
                    ListPatItem::Rest {
                        mode,
                        name: Some(n),
                        ..
                    } => out.push((*n, *mode)),
                    ListPatItem::Rest { .. } => {}
                }
            }
        }
        PatternKind::Variant { args, .. } => {
            for a in args.iter().flatten() {
                collect_pattern_names(&a.pattern, out);
            }
        }
        PatternKind::Struct { fields, .. } => {
            for f in fields {
                match &f.pattern {
                    Some(p) => collect_pattern_names(p, out),
                    None => out.push((f.name, f.mode)),
                }
            }
        }
        _ => {}
    }
}
