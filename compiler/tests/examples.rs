//! Every file in `examples/` must lex and parse without errors.

use langc::lexer::tokenize;
use langc::token::TokenKind;

fn examples() -> Vec<(String, String)> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../examples");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).expect("examples directory") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) == Some("lang") {
            let text = std::fs::read_to_string(&path).unwrap();
            found.push((path.display().to_string(), text));
        }
    }
    assert!(!found.is_empty(), "no examples found");
    found
}

#[test]
fn examples_lex_without_errors() {
    for (path, src) in examples() {
        let out = tokenize(&src);
        assert!(out.errors.is_empty(), "{path}: {:?}", out.errors);
        assert!(
            out.tokens.iter().all(|t| t.kind != TokenKind::Error),
            "{path}: error token"
        );
    }
}

#[test]
fn tour_uses_the_interesting_tokens() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../examples/tour.lang"
    ))
    .unwrap();
    let out = tokenize(&src);
    let has = |kind: TokenKind| out.tokens.iter().any(|t| t.kind == kind);
    assert!(has(TokenKind::DocBlockComment));
    assert!(has(TokenKind::DocLineComment));
    assert!(has(TokenKind::Char));
    assert!(has(TokenKind::Float));
    assert!(has(TokenKind::Underscore) || src.contains("..."));
}

#[test]
fn examples_parse_without_errors() {
    for (path, src) in examples() {
        let (out, lex_errors) = langc::parser::parse_and_check(&src);
        assert!(lex_errors.is_empty(), "{path}: {lex_errors:?}");
        assert!(out.errors.is_empty(), "{path}: {:?}", out.errors);
    }
}

/// The printed syntax tree of every example is checked in under `tests/golden/`.
/// Run with `BLESS=1 cargo test` to rewrite them after an intended change.
#[test]
fn examples_match_their_golden_trees() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden");
    for (path, src) in examples() {
        let (out, _) = langc::parser::parse_source(&src);
        let printed = langc::ast_print::print_file(&out.file, &src) + "\n";
        let name = std::path::Path::new(&path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap()
            .to_string();
        let golden = format!("{dir}/{name}.ast");
        if std::env::var_os("BLESS").is_some() {
            std::fs::write(&golden, &printed).unwrap();
            continue;
        }
        let expected = std::fs::read_to_string(&golden)
            .unwrap_or_else(|_| panic!("missing {golden}; run `BLESS=1 cargo test`"));
        assert!(
            printed == expected,
            "{name}: the syntax tree changed; if that is intended run `BLESS=1 cargo test` and review the diff of {golden}"
        );
    }
}

/// Real programs in `examples/programs/` must type check cleanly.
#[test]
fn programs_check_cleanly() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../examples/programs");
    let mut count = 0;
    for entry in std::fs::read_dir(dir).expect("examples/programs") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("lang") {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap()
            .to_string();
        let mut program = langc::package::Program::with_core();
        program.add_package("app", &[(name.as_str(), src.as_str())]);
        let resolved = langc::resolve::resolve(&program);
        let typed = langc::typecheck::check(&program, &resolved);
        let mut all = typed.diagnostics.clone();
        all.extend(langc::values::check(&program, &resolved, &typed));
        let messages: Vec<String> = all
            .iter()
            .map(|d| format!("{}: {}", name, d.message))
            .collect();
        assert!(messages.is_empty(), "{path:?}: {messages:?}");
        count += 1;
    }
    assert!(count >= 3, "expected the sample programs");
}
