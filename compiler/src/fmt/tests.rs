use super::format;
use crate::ast_print::print_file;
use crate::parser::parse_and_check;

fn fmt(src: &str) -> String {
    format(src).expect("formats")
}

#[test]
fn indents_by_open_brackets_and_trims() {
    let src = "fn main() {\n  const x = 1   \n  if x == 1 {\n  print(x)\n}\n}\n";
    assert_eq!(
        fmt(src),
        "fn main() {\n    const x = 1\n    if x == 1 {\n        print(x)\n    }\n}\n"
    );
}

#[test]
fn commas_blank_lines_and_trailing_comments() {
    let src = "\n\nfn f(a: int,b: int , c: int) {   /// note\n\n\n    g(a,b)\n\n}\n\n\n\nfn g() {}/// end\n";
    assert_eq!(
        fmt(src),
        "fn f(a: int, b: int, c: int) {   /// note\n    g(a, b)\n}\n\nfn g() {} /// end\n"
    );
}

#[test]
fn levels_the_author_added_are_kept() {
    let src = "fn f() {\n    const chain = items\n        .map(~> it)\n        .to_list()\n  switch x {\n        case 1:\n            g()\n  }\n}\n";
    assert_eq!(
        fmt(src),
        "fn f() {\n    const chain = items\n        .map(~> it)\n        .to_list()\n    switch x {\n        case 1:\n            g()\n    }\n}\n"
    );
    let calls = "fn f() {\n    g(\n  1,\n  2,\n)\n}\n";
    assert_eq!(
        fmt(calls),
        "fn f() {\n    g(\n        1,\n        2,\n    )\n}\n"
    );
}

#[test]
fn comments_keep_their_text_and_crlf_is_kept() {
    let src =
        "//~ Docs.\r\n//~* a\n b *~//\r\nfn f() {\r\n/// inside\r\n    ///* c */// f()\r\n}\r\n";
    let out = fmt(src);
    assert!(out.contains("//~* a\r\n b *~//"));
    assert!(out.contains("    /// inside\r\n"));
    assert!(out.contains("    ///* c */// f()"));
    assert!(out.ends_with("}\r\n"));
}

#[test]
fn files_with_errors_are_refused() {
    assert!(format("fn f( {\n").is_err());
    assert!(format("const s = \"abc\n").is_err());
}

#[test]
fn formatting_is_idempotent_and_keeps_the_tree_for_every_example() {
    let mut files = Vec::new();
    for dir in ["../examples", "../examples/programs"] {
        for e in std::fs::read_dir(format!("{}/{dir}", env!("CARGO_MANIFEST_DIR"))).unwrap() {
            let p = e.unwrap().path();
            if p.extension().and_then(|x| x.to_str()) == Some("pel") {
                files.push(p);
            }
        }
    }
    assert!(!files.is_empty());
    for p in files {
        let src = std::fs::read_to_string(&p).unwrap();
        let once = match format(&src) {
            Ok(o) => o,
            // A sample that is not meant to parse cleanly (a syntax tour of errors).
            Err(_) => continue,
        };
        assert_eq!(
            format(&once).unwrap(),
            once,
            "not idempotent: {}",
            p.display()
        );
        let tree = |s: &str| print_file(&parse_and_check(s).0.file, s);
        assert_eq!(tree(&src), tree(&once), "tree changed: {}", p.display());
    }
}
