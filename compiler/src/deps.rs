//! Packages that Islet installed: reading `deps/map.yaml` and adding the packages it names to a [`Program`].
//!
//! `islet install` writes `deps/map.yaml` next to the project (see `islet/docs/resolved-map.md`): one import scope
//! per package and per workspace member, each mapping an import key (`json`, `json/v2`) to a package folder under
//! `deps/` or to another member (`member:libs/core`). The compiler never runs Islet and never reads `islet.lock`;
//! this file is all it needs.
//!
//! Each package has its own import table (`Package::imports`), filled from the map, so two packages may use
//! different versions of one dependency in the same build. A package for each distinct target is loaded once and
//! named by the target (`json@1.5.0`).
//!
//! A second major is written as a module path inside the import: `import json.v2.decode` is the dependency
//! `json`, then its second major. The package for the key `json/v2` goes into `Package::majors` under
//! `("json", "v2")`, so it needs no `json` of its own. The plain key `json` is major 0 or 1.

use crate::package::{collect_files, PackageId, Program};
use crate::yaml::{self, Diag, Val};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

/// What one package or member imports: key to target (`json@1.5.0` or `member:libs/core`).
type Imports = BTreeMap<String, String>;

#[derive(Debug, Default, PartialEq)]
pub struct DepMap {
    /// Package id to (folder below `deps/`, its imports).
    pub packages: BTreeMap<String, (String, Imports)>,
    /// Member path to its imports.
    pub members: BTreeMap<String, Imports>,
}

fn str_of(n: &yaml::Node, what: &str) -> Result<String, yaml::PErr> {
    match &n.v {
        Val::Str(s) => Ok(s.clone()),
        _ => Err(yaml::PErr::new(n.at, format!("{what} must be text"))),
    }
}

fn imports_of(n: &yaml::Node) -> Result<Imports, yaml::PErr> {
    let Val::Table(t) = &n.v else {
        return Err(yaml::PErr::new(n.at, "`imports` must be a mapping"));
    };
    let mut out = Imports::new();
    for e in &t.entries {
        out.insert(e.key.clone(), str_of(&e.node, "an import target")?);
    }
    Ok(out)
}

impl DepMap {
    pub fn parse(text: &str) -> Result<DepMap, String> {
        let read = || -> Result<DepMap, yaml::PErr> {
            let root = yaml::parse(text)?;
            match root.get("format") {
                Some(n) if n.v == Val::Int(1) => {}
                Some(n) => {
                    return Err(yaml::PErr::new(
                        n.at,
                        "unsupported `format` (this pelago reads `format: 1`)",
                    ))
                }
                None => return Err(yaml::PErr::new(0, "the map needs `format: 1`")),
            }
            let mut map = DepMap::default();
            if let Some(Val::Table(t)) = root.get("packages").map(|n| &n.v) {
                for e in &t.entries {
                    let Val::Table(p) = &e.node.v else {
                        return Err(yaml::PErr::new(e.node.at, "a package is a mapping"));
                    };
                    let dir = match p.get("dir") {
                        Some(n) => str_of(n, "`dir`")?,
                        None => return Err(yaml::PErr::new(e.key_at, "`dir` is missing")),
                    };
                    let imports = match p.get("imports") {
                        Some(n) => imports_of(n)?,
                        None => Imports::new(),
                    };
                    map.packages.insert(e.key.clone(), (dir, imports));
                }
            }
            if let Some(Val::Table(t)) = root.get("members").map(|n| &n.v) {
                for e in &t.entries {
                    let Val::Table(m) = &e.node.v else {
                        return Err(yaml::PErr::new(e.node.at, "a member is a mapping"));
                    };
                    let imports = match m.get("imports") {
                        Some(n) => imports_of(n)?,
                        None => Imports::new(),
                    };
                    map.members.insert(e.key.clone(), imports);
                }
            }
            Ok(map)
        };
        read().map_err(|e| {
            let d = Diag::new("", text, e);
            format!("{}:{}: {}", d.line, d.col, d.msg)
        })
    }
}

/// The `deps/map.yaml` closest above `start` (a directory), if any.
pub fn find_map(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        let f = d.join("deps").join("map.yaml");
        if f.is_file() {
            return Some(f);
        }
        dir = d.parent();
    }
    None
}

impl Program {
    /// Adds the packages the project at `project` (already added as `pid`) imports, if a `deps/map.yaml` is found
    /// above it, and fills each package's import table. Without a map nothing changes.
    pub fn add_dependencies(&mut self, project: &Path, pid: PackageId) -> Result<(), String> {
        let abs =
            std::fs::canonicalize(project).map_err(|e| format!("{}: {e}", project.display()))?;
        let start = if abs.is_dir() {
            abs
        } else {
            abs.parent().map(Path::to_path_buf).unwrap_or(abs)
        };
        let Some(map_path) = find_map(&start) else {
            return Ok(());
        };
        let text = std::fs::read_to_string(&map_path)
            .map_err(|e| format!("{}: {e}", map_path.display()))?;
        let map = DepMap::parse(&text).map_err(|e| format!("{}:{e}", map_path.display()))?;
        let deps_dir = map_path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let root = deps_dir.parent().unwrap_or(Path::new(".")).to_path_buf();
        let rel = start
            .strip_prefix(&root)
            .map_err(|_| "the project is not below its deps folder".to_string())?;
        let member = if rel.as_os_str().is_empty() {
            ".".to_string()
        } else {
            rel.components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        };
        let Some(first) = map.members.get(&member) else {
            return Ok(());
        };
        let imports_of = |target: &str| -> Result<Imports, String> {
            match target.strip_prefix("member:") {
                Some(m) => Ok(map.members.get(m).cloned().unwrap_or_default()),
                None => match map.packages.get(target) {
                    Some((_, i)) => Ok(i.clone()),
                    None => Err(format!("{target} is not in deps/map.yaml")),
                },
            }
        };
        // Load every distinct target once.
        let mut loaded: BTreeMap<String, PackageId> = BTreeMap::new();
        let mut queue: VecDeque<String> = first.values().cloned().collect();
        while let Some(target) = queue.pop_front() {
            if loaded.contains_key(&target) {
                continue;
            }
            let next = imports_of(&target)?;
            let dir = match target.strip_prefix("member:") {
                Some(m) => root.join(m),
                None => deps_dir.join(&map.packages[&target].0),
            };
            let mut files = Vec::new();
            collect_files(&dir, &dir, &mut files)?;
            files.sort();
            let refs: Vec<(&str, &str)> = files
                .iter()
                .map(|(p, s)| (p.as_str(), s.as_str()))
                .collect();
            let id = self.add_package(&target, &refs);
            loaded.insert(target, id);
            queue.extend(next.into_values());
        }
        // Each package's table: key to package, and a second major under (name, major).
        let mut owners: Vec<(PackageId, Imports)> = vec![(pid, first.clone())];
        for (target, id) in &loaded {
            owners.push((*id, imports_of(target)?));
        }
        for (owner, imports) in owners {
            for (key, target) in imports {
                let dep = loaded[&target];
                let p = &mut self.packages[owner.0 as usize];
                match key.split_once('/') {
                    Some((base, major)) => {
                        p.majors.insert((base.to_string(), major.to_string()), dep);
                    }
                    None => {
                        p.imports.insert(key, dep);
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAP: &str = "format: 1\n\npackages:\n  json@1.5.0:\n    dir: json@1.5.0\n    imports:\n      units: units@0.2.4\n  units@0.2.4:\n    dir: units@0.2.4\n\nmembers:\n  .:\n    imports:\n      json: json@1.5.0\n";

    #[test]
    fn the_map_reads() {
        let m = DepMap::parse(MAP).unwrap();
        assert_eq!(m.packages["json@1.5.0"].0, "json@1.5.0");
        assert_eq!(m.packages["json@1.5.0"].1["units"], "units@0.2.4");
        assert_eq!(m.members["."]["json"], "json@1.5.0");
    }

    #[test]
    fn a_bad_map_says_where() {
        let e = DepMap::parse("format: 2\n").unwrap_err();
        assert!(e.contains("1:9") && e.contains("unsupported"), "{e}");
        assert!(DepMap::parse("packages: {}\n")
            .unwrap_err()
            .contains("format"));
    }

    fn scratch(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("pelago-deps-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn two_majors_of_one_package_load_and_check() {
        let d = scratch("majors");
        write(
            &d.join("deps/map.yaml"),
            "format: 1\n\npackages:\n  json/v2@2.0.1:\n    dir: json/v2@2.0.1\n  json@1.5.0:\n    dir: json@1.5.0\n\nmembers:\n  .:\n    imports:\n      json: json@1.5.0\n      json/v2: json/v2@2.0.1\n",
        );
        write(
            &d.join("deps/json@1.5.0/decode.pel"),
            "pub fn one() -> int {\n    1\n}\n",
        );
        write(
            &d.join("deps/json/v2@2.0.1/decode.pel"),
            "pub fn two() -> int {\n    2\n}\n",
        );
        write(
            &d.join("main.pel"),
            "import { one } = json.decode\nimport { two } = json.v2.decode\n\nfn main() {\n    print(one(), two())\n}\n",
        );
        let mut program = Program::with_core();
        program.add_package_from_path(&d.join("main.pel")).unwrap();
        assert!(program.find_package("json@1.5.0").is_some());
        assert!(program.find_package("json/v2@2.0.1").is_some());
        // `deps/` is not part of the project itself.
        assert_eq!(
            program.packages.iter().filter(|p| p.name == "main").count(),
            1
        );
        let main = program.find_package("main").unwrap();
        assert!(program
            .modules
            .iter()
            .filter(|m| m.package == main)
            .all(|m| m.path == "main"));
        let res = crate::resolve::resolve(&program);
        let typed = crate::typecheck::check(&program, &res);
        assert!(
            typed.diagnostics.iter().all(|x| x.warning),
            "{:?}",
            typed.diagnostics
        );
        assert!(program.diagnostics.is_empty());
    }

    #[test]
    fn two_packages_may_use_different_versions_of_one_dependency() {
        let d = scratch("scopes");
        write(
            &d.join("deps/map.yaml"),
            "format: 1\n\npackages:\n  a@1.0.0:\n    dir: a@1.0.0\n    imports:\n      units: units@0.3.0\n  units@0.2.0:\n    dir: units@0.2.0\n  units@0.3.0:\n    dir: units@0.3.0\n\nmembers:\n  .:\n    imports:\n      a: a@1.0.0\n      units: units@0.2.0\n",
        );
        write(
            &d.join("deps/a@1.0.0/api.pel"),
            "import { which } = units.u\n\npub fn seen() -> int {\n    which()\n}\n",
        );
        write(
            &d.join("deps/units@0.2.0/u.pel"),
            "pub fn which() -> int {\n    2\n}\n",
        );
        write(
            &d.join("deps/units@0.3.0/u.pel"),
            "pub fn which() -> int {\n    3\n}\n",
        );
        write(
            &d.join("main.pel"),
            "import { seen } = a.api\nimport { which } = units.u\n\nfn main() {\n    print(seen(), which())\n}\n",
        );
        let mut program = Program::with_core();
        program.add_package_from_path(&d.join("main.pel")).unwrap();
        let res = crate::resolve::resolve(&program);
        let typed = crate::typecheck::check(&program, &res);
        assert!(
            typed.diagnostics.iter().all(|x| x.warning),
            "{:?}",
            typed.diagnostics
        );
        assert!(program.diagnostics.is_empty());
        // `a` sees units 0.3.0, the project sees units 0.2.0, and the program prints both.
        let l = crate::lower::lower(&program, &res, &typed);
        assert!(l.diagnostics.is_empty(), "{:?}", l.diagnostics);
        let o = crate::ir::run_full(&l.module, "main", &[]);
        assert_eq!(o.output.trim(), "3 2");
        assert!(program.find_package("units@0.2.0").is_some());
        assert!(program.find_package("units@0.3.0").is_some());
    }

    #[test]
    fn no_map_changes_nothing() {
        let d = scratch("nomap");
        write(&d.join("main.pel"), "fn main() {\n}\n");
        let mut program = Program::with_core();
        program.add_package_from_path(&d.join("main.pel")).unwrap();
        assert!(program.packages.iter().all(|p| p.imports.is_empty()));
    }
}
