//! Tests for stage 5: moves, exclusive access, `const` depth.

use crate::package::Program;
use crate::resolve::resolve;
use crate::typecheck::check as typecheck;

fn errors(src: &str) -> Vec<String> {
    let mut program = Program::with_core();
    program.add_package("app", &[("main", src)]);
    let res = resolve(&program);
    let typed = typecheck(&program, &res);
    let mut out: Vec<String> = typed
        .diagnostics
        .iter()
        .map(|d| d.message.clone())
        .collect();
    out.extend(
        super::check(&program, &res, &typed)
            .into_iter()
            .map(|d| d.message),
    );
    out
}

fn clean(src: &str) {
    let e = errors(src);
    assert!(e.is_empty(), "unexpected errors in {src:?}: {e:?}");
}

fn flagged(src: &str, needle: &str) {
    let e = errors(src);
    assert!(
        e.iter().any(|m| m.contains(needle)),
        "expected {needle:?} in {src:?}, got {e:?}"
    );
}

#[test]
fn const_depth() {
    flagged("fn t() { const xs = [1]\n xs[0] = 2 }", "`xs` is a `const`");
    flagged(
        "fn t() { const xs: [int] = []\n xs.push!(1) }",
        "`xs` is a `const`",
    );
    flagged(
        "struct p { x: int }\nfn t() { const a = p { x: 1 }\n a.x = 2 }",
        "`a` is a `const`",
    );
    clean("struct p { x: int }\nfn t() { var a = p { x: 1 }\n a.x = 2 }");
    flagged(
        "fn t(xs: [int]) { xs.push!(1) }",
        "`xs` is a read-only parameter; write `inout xs`",
    );
    clean("fn t(inout xs: [int]) { xs.push!(1) }");
    flagged(
        "struct c { n: int\n fn bump(self) { self.n += 1 } }",
        "takes `self` read-only; write `inout self`",
    );
    clean("struct c { n: int\n fn bump(inout self) { self.n += 1 } }");
    flagged(
        "fn t(inout xs: [int]) { }\nfn u() { const a: [int] = []\n t(inout a) }",
        "`a` is a `const`",
    );
    flagged(
        "fn t(xs: [int]) { for x of xs { x = 1 } }",
        "only views part of another value",
    );
    clean("fn t(inout xs: [int]) { for inout x of xs { x += 1 } }");
    clean("struct c { n: int\n init(n: int) { self.n = n } }");
}

#[test]
fn closures_change_only_what_they_capture_with_inout() {
    flagged(
        "fn t() { var total = 0\n const f = (x: int) ~> total += x }",
        "capture it with `[inout total]`",
    );
    clean("fn t() { var total = 0\n const f = [inout total] (x: int) ~> total += x }");
    clean("fn t() { var total = 0\n const f = (x: int) ~> x + total }");
}

#[test]
fn moves_of_owning_values() {
    flagged(
        "fn t() { const a = \"x\"\n const b = a\n print(a) }",
        "`a` was moved on line 2",
    );
    clean("fn t() { const a = \"x\"\n const b = copy a\n print(a) }");
    // Plain values copy.
    clean("fn t() { const a = 1\n const b = a\n print(a + b) }");
    flagged(
        "fn t() { var xs = [1]\n const ys = xs\n xs.push!(2) }",
        "was moved",
    );
    // A new value revives the variable.
    clean("fn t() { var s = \"a\"\n const t = s\n s = \"b\"\n print(s) }");
    flagged(
        "fn take(sink s: string) { }\nfn t() { const a = \"x\"\n take(sink a)\n take(sink a) }",
        "was moved",
    );
    clean("fn take(sink s: string) { }\nfn t() { const a = \"x\"\n take(sink a) }");
    flagged(
        "fn t() -> [string] { const a = \"x\"\n const l = [a]\n print(a)\n l }",
        "was moved",
    );
    flagged("struct b { name: string }\nfn t() { const n = \"x\"\n const v = b { name: n }\n print(n) }", "was moved");
    flagged(
        "fn t() { const n = \"x\"\n const r: string ! error = ok(n)\n print(n) }",
        "was moved",
    );
}

#[test]
fn moves_through_branches_and_loops() {
    flagged(
        "fn t(c: bool) { const a = \"x\"\n if c { const b = a }\n print(a) }",
        "was moved",
    );
    clean("fn t(c: bool) { const a = \"x\"\n if c { const b = a } else { const d = a }\n }");
    clean("fn t(c: bool) { const a = \"x\"\n if c { const b = a\n return }\n print(a) }");
    flagged(
        "fn take(sink s: string) { }\nfn t() { const a = \"x\"\n for i of 0..3 { take(sink a) } }",
        "was moved",
    );
    clean(
        "fn take(sink s: string) { }\nfn t() { for i of 0..3 { const a = \"x\"\n take(sink a) } }",
    );
    clean("fn take(sink s: string) { }\nfn t() { var a = \"x\"\n for i of 0..3 { take(sink a)\n a = \"y\" } }");
    flagged("fn t(s: string) -> string = s", "read-only here");
    clean("fn t(s: string) -> string = copy s");
    clean("fn t(sink s: string) -> string = s");
    flagged(
        "struct b { name: string\n fn get(self) -> string = self.name }",
        "cannot be taken out of `self`",
    );
    clean("struct b { name: string\n fn get(self) -> string = copy self.name }");
}

#[test]
fn objects_and_methods_that_consume() {
    clean(
        "obj f { x: int\n fn close(sink self) { } }\nfn t() { const a = f { x: 1 }\n a.close!!() }",
    );
    flagged("obj f { x: int\n fn close(sink self) { } }\nfn t() { const a = f { x: 1 }\n a.close!!()\n a.close!!() }", "was moved");
    flagged(
        "obj f { x: int }\nfn t() { const a = f { x: 1 }\n const b = a\n print(a.x) }",
        "was moved",
    );
}

#[test]
fn exclusive_access() {
    flagged("fn swap2(inout a: int, inout b: int) { }\nfn t() { var xs = [1, 2]\n swap2(inout xs[0], inout xs[1]) }", "used by another argument");
    clean("fn swap2(inout a: int, inout b: int) { }\nfn t() { var x = 1\n var y = 2\n swap2(inout x, inout y) }");
    flagged(
        "fn both(inout a: [int], b: [int]) { }\nfn t() { var xs = [1]\n both(inout xs, xs) }",
        "used by another argument",
    );
    clean(
        "fn both(inout a: [int], b: int) { }\nfn t() { var xs = [1]\n both(inout xs, xs.len()) }",
    );
    flagged(
        "fn t() { var xs = [1]\n xs.extend!(xs) }",
        "used by another argument",
    );
    clean("fn t() { var xs = [1]\n xs.push!(xs.len()) }");
    // Different fields do not overlap.
    clean("struct p { a: [int], b: [int] }\nfn both(inout a: [int], b: [int]) { }\nfn t() { var v = p { a: [], b: [] }\n both(inout v.a, v.b) }");
}

#[test]
fn control_flow_does_not_confuse_moves() {
    clean("fn t(c: bool) -> string { const a = \"x\"\n if c { return a }\n a }");
    clean("fn t() { const a = \"x\"\n const b = loop { break a } }");
    clean("fn t(items: [string]) { var best: string? = none\n for w of items { best = copy w } }");
}

#[test]
fn checking_never_panics_on_damaged_input() {
    let tour =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../examples/tour.pel"))
            .unwrap();
    let chars: Vec<char> = tour.chars().collect();
    let mut state = 0x1357_9BDF_2468_ACE0u64;
    let mut next = |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n as u64) as usize
    };
    for _ in 0..100 {
        let mut c = chars.clone();
        for _ in 0..1 + next(6) {
            let i = next(c.len());
            match next(3) {
                0 => {
                    c.remove(i);
                }
                1 => c.insert(
                    i,
                    ['{', '}', '(', ')', ':', '.', 'x', '\n', '<', '>'][next(10)],
                ),
                _ => {
                    let j = next(c.len());
                    c.swap(i, j);
                }
            }
        }
        let src: String = c.into_iter().collect();
        let mut program = Program::with_core();
        program.add_package("app", &[("main", src.as_str())]);
        let res = resolve(&program);
        let typed = typecheck(&program, &res);
        let _ = super::check(&program, &res, &typed);
    }
}

#[test]
fn the_tour_runs_through_without_panicking() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../examples/tour.pel"))
        .unwrap();
    let e = errors(&src);
    assert!(!e.is_empty());
}

#[test]
fn defer_moves_happen_at_the_end() {
    clean("obj f { x: int\n fn close(sink self) { } }\nfn t() { const a = f { x: 1 }\n defer a.close!!()\n print(a.x) }");
}

#[test]
fn become_cannot_lend_locals() {
    let sum = "fn sum_to(n: int, acc: int) -> int { if n == 0 { return acc }\n become sum_to(n - 1, acc + n) }\n";
    clean(sum);
    flagged(
        "fn use2(xs: [int]) -> int = 1\nfn t() -> int { const xs = [1]\n become use2(xs) }",
        "would be gone when `become`",
    );
    clean("fn use2(xs: [int]) -> int = 1\nfn t(xs: [int]) -> int { become use2(xs) }");
    clean("fn use2(sink xs: [int]) -> int = 1\nfn t() -> int { const xs = [1]\n become use2(sink xs) }");
    flagged(
        "fn use2(xs: [int]) -> int = 1\nfn t() -> int { defer print(1)\n become use2([1]) }",
        "cannot follow a `defer`",
    );
}

#[test]
fn views_do_not_outlive_their_source() {
    flagged(
        "fn t() -> str { const s = \"a\" + \"b\"\n s.trim() }",
        "points into `s`",
    );
    clean("fn t(s: str) -> str = s.trim()");
    clean("fn t(s: string) -> str = s.trim()");
    clean("fn t() -> str = \"literal\"");
    flagged("struct b { name: str }", "cannot hold a view");
    flagged("enum e { a(str) }", "cannot hold a view");
    flagged(
        "fn make() -> fn(int) -> int { ~> it }",
        "cannot return a function value",
    );
    clean("fn make(sink f: box<fn(int) -> int>) -> box<fn(int) -> int> = f");
}

#[test]
fn spawn_moves_what_it_uses() {
    flagged("fn work(xs: [int]) -> int = 1\nfn t() { const data = [1]\n const h = spawn(~> work(data)) }", "must be moved in");
    clean("fn work(xs: [int]) -> int = 1\nfn t() { const data = [1]\n const h = spawn([sink data] ~> work(data))\n h.join!!() }");
    clean("fn t() { const n = 3\n const h = spawn(~> n + 1)\n h.join!!() }");
    flagged(
        "fn t() { var total = 0\n const h = spawn([inout total] ~> total += 1) }",
        "captured with `inout`",
    );
    clean("fn t() { const (tx, rx) = chan<int>()\n spawn([sink tx] ~> { tx.send(1) })\n for x of rx { print(x) } }");
}

#[test]
fn linear_values_are_consumed_exactly_once() {
    let tx = "linear struct txn { id: int\n fn commit(sink self) { _ = self }\n fn rollback(sink self) { _ = self } }\nfn begin() -> txn = txn { id: 1 }\n";
    clean(&format!(
        "{tx}fn t() {{ const t = begin()\n t.commit!!() }}"
    ));
    flagged(
        &format!("{tx}fn t() {{ const t = begin() }}"),
        "must be consumed",
    );
    flagged(
        &format!("{tx}fn t(c: bool) {{ const t = begin()\n if c {{ t.commit!!() }} }}"),
        "must be consumed",
    );
    clean(&format!("{tx}fn t(c: bool) {{ const t = begin()\n if c {{ t.commit!!() }} else {{ t.rollback!!() }} }}"));
    flagged(
        &format!("{tx}fn t(c: bool) {{ const t = begin()\n if c {{ return }}\n t.commit!!() }}"),
        "must be consumed before this `return`",
    );
    flagged(
        &format!("{tx}fn t() {{ begin()\n print(1) }}"),
        "thrown away",
    );
    flagged(
        &format!("{tx}fn t() {{ var t = begin()\n t = begin()\n t.commit!!() }}"),
        "assigning over it would drop",
    );
    flagged(
        &format!("{tx}fn t() {{ const t = begin()\n t.commit!!()\n t.commit!!() }}"),
        "was moved",
    );
    clean(&format!(
        "{tx}fn finish(sink t: txn) {{ t.commit!!() }}\nfn t() {{ finish(sink begin()) }}"
    ));
    flagged(
        &format!("{tx}fn finish(sink t: txn) {{ }}"),
        "must be consumed",
    );
    clean(&format!("{tx}fn peek(t: txn) -> int = t.id"));
    flagged(
        &format!("{tx}struct holder {{ t: txn }}"),
        "must be declared `linear` too",
    );
    clean(&format!("{tx}linear struct holder {{ t: txn }}"));
    flagged(
        "linear struct a { x: int\n deinit() { } }",
        "cannot have `deinit`",
    );
    flagged(&format!("{tx}fn t() -> int ! error {{ const t = begin()\n const n = \"1\".parse_int()?\n t.commit!!()\n ok(n) }}"), "before `?` can return");
}

const TXN: &str = "linear struct txn { id: int\n fn commit(sink self) { _ = self } }\nfn begin() -> txn = txn { id: 1 }\n";

#[test]
fn linear_type_parameters() {
    // A plain type parameter cannot hold a linear value.
    flagged(
        &format!(
            "{TXN}fn keep<T>(x: T) -> T = x\nfn t() {{ const a = keep(begin())\n a.commit!!() }}"
        ),
        "is `linear`, so it cannot be used for `T`",
    );
    clean(&format!("{TXN}fn keep<T: linear>(sink x: T) -> T = x\nfn t() {{ const a = keep(sink begin())\n a.commit!!() }}"));
    clean(&format!(
        "{TXN}fn keep<T>(x: T) -> T = x\nfn t() {{ const a = keep(1)\n print(a) }}"
    ));
    // Inside, a `T: linear` value must be consumed.
    flagged("fn drop_it<T: linear>(sink x: T) { }", "must be consumed");
    clean("fn pass<T: linear>(sink x: T) -> T = x");
    clean(
        "fn pass<T>(x: T) -> T where T: linear = x"
            .replace("(x: T)", "(sink x: T)")
            .as_str(),
    );
    // Containers of linear values are linear.
    flagged(
        &format!("{TXN}fn t() {{ var xs: [txn] = []\n xs.push!(begin()) }}"),
        "must be consumed",
    );
}

#[test]
fn linear_values_and_closures() {
    clean(&format!(
        "{TXN}fn t() {{ const a = begin()\n const f = [sink a] ~> a.commit!!()\n f() }}"
    ));
    flagged(
        &format!("{TXN}fn t() {{ const a = begin()\n const f = [sink a] ~> print(1)\n f() }}"),
        "must be consumed before the closure ends",
    );
    flagged(
        &format!("{TXN}fn t() {{ const a = begin()\n const f = ~> a.commit!!()\n f() }}"),
        "capture it with `[sink a]`",
    );
}

#[test]
fn unreachable_arms_are_warnings() {
    let mut program = crate::package::Program::with_core();
    program.add_package(
        "app",
        &[(
            "main",
            "fn t(n: int) -> int { switch n { case x: 1\n case 5: 2 } }",
        )],
    );
    let res = resolve(&program);
    let typed = typecheck(&program, &res);
    assert!(typed
        .diagnostics
        .iter()
        .any(|d| d.warning && d.message.contains("can never match")));
    assert!(typed.diagnostics.iter().all(|d| d.warning));
}

#[test]
fn boxes_follow_the_move_rules() {
    flagged(
        "struct p { name: string }\nfn t(b: box<p>) { const n = b.get().name }",
        "write `copy b.get().name`",
    );
    flagged(
        "fn t(b: box<string>) { const s = b.get() }",
        "write `copy b.get()`",
    );
    clean("struct p { name: string }\nfn t(b: box<p>) { const n = copy b.get().name }");
    clean("fn t(b: box<int>) -> int { b.get() + 1 }");
    clean("fn t(sink b: box<string>) -> string { b.take!!() }");
    flagged(
        "fn t(sink b: box<string>) { const s = b.take!!()\n print(b.get()) }",
        "`b`",
    );
    flagged(
        "fn t() { const b = box(1)\n b.set!(2) }",
        "`b` is a `const`",
    );
    clean("fn t() { var b = box(1)\n b.set!(2) }");
}

#[test]
fn scope_tasks_cannot_share_a_changed_variable() {
    flagged(
        "fn t() { var a = 0\n scope s { s.spawn([inout a] ~> { a += 1 })\n s.spawn([inout a] ~> { a += 2 }) } }",
        "captured by two tasks",
    );
    clean("fn t() { var a = 0\n var b = 0\n scope s { s.spawn([inout a] ~> { a += 1 })\n s.spawn([inout b] ~> { b += 2 }) } }");
}
