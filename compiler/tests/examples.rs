//! Every file in `examples/` must lex without errors.

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
