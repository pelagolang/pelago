//! Tests for name resolution.

use super::*;
use crate::package::Program;

fn run(files: &[(&str, &str)]) -> (Program, Resolved) {
    let mut program = Program::new();
    program.add_package("app", files);
    let resolved = resolve(&program);
    (program, resolved)
}

fn one(src: &str) -> (Program, Resolved) {
    run(&[("main", src)])
}

fn messages(r: &Resolved) -> Vec<String> {
    r.diagnostics.iter().map(|d| d.message.clone()).collect()
}

fn assert_clean(files: &[(&str, &str)]) {
    let (_, r) = run(files);
    assert!(
        r.diagnostics.is_empty(),
        "unexpected errors: {:?}",
        messages(&r)
    );
}

fn assert_flagged(files: &[(&str, &str)], needle: &str) {
    let (_, r) = run(files);
    let m = messages(&r);
    assert!(
        m.iter().any(|x| x.contains(needle)),
        "expected {needle:?}, got {m:?}"
    );
}

fn clean(src: &str) {
    assert_clean(&[("main", src)]);
}

fn flagged(src: &str, needle: &str) {
    assert_flagged(&[("main", src)], needle);
}

/// The kind and name of what the `nth` whole-word occurrence of `word` in module `path` refers to.
fn what(
    program: &Program,
    r: &Resolved,
    path: &str,
    word: &str,
    nth: usize,
) -> Option<(DefKind, String)> {
    let m = program.modules.iter().find(|m| m.path == path)?;
    let mut count = 0;
    let bytes = m.src.as_bytes();
    let mut from = 0;
    while let Some(pos) = m.src[from..].find(word) {
        let start = from + pos;
        let end = start + word.len();
        let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        let before = start == 0 || !ident(bytes[start - 1]);
        let after = end >= bytes.len() || !ident(bytes[end]);
        if before && after {
            if count == nth {
                let d = r
                    .use_at(m.id, start as u32)
                    .or_else(|| r.decls.get(&(m.id, start as u32)).copied())?;
                let def = r.def(d);
                return Some((def.kind, def.name.clone()));
            }
            count += 1;
        }
        from = end;
    }
    None
}

fn kind_of(src: &str, word: &str, nth: usize) -> Option<DefKind> {
    let (p, r) = one(src);
    what(&p, &r, "main", word, nth).map(|(k, _)| k)
}

#[test]
fn locals_params_and_items() {
    let src = "fn f(a: int) -> int { const b = a + 1\n b }\nconst g = 3\nfn h() = f(g)";
    assert_eq!(kind_of(src, "a", 1), Some(DefKind::Param));
    assert_eq!(kind_of(src, "b", 1), Some(DefKind::Local));
    assert_eq!(kind_of(src, "f", 1), Some(DefKind::Fn));
    assert_eq!(kind_of(src, "g", 1), Some(DefKind::Const));
    clean(src);
}

#[test]
fn order_of_module_items_does_not_matter() {
    clean("fn a() = b()\nfn b() = c()\nfn c() = 1");
    clean("const x = y + 1\nconst y = 2");
}

#[test]
fn unknown_names_get_help() {
    flagged(
        "fn f() = undefined_thing",
        "cannot find value `undefined_thing`",
    );
    flagged(
        "fn counter() = 1\nfn f() = countr()",
        "did you mean `counter`",
    );
    flagged("fn f() = null", "no null");
    flagged("const x: strng = 1", "did you mean `string`");
}

#[test]
fn a_local_is_not_visible_before_its_declaration() {
    flagged(
        "fn f() { const a = b\n const b = 1 }",
        "cannot find value `b`",
    );
    // The value is resolved before the new name exists.
    clean("const x = 1\nfn f() { const x = x + 1\n x }");
    flagged("fn f() { const x = x + 1 }", "cannot find value `x`");
}

#[test]
fn shadowing_in_an_inner_scope_is_fine_but_not_in_the_same_scope() {
    clean("fn f() { const a = 1\n { const a = 2\n a }\n a }");
    flagged(
        "fn f() { const a = 1\n const a = 2 }",
        "already defined in this scope",
    );
    flagged("fn f(a: int, a: int) = a", "already defined");
    clean("fn f(a: int) { const b = a\n for a of [1] { a } }");
}

#[test]
fn duplicate_module_items() {
    flagged("fn f() = 1\nfn f() = 2", "defined twice in this module");
    flagged("struct s {}\nfn s() = 1", "defined twice in this module");
    flagged("struct s { x: int\n x: int }", "defined twice in `s`");
    // Between the branches of one `meta if` they are fine.
    clean("meta if true { fn p() = 1 } else { fn p() = 2 }");
}

#[test]
fn nested_functions_cannot_use_outer_locals() {
    flagged("fn f() { const a = 1\n fn g() = a }", "enclosing function");
    flagged("fn f(a: int) { fn g() = a }", "enclosing function");
    // A closure can.
    clean("fn f() { const a = 1\n const g = ~> a }");
    // Local functions and types are hoisted and are fine.
    clean("fn f() { g()\n fn g() = 1 }");
    clean("fn f() { const p = point { x: 1 }\n struct point { x: int } }");
}

#[test]
fn closures_and_it() {
    let src = "fn f() { const g = ~> it + 1 }";
    assert_eq!(kind_of(src, "it", 0), Some(DefKind::Param));
    clean("fn f() { const g = x ~> x + 1 }");
    flagged("fn f() { const g = x ~> it + 1 }", "cannot find value `it`");
    clean("fn f() { var t = 0\n const g = [inout t] x ~> t += x }");
    flagged(
        "fn f() { const g = [inout t] x ~> x }",
        "cannot find value `t`",
    );
}

#[test]
fn self_and_self_type() {
    clean("struct s { x: int\n fn get(self) -> int = self.x\n fn make() -> Self = s { x: 1 } }");
    flagged("fn f() = self", "only available inside a method");
    flagged(
        "struct s { fn make() = self }",
        "only available inside a method",
    );
    flagged("fn f() -> Self { }", "`Self` is only available");
    clean("impl show for s { fn show(self) -> Self = self }\nstruct s {}\ninterface show {}");
}

#[test]
fn types_and_generics() {
    clean("fn id<T>(x: T) -> T = x");
    clean("struct stack<T> { items: [T]\n fn push(inout self, x: T) {} }");
    flagged("fn f(x: T) {}", "cannot find type `T`");
    flagged("const x = 1\nfn f(a: x) {}", "not a type");
    clean("fn f(a: map<string, [int]>, b: int?, c: int ! error) {}");
    clean("enum shape { circle(r: float) }\nfn f(s: shape) {}");
    clean("fn f<T: show>(x: T) {}\ninterface show {}");
}

#[test]
fn patterns_bind_names() {
    let src = "enum shape { circle(r: float)\n empty }\nfn f(s: shape) = switch s { case .circle(r): r\n case .empty: 0.0 }";
    assert_eq!(kind_of(src, "r", 2), Some(DefKind::Local));
    clean(src);
    clean("fn f(p: (int, int)) = switch p { case (a, b): a + b }");
    clean("fn f(xs: [int]) = switch xs { case [h, ...t]: h + t.len()\n case []: 0 }");
    clean("struct point { x: int\n y: int }\nfn f(p: point) = switch p { case point { x, y: 0 }: x\n case point { y, .. }: y }");
}

#[test]
fn alternatives_share_names() {
    clean("enum e { a(x: int)\n b(x: int) }\nfn f(v: e) = switch v { case .a(n), .b(n): n }");
}

#[test]
fn bindings_from_is_flow_into_the_guarded_code() {
    clean("enum e { a(x: int) }\nfn f(v: e) { if v is .a(n) { n } }");
    clean("enum e { a(x: int) }\nfn f(v: e) { if v is .a(n) && n > 1 { n } }");
    flagged(
        "enum e { a(x: int) }\nfn f(v: e) { if v is .a(n) { }\n n }",
        "cannot find value `n`",
    );
    flagged(
        "enum e { a(x: int) }\nfn f(v: e) { if v is .a(n) || n > 1 { } }",
        "cannot find value `n`",
    );
    clean("enum e { a(x: int) }\nfn f(v: e) -> int { guard v is .a(n) else { return 0 }\n n }");
    clean("enum e { a(x: int) }\nfn f(v: e) -> int = v is .a(n) ? n : 0");
    flagged(
        "enum e { a(x: int) }\nfn f(v: e) { while v is .a(n) { }\n n }",
        "cannot find value `n`",
    );
}

#[test]
fn loops_and_builders_scope_their_names() {
    clean("fn f(xs: [int]) { for x of xs { x }\n for i, x of xs { i + x }\n for k in 0..3 { k } }");
    flagged(
        "fn f(xs: [int]) { for x of xs { }\n x }",
        "cannot find value `x`",
    );
    clean("fn f(xs: [int]) = [x of xs if x > 0 ~> x * 2]");
    clean("fn f(xs: [int], ys: [int]) = [x of xs, y of ys if x != y ~> (x, y)]");
    flagged("fn f(xs: [int]) = [x of xs ~> y]", "cannot find value `y`");
    clean("fn f() { for (var i = 0; i < 3; i += 1) { i } }");
}

#[test]
fn struct_literal_shorthand_uses_variables() {
    clean("struct p { x: int }\nfn f(x: int) = p { x }");
    flagged(
        "struct p { x: int }\nfn f() = p { x }",
        "cannot find value `x`",
    );
}

#[test]
fn spawn_scope_and_select_names() {
    clean("fn f() { scope s { s } }");
    clean("fn f(rx: int) { select { case v = rx { v }\n default { } } }");
}

#[test]
fn modules_are_not_values() {
    let files = [
        ("util", "pub fn helper() = 1"),
        ("main", "import util\nfn f() = util.helper()"),
    ];
    assert_clean(&files);
    assert_flagged(
        &[
            ("util", "pub fn helper() = 1"),
            ("main", "import util\nfn f() = util"),
        ],
        "is a module, not a value",
    );
    assert_flagged(
        &[
            ("util", "pub fn helper() = 1"),
            ("main", "import util\nconst x: util = 1"),
        ],
        "is a module, not a type",
    );
}

#[test]
fn import_forms() {
    let util = (
        "util",
        "pub fn helper() = 1\npub fn other() = 2\nstruct hidden {}",
    );
    assert_clean(&[util, ("main", "import util\nfn f() = util.helper()")]);
    assert_clean(&[util, ("main", "import u = util\nfn f() = u.helper()")]);
    assert_clean(&[
        util,
        ("main", "import { helper } = util\nfn f() = helper()"),
    ]);
    assert_clean(&[
        util,
        (
            "main",
            "import { helper: h, other } = util\nfn f() = h() + other()",
        ),
    ]);
    assert_clean(&[
        util,
        ("main", "import { ... } = util\nfn f() = helper() + other()"),
    ]);
    assert_clean(&[util, ("main", "use util\nfn f() = util.helper()")]);
    assert_clean(&[util, ("main", "import util.helper\nfn f() = helper()")]);
    assert_flagged(
        &[util, ("main", "import { nope } = util")],
        "is not exported",
    );
    assert_flagged(
        &[util, ("main", "import { helpr } = util")],
        "did you mean `helper`",
    );
    assert_flagged(&[util, ("main", "import missing")], "no module or package");
    assert_flagged(&[util, ("main", "import utill")], "did you mean `util`");
}

#[test]
fn privacy_across_modules() {
    let util = (
        "util",
        "pub fn open() = 1\nfn closed() = 2\npub pkg fn package_only() = 3",
    );
    assert_flagged(
        &[util, ("main", "import util\nfn f() = util.closed()")],
        "private",
    );
    assert_flagged(
        &[util, ("main", "import { closed } = util")],
        "not exported",
    );
    assert_clean(&[util, ("main", "import util\nfn f() = util.package_only()")]);
    // `{ ... }` brings in only what is visible.
    assert_flagged(
        &[util, ("main", "import { ... } = util\nfn f() = closed()")],
        "cannot find value `closed`",
    );
}

#[test]
fn pub_pkg_stops_at_the_package_edge() {
    let mut program = Program::new();
    program.add_package(
        "lib",
        &[("api", "pub pkg fn inner() = 1\npub fn outer() = 2")],
    );
    program.add_package(
        "app",
        &[("main", "import lib.api\nfn f() = api.inner() + api.outer()")],
    );
    let r = resolve(&program);
    let m = messages(&r);
    assert!(
        m.iter()
            .any(|x| x.contains("private") && x.contains("inner")),
        "{m:?}"
    );
    assert!(!m.iter().any(|x| x.contains("outer")), "{m:?}");
}

#[test]
fn re_exports_and_chains() {
    let files = [
        ("a", "pub fn thing() = 1"),
        ("b", "pub import { thing } = a"),
        ("main", "import { thing } = b\nfn f() = thing()"),
    ];
    assert_clean(&files);
    // Not re-exported without `pub`.
    let files = [
        ("a", "pub fn thing() = 1"),
        ("b", "import { thing } = a"),
        ("main", "import { thing } = b"),
    ];
    assert_flagged(&files, "not exported");
    // Order of the files does not matter.
    let files = [
        ("main", "import { thing } = b\nfn f() = thing()"),
        ("b", "pub import { thing } = a"),
        ("a", "pub fn thing() = 1"),
    ];
    assert_clean(&files);
}

#[test]
fn modules_may_import_each_other() {
    assert_clean(&[
        ("a", "import b\npub fn fa() = b.fb()"),
        ("b", "import a\npub fn fb() = 1\npub fn back() = a.fa()"),
    ]);
}

#[test]
fn ambiguous_imports_are_an_error_at_the_use() {
    let files = [
        ("a", "pub fn run() = 1"),
        ("b", "pub fn run() = 2"),
        (
            "main",
            "import { run } = a\nimport { run } = b\nfn f() = run()",
        ),
    ];
    assert_flagged(&files, "could be");
    // Writing the module name is fine.
    let files = [
        ("a", "pub fn run() = 1"),
        ("b", "pub fn run() = 2"),
        ("main", "import a\nimport b\nfn f() = a.run() + b.run()"),
    ];
    assert_clean(&files);
}

#[test]
fn a_local_definition_beats_an_import() {
    assert_clean(&[
        ("a", "pub fn run() = 1"),
        ("main", "import { run } = a\nfn run() = 2\nfn f() = run()"),
    ]);
    // Explicit imports beat `...`.
    assert_clean(&[
        ("a", "pub fn run() = 1"),
        ("b", "pub fn run() = 2"),
        (
            "main",
            "import { ... } = a\nimport { run } = b\nfn f() = run()",
        ),
    ]);
}

#[test]
fn submodules_directories_and_relative_paths() {
    let files = [
        ("geometry", "pub import { circle } = geometry.shapes"),
        (
            "geometry/shapes",
            "pub fn circle() = 1\npub fn square() = 2",
        ),
        (
            "main",
            "import geometry\nimport geometry.shapes\nfn f() = geometry.circle() + shapes.square()",
        ),
    ];
    assert_clean(&files);
    // `.sibling` is a module in the same directory.
    assert_clean(&[
        ("util", "pub fn helper() = 1"),
        ("main", "import { helper } = .util\nfn f() = helper()"),
    ]);
    assert_clean(&[
        ("shared", "pub fn s() = 1"),
        ("deep/inner", "import { s } = ..shared\nfn f() = s()"),
    ]);
    assert_flagged(&[("main", "import .nothing")], "no module `nothing`");
    assert_flagged(&[("main", "import ...x")], "too many leading dots");
}

#[test]
fn the_mod_line_must_match_the_file() {
    assert_clean(&[("main", "mod main\nfn f() = 1")]);
    assert_flagged(
        &[("main", "mod other\nfn f() = 1")],
        "file is the module `main`",
    );
}

#[test]
fn inline_modules() {
    clean("mod inner { pub fn f() = 1 }\nfn g() = inner.f()");
    flagged("mod inner { fn f() = 1 }\nfn g() = inner.f()", "private");
    clean("mod inner { pub fn f() = 1 }\nimport { f } = inner\nfn g() = f()");
}

#[test]
fn enum_cases_by_path() {
    clean("enum shape { circle(r: float)\n empty }\nfn f(s: shape) = switch s { case shape.circle(r): r\n case shape.empty: 0.0 }");
    flagged(
        "enum shape { circle(r: float) }\nfn f(s: shape) = switch s { case shape.square(r): r }",
        "no case `square`",
    );
    flagged(
        "enum shape { circle(r: float) }\nfn f(s: shape) = switch s { case circle(r): r }",
        "unknown case `circle`",
    );
    clean("fn f(o: int?) = switch o { case some(v): v\n case none: 0 }");
    clean("fn f(r: int ! error) = switch r { case ok(v): v\n case err(e): 0 }");
    flagged("fn f() = some", "only a pattern");
    // Enum cases as values by path.
    clean("enum shape { circle(r: float) }\nfn f() = shape.circle(r: 1.0)");
}

#[test]
fn associated_items_by_path() {
    clean(
        "struct point { x: int\n fn origin() -> point = point { x: 0 } }\nfn f() = point.origin()",
    );
}

#[test]
fn type_members_are_recorded() {
    let (_, r) = one("struct point { x: int\n fn len(self) -> int = 1 }\nenum e { a\n b }");
    let point = r
        .defs
        .iter()
        .position(|d| d.name == "point")
        .map(|i| DefId(i as u32))
        .unwrap();
    assert_eq!(r.def(r.member(point, "x").unwrap()).kind, DefKind::Field);
    assert_eq!(r.def(r.member(point, "len").unwrap()).kind, DefKind::Fn);
    let e = r
        .defs
        .iter()
        .position(|d| d.name == "e")
        .map(|i| DefId(i as u32))
        .unwrap();
    assert_eq!(r.def(r.member(e, "a").unwrap()).kind, DefKind::Variant);
    assert!(r.member(e, "z").is_none());
}

#[test]
fn syntax_errors_are_reported_too() {
    flagged("fn f( {", "expected");
    flagged("fn f() { break }", "inside a loop");
}

#[test]
fn the_tour_resolves_apart_from_the_standard_library() {
    // `tour.lang` imports a library that does not exist yet and calls many library functions.
    // Everything else must resolve.
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../examples/tour.lang"
    ))
    .unwrap();
    let mut program = Program::new();
    program.add_package("app", &[("tour", &src)]);
    let r = resolve(&program);
    // It is a syntax tour, not a program: it uses names that are never defined. Make sure resolving
    // finishes and finds the names that are defined.
    assert!(!r.defs.is_empty());
}

#[test]
fn resolving_never_panics_on_damaged_input() {
    let tour = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../examples/tour.lang"
    ))
    .unwrap();
    let chars: Vec<char> = tour.chars().collect();
    let mut state = 0x1234_5678_9ABC_DEF1u64;
    let mut next = |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n as u64) as usize
    };
    for round in 0..300 {
        let mut c = chars.clone();
        for _ in 0..1 + next(8) {
            let i = next(c.len());
            match next(3) {
                0 => {
                    c.remove(i);
                }
                1 => c.insert(i, ['{', '}', '(', ')', ':', '.', 'x', '\n'][next(8)]),
                _ => {
                    let j = next(c.len());
                    c.swap(i, j);
                }
            }
        }
        let src: String = c.into_iter().collect();
        let mut program = Program::new();
        let name = format!("m{round}");
        program.add_package(
            "app",
            &[(name.as_str(), src.as_str()), ("other", "pub fn x() = 1")],
        );
        let _ = resolve(&program);
    }
}
