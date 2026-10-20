use super::json::{obj, text, Json};
use super::*;

const SRC: &str = "fn add(a: int, b: int) -> int { a + b }\nfn main() {\n    const x = add(1, 2)\n    print(x)\n}\n";
const URI: &str = "file:///work/demo.pel";

fn pos(line: i64, ch: i64) -> Json {
    obj(vec![
        ("line", Json::Num(line as f64)),
        ("character", Json::Num(ch as f64)),
    ])
}

fn request(id: i64, method: &str, params: Json) -> Json {
    obj(vec![
        ("jsonrpc", text("2.0")),
        ("id", Json::Num(id as f64)),
        ("method", text(method)),
        ("params", params),
    ])
}

fn note(method: &str, params: Json) -> Json {
    obj(vec![
        ("jsonrpc", text("2.0")),
        ("method", text(method)),
        ("params", params),
    ])
}

fn doc(uri: &str) -> Json {
    obj(vec![("uri", text(uri))])
}

fn open(s: &mut Server, uri: &str, src: &str) -> Vec<Json> {
    s.handle(&note(
        "textDocument/didOpen",
        obj(vec![(
            "textDocument",
            obj(vec![("uri", text(uri)), ("text", text(src))]),
        )]),
    ))
}

fn at(uri: &str, line: i64, ch: i64) -> Json {
    obj(vec![
        ("textDocument", doc(uri)),
        ("position", pos(line, ch)),
    ])
}

fn result(r: Vec<Json>) -> Json {
    r.into_iter()
        .next()
        .and_then(|m| m.get("result").cloned())
        .expect("a result")
}

fn diagnostics(msgs: &[Json]) -> Vec<(String, Vec<String>)> {
    msgs.iter()
        .filter(|m| m.get("method").and_then(Json::str) == Some("textDocument/publishDiagnostics"))
        .map(|m| {
            let p = m.get("params").unwrap();
            let msgs = p
                .get("diagnostics")
                .and_then(Json::arr)
                .unwrap()
                .iter()
                .map(|d| d.get("message").and_then(Json::str).unwrap().to_string())
                .collect();
            (p.get("uri").and_then(Json::str).unwrap().to_string(), msgs)
        })
        .collect()
}

fn started() -> Server {
    let mut s = Server::new();
    let r = s.handle(&request(
        1,
        "initialize",
        obj(vec![("rootUri", Json::Null)]),
    ));
    let caps = result(r);
    assert_eq!(
        caps.get("capabilities")
            .and_then(|c| c.get("hoverProvider")),
        Some(&Json::Bool(true))
    );
    s
}

#[test]
fn json_round_trip_and_errors() {
    let src = r#"{"a":[1,2.5,-3e2,true,null,"x\n\"y\u00e9\ud83d\ude00"],"b":{}}"#;
    let v = Json::parse(src).unwrap();
    assert_eq!(Json::parse(&v.dump()).unwrap(), v);
    assert_eq!(
        v.get("a").and_then(Json::arr).unwrap()[5].str(),
        Some("x\n\"y\u{e9}\u{1F600}")
    );
    for bad in [
        "",
        "{",
        "[1,]",
        "{\"a\" 1}",
        "\"abc",
        "tru",
        "[1] x",
        "\"\\q\"",
    ] {
        assert!(Json::parse(bad).is_err(), "{bad}");
    }
    // Deep nesting is refused, not a stack overflow.
    assert!(Json::parse(&"[".repeat(10_000)).is_err());
}

#[test]
fn json_reader_survives_random_input() {
    let mut x: u64 = 0x2545F4914F6CDD1D;
    let alphabet = b"{}[]\":,0123456789.-+eE tfnrul\\ua";
    for _ in 0..3000 {
        let mut s = String::new();
        for _ in 0..(x % 40) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            s.push(alphabet[(x % alphabet.len() as u64) as usize] as char);
        }
        let _ = Json::parse(&s);
    }
}

#[test]
fn frames_are_read_and_written() {
    let mut buf: Vec<u8> = Vec::new();
    write_frame(&mut buf, &obj(vec![("a", text("é"))])).unwrap();
    let mut two = buf.clone();
    two.extend_from_slice(&buf);
    let mut input = std::io::Cursor::new(two);
    let first = read_frame(&mut input).unwrap();
    assert_eq!(first, "{\"a\":\"é\"}");
    assert!(read_frame(&mut input).is_some());
    assert!(read_frame(&mut input).is_none());
}

#[test]
fn positions_count_utf16_units() {
    let src = "a😀b\nxy";
    let m = LineMap::new(src);
    assert_eq!(m.position(0), (0, 0));
    assert_eq!(m.position(1), (0, 1));
    assert_eq!(m.position(5), (0, 3)); // after the emoji: two UTF-16 units
    assert_eq!(m.position(6), (0, 4)); // after `b`
    assert_eq!(m.position(src.len()), (1, 2));
    assert_eq!(m.offset(0, 3), 5);
    assert_eq!(m.offset(1, 1), 8);
    assert_eq!(m.offset(9, 0), src.len());
    for off in 0..=src.len() {
        if src.is_char_boundary(off) {
            let (l, c) = m.position(off);
            assert_eq!(m.offset(l, c), off);
        }
    }
}

#[test]
fn diagnostics_come_and_go() {
    let mut s = started();
    let bad = "fn main() {\n    const x: int = \"no\"\n}\n";
    let d = diagnostics(&open(&mut s, URI, bad));
    assert_eq!(d.len(), 1);
    assert!(!d[0].1.is_empty(), "{d:?}");
    // Fixing the file clears them.
    let fixed = s.handle(&note(
        "textDocument/didChange",
        obj(vec![
            ("textDocument", doc(URI)),
            (
                "contentChanges",
                Json::Arr(vec![obj(vec![("text", text(SRC))])]),
            ),
        ]),
    ));
    let d = diagnostics(&fixed);
    assert_eq!(d, vec![(URI.to_string(), vec![])]);
}

#[test]
fn syntax_errors_are_reported() {
    let mut s = started();
    let d = diagnostics(&open(&mut s, URI, "fn main() {\n    const = \n}\n"));
    assert!(d.iter().any(|(_, m)| !m.is_empty()), "{d:?}");
}

#[test]
fn hover_definition_references() {
    let mut s = started();
    open(&mut s, URI, SRC);
    // Hover on `add` in the call (line 2, column 14).
    let h = result(s.handle(&request(2, "textDocument/hover", at(URI, 2, 14))));
    let value = h
        .get("contents")
        .and_then(|c| c.get("value"))
        .and_then(Json::str)
        .unwrap();
    assert!(value.contains("fn add(a: int, b: int) -> int"), "{value}");
    // Hover on the local `x` in `print(x)`.
    let h = result(s.handle(&request(3, "textDocument/hover", at(URI, 3, 10))));
    let value = h
        .get("contents")
        .and_then(|c| c.get("value"))
        .and_then(Json::str)
        .unwrap();
    assert!(value.contains("(local) x: int"), "{value}");
    // Hover on a literal gives its type.
    let h = result(s.handle(&request(4, "textDocument/hover", at(URI, 2, 18))));
    assert!(h
        .get("contents")
        .and_then(|c| c.get("value"))
        .and_then(Json::str)
        .unwrap()
        .contains("int"));
    // Go to definition of `add`.
    let d = result(s.handle(&request(5, "textDocument/definition", at(URI, 2, 14))));
    assert_eq!(d.get("uri").and_then(Json::str), Some(URI));
    let start = d.get("range").and_then(|r| r.get("start")).unwrap();
    assert_eq!(start.get("line").and_then(Json::int), Some(0));
    assert_eq!(start.get("character").and_then(Json::int), Some(3));
    // References to `add`: the declaration and the call.
    let mut params = at(URI, 2, 14);
    if let Json::Obj(f) = &mut params {
        f.push((
            "context".to_string(),
            obj(vec![("includeDeclaration", Json::Bool(true))]),
        ));
    }
    let r = result(s.handle(&request(6, "textDocument/references", params)));
    assert_eq!(r.arr().unwrap().len(), 2, "{r:?}");
    // Nothing to say about whitespace.
    let r = s.handle(&request(7, "textDocument/definition", at(URI, 1, 1)));
    assert_eq!(r[0].get("result"), Some(&Json::Null));
}

#[test]
fn outline_lists_declarations_with_members() {
    let mut s = started();
    let src = "struct point {\n    x: float\n    y: float\n    fn len(self) -> float = self.x\n}\nenum shape { circle(float), empty }\nfn main() { }\n";
    open(&mut s, URI, src);
    let r = result(s.handle(&request(
        2,
        "textDocument/documentSymbol",
        obj(vec![("textDocument", doc(URI))]),
    )));
    let names: Vec<&str> = r
        .arr()
        .unwrap()
        .iter()
        .map(|s| s.get("name").and_then(Json::str).unwrap())
        .collect();
    assert_eq!(names, ["point", "shape", "main"]);
    let kids: Vec<&str> = r.arr().unwrap()[0]
        .get("children")
        .and_then(Json::arr)
        .unwrap()
        .iter()
        .map(|s| s.get("name").and_then(Json::str).unwrap())
        .collect();
    assert_eq!(kids, ["x", "y", "len"]);
}

#[test]
fn semantic_tokens_are_ordered_and_cover_names() {
    let mut s = started();
    open(&mut s, URI, SRC);
    let r = result(s.handle(&request(
        2,
        "textDocument/semanticTokens/full",
        obj(vec![("textDocument", doc(URI))]),
    )));
    let data: Vec<i64> = r
        .get("data")
        .and_then(Json::arr)
        .unwrap()
        .iter()
        .map(|n| n.int().unwrap())
        .collect();
    assert!(!data.is_empty() && data.len().is_multiple_of(5));
    // Rebuild absolute positions: they never go backwards.
    let (mut line, mut col) = (0i64, 0i64);
    let mut types = Vec::new();
    for t in data.chunks(5) {
        line += t[0];
        col = if t[0] == 0 { col + t[1] } else { t[1] };
        assert!(t[2] > 0);
        types.push((line, col, TOKEN_TYPES[t[3] as usize]));
    }
    // `add` in the call is a function, `x` in `print(x)` a variable, `int` a type.
    assert!(types.contains(&(2, 14, "function")), "{types:?}");
    assert!(types.contains(&(3, 10, "variable")), "{types:?}");
    assert!(types.contains(&(0, 10, "parameter")) || types.contains(&(0, 7, "parameter")));
}

#[test]
fn a_workspace_package_sees_other_files() {
    let dir = std::env::temp_dir().join(format!("pelago-lsp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("util.pel"),
        "pub fn twice(x: int) -> int = x * 2\n",
    )
    .unwrap();
    let main_src = "import { twice } = .util\nfn main() {\n    print(twice(4))\n}\n";
    std::fs::write(dir.join("main.pel"), main_src).unwrap();
    let root = path_to_uri(&dir);
    let mut s = Server::new();
    s.handle(&request(
        1,
        "initialize",
        obj(vec![("rootUri", text(root.clone()))]),
    ));
    let main_uri = format!("{root}/main.pel");
    let d = diagnostics(&open(&mut s, &main_uri, main_src));
    assert!(d.iter().all(|(_, m)| m.is_empty()), "{d:?}");
    // `twice` is defined in the other file.
    let r = result(s.handle(&request(2, "textDocument/definition", at(&main_uri, 2, 10))));
    assert_eq!(
        r.get("uri").and_then(Json::str),
        Some(format!("{root}/util.pel").as_str())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unknown_requests_get_an_error_and_exit_works() {
    let mut s = started();
    let r = s.handle(&request(9, "workspace/whatever", Json::Null));
    assert_eq!(
        r[0].get("error")
            .and_then(|e| e.get("code"))
            .and_then(Json::int),
        Some(-32601)
    );
    s.handle(&request(10, "shutdown", Json::Null));
    s.handle(&note("exit", Json::Null));
    assert!(s.exited);
}

#[test]
fn the_server_survives_every_example() {
    for (i, src) in [
        include_str!("../../../examples/tour.pel"),
        include_str!("../../../examples/programs/shapes.pel"),
        include_str!("../../../examples/programs/words.pel"),
    ]
    .iter()
    .enumerate()
    {
        let mut s = started();
        let uri = format!("file:///work/ex{i}.pel");
        open(&mut s, &uri, src);
        // Ask about every position of the first lines: nothing may crash.
        for line in 0..40 {
            for col in [0, 3, 7, 12] {
                s.handle(&request(2, "textDocument/hover", at(&uri, line, col)));
                s.handle(&request(3, "textDocument/definition", at(&uri, line, col)));
            }
        }
        s.handle(&request(
            4,
            "textDocument/semanticTokens/full",
            obj(vec![("textDocument", doc(&uri))]),
        ));
    }
}

#[test]
fn completion_offers_members_after_a_dot_and_names_elsewhere() {
    let mut s = started();
    let src = "struct point {\n    x: float\n    y: float\n    fn len(self) -> float = self.x\n}\nfn main() {\n    const p = point { x: 1.0, y: 2.0 }\n    const total = 1\n    p.\n}\n";
    open(&mut s, URI, src);
    let labels = |r: Json| -> Vec<String> {
        r.arr()
            .unwrap()
            .iter()
            .map(|i| i.get("label").and_then(Json::str).unwrap().to_string())
            .collect()
    };
    // After `p.` (line 8, column 6): the members of `point`.
    let after_dot = labels(result(s.handle(&request(
        2,
        "textDocument/completion",
        at(URI, 8, 6),
    ))));
    assert!(
        after_dot.contains(&"x".to_string()) && after_dot.contains(&"len".to_string()),
        "{after_dot:?}"
    );
    // On a fresh line: keywords, declarations and the locals so far.
    let names = labels(result(s.handle(&request(
        3,
        "textDocument/completion",
        at(URI, 8, 4),
    ))));
    for want in ["fn", "point", "main", "p", "total", "print"] {
        assert!(names.contains(&want.to_string()), "{want}: {names:?}");
    }
}

fn code_actions(
    s: &mut Server,
    uri: &str,
    line: i64,
    from: i64,
    to: i64,
) -> Vec<(String, String, String)> {
    let params = obj(vec![
        ("textDocument", doc(uri)),
        (
            "range",
            obj(vec![("start", pos(line, from)), ("end", pos(line, to))]),
        ),
        ("context", obj(vec![("diagnostics", Json::Arr(vec![]))])),
    ]);
    let r = result(s.handle(&request(9, "textDocument/codeAction", params)));
    r.arr()
        .unwrap()
        .iter()
        .map(|a| {
            let e = &a
                .get("edit")
                .unwrap()
                .get("changes")
                .unwrap()
                .get(uri)
                .unwrap()
                .arr()
                .unwrap()[0];
            let r = e.get("range").unwrap();
            let p = |k: &str| {
                let p = r.get(k).unwrap();
                format!(
                    "{}:{}",
                    p.get("line").unwrap().int().unwrap(),
                    p.get("character").unwrap().int().unwrap()
                )
            };
            (
                a.get("title").and_then(Json::str).unwrap().to_string(),
                format!("{}-{}", p("start"), p("end")),
                e.get("newText").and_then(Json::str).unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn code_action_changes_a_misspelt_name_to_the_suggestion() {
    let mut s = started();
    let src = "fn total() -> int { 1 }\nfn main() {\n    const x = totl()\n    print(x)\n}\n";
    open(&mut s, URI, src);
    let acts = code_actions(&mut s, URI, 2, 14, 14);
    assert_eq!(
        acts,
        vec![(
            "Change to `total`".into(),
            "2:14-2:18".into(),
            "total".into()
        )]
    );
    // Nothing is offered away from the problem.
    assert!(code_actions(&mut s, URI, 0, 0, 0).is_empty());
}

#[test]
fn code_action_removes_unused_imports() {
    let mut s = started();
    let src = "import { print_line, other } = std.io\nimport std.fmt\nfn main() {\n    print_line(\"hi\")\n}\n";
    let out = open(&mut s, URI, src);
    // Unused imports show up as hints, without making the file fail to compile.
    let hints = out
        .iter()
        .flat_map(|m| {
            m.get("params")
                .and_then(|p| p.get("diagnostics"))
                .and_then(Json::arr)
                .unwrap_or(&[])
                .to_vec()
        })
        .filter(|d| d.get("code").and_then(Json::str) == Some("unused-import"))
        .count();
    assert_eq!(hints, 2);
    // The comma goes with the name.
    assert_eq!(
        code_actions(&mut s, URI, 0, 0, 40),
        vec![(
            "Remove unused import `other`".into(),
            "0:19-0:26".into(),
            String::new()
        )]
    );
    // A whole unused import takes its line.
    assert_eq!(
        code_actions(&mut s, URI, 1, 0, 20),
        vec![(
            "Remove unused import".into(),
            "1:0-2:0".into(),
            String::new()
        )]
    );
}

fn rename_params(uri: &str, line: i64, ch: i64, new: &str) -> Json {
    let mut p = at(uri, line, ch);
    if let Json::Obj(f) = &mut p {
        f.push(("newName".to_string(), text(new)));
    }
    p
}

#[test]
fn rename_changes_declaration_and_uses() {
    let mut s = started();
    open(&mut s, URI, SRC);
    let r = result(s.handle(&request(
        2,
        "textDocument/rename",
        rename_params(URI, 2, 14, "plus"),
    )));
    let edits = r
        .get("changes")
        .and_then(|c| c.get(URI))
        .and_then(Json::arr)
        .unwrap();
    assert_eq!(edits.len(), 2, "{r:?}");
    assert!(edits
        .iter()
        .all(|e| e.get("newText").and_then(Json::str) == Some("plus")));
    // A local, a keyword and a library name are refused.
    let r = s.handle(&request(
        3,
        "textDocument/rename",
        rename_params(URI, 2, 14, "fn"),
    ));
    assert!(r[0].get("error").is_some(), "{r:?}");
    let r = s.handle(&request(4, "textDocument/prepareRename", at(URI, 3, 4)));
    assert_eq!(r[0].get("result"), Some(&Json::Null), "print is built in");
    let p = result(s.handle(&request(5, "textDocument/prepareRename", at(URI, 2, 10))));
    assert_eq!(p.get("placeholder").and_then(Json::str), Some("x"));
}

#[test]
fn rename_reaches_other_files_of_the_package() {
    let dir = std::env::temp_dir().join(format!("pelago-lsp-rename-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("util.pel"),
        "pub fn twice(x: int) -> int = x * 2\n",
    )
    .unwrap();
    let main_src = "import { twice } = .util\nfn main() {\n    print(twice(4))\n}\n";
    std::fs::write(dir.join("main.pel"), main_src).unwrap();
    let root = path_to_uri(&dir);
    let mut s = Server::new();
    s.handle(&request(
        1,
        "initialize",
        obj(vec![("rootUri", text(root.clone()))]),
    ));
    let main_uri = format!("{root}/main.pel");
    open(&mut s, &main_uri, main_src);
    let r = result(s.handle(&request(
        2,
        "textDocument/rename",
        rename_params(&main_uri, 2, 10, "double"),
    )));
    let changes = r.get("changes").unwrap();
    let util = changes.get(&format!("{root}/util.pel")).and_then(Json::arr);
    assert_eq!(util.map(|u| u.len()), Some(1), "{r:?}");
    assert!(changes.get(&main_uri).and_then(Json::arr).unwrap().len() >= 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn signature_help_marks_the_parameter_being_written() {
    let mut s = started();
    let src = "fn add(a: int, b: int) -> int { a + b }\nfn main() {\n    const x = add(1, )\n}\n";
    open(&mut s, URI, src);
    let help = |s: &mut Server, id, ch| {
        result(s.handle(&request(id, "textDocument/signatureHelp", at(URI, 2, ch))))
    };
    // After `add(` and after the comma.
    let h = help(&mut s, 2, 18);
    assert_eq!(h.get("activeParameter").and_then(Json::int), Some(0));
    let sig = &h.get("signatures").and_then(Json::arr).unwrap()[0];
    assert_eq!(
        sig.get("label").and_then(Json::str),
        Some("fn add(a: int, b: int) -> int")
    );
    assert_eq!(sig.get("parameters").and_then(Json::arr).unwrap().len(), 2);
    let h = help(&mut s, 3, 21);
    assert_eq!(h.get("activeParameter").and_then(Json::int), Some(1));
    // Outside a call there is nothing.
    let r = s.handle(&request(4, "textDocument/signatureHelp", at(URI, 2, 8)));
    assert_eq!(r[0].get("result"), Some(&Json::Null));
}

fn error_message(r: &[Json]) -> String {
    r[0].get("error")
        .and_then(|e| e.get("message"))
        .and_then(Json::str)
        .unwrap_or_default()
        .to_string()
}

#[test]
fn rename_refuses_a_clash_and_allows_a_free_name() {
    let mut s = started();
    let src = "fn add(a: int, b: int) -> int { a + b }\nfn plus(a: int) -> int { a }\nfn main() {\n    print(add(1, 2))\n}\n";
    open(&mut s, URI, src);
    let r = s.handle(&request(
        2,
        "textDocument/rename",
        rename_params(URI, 3, 10, "plus"),
    ));
    assert!(error_message(&r).contains("would cause"), "{r:?}");
    // Inside a function: a parameter cannot take the name of another one.
    let r = s.handle(&request(
        3,
        "textDocument/rename",
        rename_params(URI, 0, 7, "b"),
    ));
    assert!(error_message(&r).contains("would cause"), "{r:?}");
    let r = s.handle(&request(
        4,
        "textDocument/rename",
        rename_params(URI, 0, 7, "left"),
    ));
    assert!(
        r[0].get("result").is_some_and(|v| *v != Json::Null),
        "{r:?}"
    );
}

#[test]
fn rename_follows_fields_methods_and_cases() {
    let mut s = started();
    let src = "struct point {\n    x: float\n    y: float\n    fn len(self) -> float = self.x\n}\nenum shape { circle(float), empty }\nfn main() {\n    const p = point { x: 1.0, y: 2.0 }\n    const q = p.x\n    print(q + p.len())\n    switch p {\n        case point { x: 1.0, y: _ }: print(1)\n        default: print(2)\n    }\n}\n";
    let uri = "file:///work/members.pel";
    let d = diagnostics(&open(&mut s, uri, src));
    assert!(d.iter().all(|(_, m)| m.is_empty()), "{d:?}");
    // `x`: the declaration, `self.x`, the literal, `p.x` and the pattern.
    let r = result(s.handle(&request(
        2,
        "textDocument/rename",
        rename_params(uri, 1, 4, "px"),
    )));
    let edits = r
        .get("changes")
        .and_then(|c| c.get(uri))
        .and_then(Json::arr)
        .unwrap();
    assert_eq!(edits.len(), 5, "{r:?}");
    // The method, from its call.
    let r = result(s.handle(&request(
        3,
        "textDocument/rename",
        rename_params(uri, 9, 18, "norm"),
    )));
    let edits = r
        .get("changes")
        .and_then(|c| c.get(uri))
        .and_then(Json::arr)
        .unwrap();
    assert_eq!(edits.len(), 2, "{r:?}");
    // A field name cannot take another field's name.
    let r = s.handle(&request(
        4,
        "textDocument/rename",
        rename_params(uri, 1, 4, "y"),
    ));
    assert!(error_message(&r).contains("would cause"), "{r:?}");
}
