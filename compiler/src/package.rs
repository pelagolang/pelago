//! Packages and modules: the source files of a program, parsed.
//!
//! A file is a module and its file name is the module name (`docs/design/modules.md`). A directory
//! is a parent module: `geometry.pel` is `geometry`, `geometry/shapes.pel` is `geometry.shapes`,
//! and a front file `geometry.pel` next to the directory `geometry/` is the parent of what is
//! inside it. A parent without a front file is an empty module made up for the occasion.

use crate::ast::{File, ItemKind, StmtKind};
use crate::parser::{parse_and_check_with, ParseOptions};
use crate::span::Span;
use std::collections::HashMap;
use std::path::Path;

/// The modules of the `core` package: `(module path, source)`. `core.pel` is the front file (the index) of
/// the modules in `compiler/core/core/`, which are its children (`core.lists`, ...).
pub const CORE_MODULES: &[(&str, &str)] = &[
    ("core", include_str!("../core/core.pel")),
    ("core.rt", include_str!("../core/core/rt.pel")),
    ("core.lists", include_str!("../core/core/lists.pel")),
    ("core.text", include_str!("../core/core/text.pel")),
    ("core.maps", include_str!("../core/core/maps.pel")),
    ("core.strings", include_str!("../core/core/strings.pel")),
    ("core.iters", include_str!("../core/core/iters.pel")),
    ("core.results", include_str!("../core/core/results.pel")),
    ("core.numbers", include_str!("../core/core/numbers.pel")),
    ("core.tasks", include_str!("../core/core/tasks.pel")),
    ("core.boxes", include_str!("../core/core/boxes.pel")),
    ("core.arena", include_str!("../core/core/arena.pel")),
    ("core.columns", include_str!("../core/core/columns.pel")),
    ("core.objects", include_str!("../core/core/objects.pel")),
];

/// The modules of the `std` package: `(module name, source)`. The files live in `compiler/std/`.
/// Unlike `core`, the package is added only when a program imports `std.<module>`.
pub const STD_MODULES: &[(&str, &str)] = &[("big", include_str!("../std/big.pel"))];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PackageId(pub u32);

/// A problem found while loading, parsing or checking, tied to a module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub module: ModuleId,
    pub span: Span,
    pub message: String,
    /// A warning does not stop compilation.
    pub warning: bool,
}

pub struct Module {
    pub id: ModuleId,
    pub package: PackageId,
    /// The last segment: `shapes` for `geometry.shapes`.
    pub name: String,
    /// The full dotted path inside the package: `geometry.shapes`.
    pub path: String,
    pub parent: Option<ModuleId>,
    pub children: Vec<ModuleId>,
    pub src: String,
    pub file: File,
    /// Made up because only a directory exists, not a front file.
    pub synthetic: bool,
}

pub struct Package {
    pub id: PackageId,
    pub name: String,
    /// The top-level modules.
    pub roots: Vec<ModuleId>,
}

/// Everything being compiled: the packages and all their modules.
#[derive(Default)]
pub struct Program {
    pub packages: Vec<Package>,
    pub modules: Vec<Module>,
    /// Syntax errors and syntax-check errors from all modules.
    pub diagnostics: Vec<Diagnostic>,
    /// What `target` means in a `meta if`.
    pub target: crate::meta::Target,
}

impl Program {
    pub fn new() -> Program {
        Program::default()
    }

    /// Adds the always-available library as the package `core`; the `pub` names of all its modules
    /// are in scope in every module.
    pub fn add_core(&mut self) -> PackageId {
        self.add_package("core", CORE_MODULES)
    }

    /// A program that already contains the core library.
    pub fn with_core() -> Program {
        let mut p = Program::new();
        p.add_core();
        p
    }

    pub fn module(&self, id: ModuleId) -> &Module {
        &self.modules[id.0 as usize]
    }

    pub fn package(&self, id: PackageId) -> &Package {
        &self.packages[id.0 as usize]
    }

    pub fn find_package(&self, name: &str) -> Option<PackageId> {
        self.packages.iter().find(|p| p.name == name).map(|p| p.id)
    }

    /// Adds a package made of `(module path, source)` pairs. A module path is dotted
    /// (`geometry.shapes`); slashes are accepted too.
    pub fn add_package(&mut self, name: &str, sources: &[(&str, &str)]) -> PackageId {
        let pid = PackageId(self.packages.len() as u32);
        self.packages.push(Package {
            id: pid,
            name: name.to_string(),
            roots: Vec::new(),
        });
        let mut by_path: HashMap<String, ModuleId> = HashMap::new();
        // Real files first, so a front file is never replaced by a made-up parent.
        let mut ordered: Vec<(String, &str)> = sources
            .iter()
            .map(|(p, s)| (p.replace('/', "."), *s))
            .collect();
        ordered.sort_by_key(|(p, _)| p.matches('.').count());
        for (path, src) in &ordered {
            self.ensure_module(pid, path, Some(src), &mut by_path);
        }
        if name != "std" && self.find_package("std").is_none() && self.imports_std(pid) {
            self.add_package("std", STD_MODULES);
        }
        pid
    }

    /// Whether a top-level import of the package starts at `std`.
    fn imports_std(&self, pid: PackageId) -> bool {
        self.modules
            .iter()
            .filter(|m| m.package == pid)
            .flat_map(|m| m.file.stmts.iter().map(move |s| (m, s)))
            .any(|(m, s)| match &s.kind {
                StmtKind::Item(item) => match &item.kind {
                    ItemKind::Import(i) => {
                        i.levels == 0
                            && i.path
                                .segments
                                .first()
                                .is_some_and(|seg| seg.text(&m.src) == "std")
                    }
                    _ => false,
                },
                _ => false,
            })
    }

    fn ensure_module(
        &mut self,
        pid: PackageId,
        path: &str,
        src: Option<&str>,
        by_path: &mut HashMap<String, ModuleId>,
    ) -> ModuleId {
        if let Some(&id) = by_path.get(path) {
            // A made-up parent may later turn out to have a real file.
            if let Some(src) = src {
                let m = &mut self.modules[id.0 as usize];
                if m.synthetic {
                    m.synthetic = false;
                    m.src = src.to_string();
                    let id = m.id;
                    self.parse_into(id);
                }
            }
            return id;
        }
        let (parent_path, name) = match path.rsplit_once('.') {
            Some((p, n)) => (Some(p), n),
            None => (None, path),
        };
        let parent = parent_path.map(|p| self.ensure_module(pid, p, None, by_path));
        let id = ModuleId(self.modules.len() as u32);
        let empty = crate::ast::File {
            stmts: Vec::new(),
            span: Span::new(0, 0),
        };
        self.modules.push(Module {
            id,
            package: pid,
            name: name.to_string(),
            path: path.to_string(),
            parent,
            children: Vec::new(),
            src: src.unwrap_or("").to_string(),
            file: empty,
            synthetic: src.is_none(),
        });
        by_path.insert(path.to_string(), id);
        match parent {
            Some(p) => self.modules[p.0 as usize].children.push(id),
            None => self.packages[pid.0 as usize].roots.push(id),
        }
        if src.is_some() {
            self.parse_into(id);
        }
        id
    }

    fn parse_into(&mut self, id: ModuleId) {
        let src = std::mem::take(&mut self.modules[id.0 as usize].src);
        let (out, lex) = parse_and_check_with(&src, ParseOptions::compile());
        for e in lex {
            self.diagnostics.push(Diagnostic {
                module: id,
                span: e.span,
                message: e.kind.message().to_string(),
                warning: false,
            });
        }
        for e in out.errors {
            self.diagnostics.push(Diagnostic {
                module: id,
                span: e.span,
                message: e.message,
                warning: false,
            });
        }
        let m = &mut self.modules[id.0 as usize];
        m.file = out.file;
        m.src = src;
    }

    /// Reads a package from disk. `root` is a directory (every `.pel` file below it is a module)
    /// or a single `.pel` file (a package of one module).
    pub fn add_package_from_path(&mut self, root: &Path) -> Result<PackageId, String> {
        let mut files: Vec<(String, String)> = Vec::new();
        let name;
        if root.is_dir() {
            name = root
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("main")
                .to_string();
            collect_files(root, root, &mut files)?;
        } else {
            let stem = root
                .file_stem()
                .and_then(|n| n.to_str())
                .ok_or_else(|| format!("`{}` is not a source file", root.display()))?;
            name = stem.to_string();
            let src = std::fs::read_to_string(root)
                .map_err(|e| format!("cannot read `{}`: {e}", root.display()))?;
            files.push((stem.to_string(), src));
        }
        if files.is_empty() {
            return Err(format!("no `.pel` files in `{}`", root.display()));
        }
        files.sort();
        let refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(p, s)| (p.as_str(), s.as_str()))
            .collect();
        Ok(self.add_package(&name, &refs))
    }
}

pub(crate) fn collect_files(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, String)>,
) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot read `{}`: {e}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("pel") {
            let rel = path.strip_prefix(root).unwrap_or(&path).with_extension("");
            let module_path = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(".");
            let src = std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read `{}`: {e}", path.display()))?;
            out.push((module_path, src));
        }
    }
    Ok(())
}
