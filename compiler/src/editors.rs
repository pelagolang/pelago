//! Editor support generated from the compiler's own tables (card B-03): the TextMate grammar used by
//! VS Code and many other editors, and `pelago keywords --json`.
//!
//! The grammar is `editors/textmate/template.tmLanguage.json` (hand written) with markers such as
//! `@CONTROL@` replaced by the keyword lists from `keyword.rs` and `token.rs`. The result is committed
//! as `editors/textmate/pelago.tmLanguage.json`; a test fails if the committed file is not what this
//! module generates, so the grammar cannot drift from the keyword list.

use crate::highlight::{keyword_class, Class, MORE_TYPES};
use crate::keyword::{Keyword, Strength, PRELUDE_FUNCTIONS, PRELUDE_TYPES};
use crate::token::Punct;

const TEMPLATE: &str = include_str!("../../editors/textmate/template.tmLanguage.json");

fn hard_in(class: Class, skip: &[&str]) -> Vec<&'static str> {
    Keyword::ALL
        .iter()
        .filter(|k| k.strength() == Strength::Hard && keyword_class(**k) == class)
        .map(|k| k.as_str())
        .filter(|w| !skip.contains(w))
        .collect()
}

/// The soft keywords that open a declaration (they act as modifiers before `fn`, `struct`, ...).
const SOFT_DECL: &[&str] = &[
    "type", "ext", "union", "static", "operator", "open", "abstract", "override", "final",
    "linear", "test",
];

fn regex_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if "\\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    // The text sits inside a JSON string: backslashes are doubled again.
    out.replace('\\', "\\\\")
}

fn operators() -> String {
    let skip = [
        "~>", "?", "??", "?.", "??=", "..", "..=", "...", "@", "(", ")", "[", "]", "{", "}", ",",
        ";", ":", ".",
    ];
    let mut ops: Vec<&str> = Punct::ALL
        .iter()
        .map(|p| p.as_str())
        .filter(|t| !skip.contains(t))
        .collect();
    ops.sort_by_key(|t| std::cmp::Reverse(t.len()));
    ops.iter()
        .map(|t| regex_escape(t))
        .collect::<Vec<_>>()
        .join("|")
}

/// The TextMate grammar for `.pel` files.
pub fn textmate_grammar() -> String {
    let mut types: Vec<&str> = PRELUDE_TYPES.to_vec();
    types.extend(MORE_TYPES);
    let soft_decl: Vec<&str> = Keyword::ALL
        .iter()
        .filter(|k| k.strength() == Strength::Soft)
        .map(|k| k.as_str())
        .filter(|w| SOFT_DECL.contains(w))
        .collect();
    let soft_all: Vec<&str> = Keyword::ALL
        .iter()
        .filter(|k| k.strength() == Strength::Soft)
        .map(|k| k.as_str())
        .collect();
    let marks: Vec<(&str, String)> = vec![
        ("@CONTROL@", hard_in(Class::Control, &[]).join("|")),
        ("@MODES@", hard_in(Class::Mode, &[]).join("|")),
        ("@BINDING@", hard_in(Class::Binding, &[]).join("|")),
        (
            "@DECLARATION@",
            hard_in(
                Class::Declaration,
                &[
                    "fn",
                    "struct",
                    "enum",
                    "interface",
                    "impl",
                    "obj",
                    "mod",
                    "pkg",
                ],
            )
            .join("|"),
        ),
        ("@CONSTANTS@", hard_in(Class::Constant, &[]).join("|")),
        ("@SELF@", hard_in(Class::SelfName, &[]).join("|")),
        ("@OTHER@", hard_in(Class::OtherKeyword, &[]).join("|")),
        ("@SOFT_DECL@", soft_decl.join("|")),
        ("@SOFT@", soft_all.join("|")),
        ("@TYPES@", types.join("|")),
        ("@FUNCTIONS@", PRELUDE_FUNCTIONS.join("|")),
        ("@OPERATORS@", operators()),
    ];
    let mut out = TEMPLATE.to_string();
    for (k, v) in marks {
        out = out.replace(k, &v);
    }
    out
}

/// The word lists as JSON, for tools that want them.
pub fn keywords_json() -> String {
    let list = |strength: Strength| -> String {
        Keyword::ALL
            .iter()
            .filter(|k| k.strength() == strength)
            .map(|k| format!("\"{}\"", k.as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let q = |items: &[&str]| -> String {
        items
            .iter()
            .map(|t| format!("\"{}\"", t.replace('\\', "\\\\").replace('"', "\\\"")))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let punct: Vec<&str> = Punct::ALL.iter().map(|p| p.as_str()).collect();
    let mut types: Vec<&str> = PRELUDE_TYPES.to_vec();
    types.extend(MORE_TYPES);
    format!(
        "{{\n  \"hard\": [{}],\n  \"soft\": [{}],\n  \"punctuation\": [{}],\n  \"types\": [{}],\n  \"functions\": [{}]\n}}",
        list(Strength::Hard),
        list(Strength::Soft),
        q(&punct),
        q(&types),
        q(PRELUDE_FUNCTIONS)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn committed() -> [std::path::PathBuf; 2] {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../editors");
        [
            root.join("textmate/pelago.tmLanguage.json"),
            root.join("vscode/syntaxes/pelago.tmLanguage.json"),
        ]
    }

    #[test]
    fn the_committed_grammar_is_the_generated_one() {
        let want = textmate_grammar();
        for path in committed() {
            if std::env::var_os("BLESS").is_some() {
                std::fs::write(&path, &want).unwrap();
            }
            let have = std::fs::read_to_string(&path).unwrap_or_default();
            assert_eq!(
                have,
                want,
                "{} is out of date; run `BLESS=1 cargo test editors` to regenerate it",
                path.display()
            );
        }
    }

    #[test]
    fn the_grammar_names_every_keyword() {
        let g = textmate_grammar();
        assert!(
            !g.contains('@') || !g.contains("@CONTROL@"),
            "a marker was not replaced"
        );
        for k in Keyword::ALL {
            if k.strength() == Strength::Hard {
                assert!(g.contains(k.as_str()), "the grammar lacks `{}`", k.as_str());
            }
        }
        for w in PRELUDE_TYPES {
            assert!(g.contains(w), "the grammar lacks the type `{w}`");
        }
        for w in PRELUDE_FUNCTIONS {
            assert!(g.contains(w), "the grammar lacks `{w}`");
        }
        for p in Punct::ALL {
            let t = p.as_str();
            if t.len() > 1 && !"~> ?? ?. ??= .. ..= ...".split(' ').any(|x| x == t) {
                assert!(
                    g.contains(&regex_escape(t)),
                    "the grammar lacks the operator `{t}`"
                );
            }
        }
    }

    #[test]
    fn keywords_json_has_all_lists() {
        let j = keywords_json();
        for key in ["hard", "soft", "punctuation", "types", "functions"] {
            assert!(j.contains(&format!("\"{key}\"")));
        }
        assert!(j.contains("\"inout\"") && j.contains("\"step\""));
    }
}
