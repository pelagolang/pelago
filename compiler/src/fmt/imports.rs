//! Sorts the names inside `import { b, a: x, ... } = path`: alphabetical by the imported name,
//! `...` last. Statements with a comment inside the braces are left alone. The order of names
//! does not change what the import means.

use crate::keyword::Keyword;
use crate::lexer::tokenize;
use crate::token::{Punct, TokenKind};

pub fn sort_imports(src: &str) -> String {
    let tokens = tokenize(src).tokens;
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let is_import = matches!(
            tokens[i].kind,
            TokenKind::Keyword(Keyword::Import | Keyword::Use)
        );
        let open = i + 1;
        if !is_import || tokens.get(open).map(|t| t.kind) != Some(TokenKind::Punct(Punct::LBrace)) {
            i += 1;
            continue;
        }
        // The names, split at commas up to the closing brace.
        let mut entries: Vec<(usize, usize)> = Vec::new();
        let mut cur: Option<(usize, usize)> = None;
        let mut j = open + 1;
        let mut clean = true;
        loop {
            let Some(t) = tokens.get(j) else {
                clean = false;
                break;
            };
            match t.kind {
                TokenKind::Punct(Punct::RBrace) => {
                    entries.extend(cur);
                    break;
                }
                TokenKind::Punct(Punct::Comma) => {
                    entries.extend(cur.take());
                }
                TokenKind::Newline => {}
                k if k.is_trivia() => clean = false,
                TokenKind::Ident | TokenKind::Punct(Punct::Colon | Punct::DotDotDot) => {
                    let (s, e) = (t.span.start as usize, t.span.end as usize);
                    cur = Some(cur.map_or((s, e), |(s0, _)| (s0, e)));
                }
                _ => {
                    clean = false;
                    break;
                }
            }
            j += 1;
        }
        if clean && entries.len() > 1 {
            let texts: Vec<&str> = entries.iter().map(|&(s, e)| &src[s..e]).collect();
            let mut sorted = texts.clone();
            sorted.sort_by_key(|t| (*t == "...", *t));
            for (&(s, e), new) in entries.iter().zip(&sorted) {
                edits.push((s, e, (*new).to_string()));
            }
        }
        i = j.max(i + 1);
    }
    let mut out = src.to_string();
    for (s, e, new) in edits.into_iter().rev() {
        out.replace_range(s..e, &new);
    }
    out
}
