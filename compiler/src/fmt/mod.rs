//! `pelago fmt`: the source formatter.
//!
//! This first stage works on the token stream, not on the tree. It fixes what is certain
//! without knowing the grammar: indentation (4 spaces, one level for each line that opens a
//! bracket and is still open, plus the levels the author added for things the brackets do not show,
//! such as a `case` body or a `.method()` chain), trailing
//! spaces, runs of spaces, the space after a comma and none before it, runs of blank lines
//! (at most one; none after `{` or before `}`), a single space before a trailing comment, and
//! one final newline. Everything else is left as the author wrote it. Comments are tokens, so
//! they are never lost, and their text is never touched.
//!
//! Safety: the output is lexed again and must hold the same tokens (same kinds and text, same
//! newlines) as the input, and the file must parse without errors, otherwise [`format`] returns
//! an error and the file is left alone. Formatting twice gives the same text.
//!
//! Stage 2 adds two things on top of that. Lines longer than the configured width are broken at
//! the rightmost outermost `( )` or `[ ]` group with a comma (one element per line, trailing
//! comma, closing bracket on its own line; see [`wrap`]). And the names inside
//! `import { ... }` are sorted, `...` last (see [`imports`]). Because both change more than
//! spacing, the output is also parsed and must print the same tree as the input.
//!
//! Not done yet: breaking long binary expressions, and fixing a line indented one level too far,
//! which needs the tree; see `docs/plans/cards/S-07-formatter.md`.

use crate::ast_print::print_file;
use crate::config::FmtConfig;
use crate::lexer::tokenize;
use crate::parser::parse_and_check;
use crate::token::{Punct, Token, TokenKind};

/// The indentation levels the author gave a line beyond what its brackets call for.
fn extra_levels(src: &str, t: &Token, depth: usize, indent: usize) -> usize {
    let start = t.span.start as usize;
    let line_start = src[..start].rfind('\n').map_or(0, |i| i + 1);
    let cols: usize = src[line_start..start]
        .chars()
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum();
    ((cols + indent / 2) / indent).saturating_sub(depth)
}

fn is_opener(k: TokenKind) -> bool {
    matches!(
        k,
        TokenKind::Punct(Punct::LParen | Punct::LBracket | Punct::LBrace)
    )
}

fn is_closer(k: TokenKind) -> bool {
    matches!(
        k,
        TokenKind::Punct(Punct::RParen | Punct::RBracket | Punct::RBrace)
    )
}

/// Formats a whole file. Fails, leaving nothing to write, when the file has syntax errors or the
/// result would not hold the same tokens.
pub fn format(src: &str) -> Result<String, String> {
    format_with(src, &FmtConfig::default())
}

/// Like [`format`], with the settings of a `pelago.toml`.
pub fn format_with(src: &str, config: &FmtConfig) -> Result<String, String> {
    let crlf = src.contains("\r\n");
    let src_lf = src.replace("\r\n", "\n");
    let src = src_lf.as_str();
    let lexed = tokenize(src);
    if !lexed.errors.is_empty() {
        return Err("the file has lexer errors".to_string());
    }
    let (parsed, _) = parse_and_check(src);
    if !parsed.errors.is_empty() {
        return Err("the file has syntax errors".to_string());
    }
    // Names in import lists are sorted first; everything after compares against that text.
    let sorted = imports::sort_imports(src);
    let src = sorted.as_str();
    let tokens = tokenize(src).tokens;
    let out = wrap::wrap(&layout(src, &tokens, config), config);
    if signature(src, &tokens) != signature(&out, &tokenize(&out).tokens) {
        return Err("formatting would change the tokens; not applied (this is a bug)".to_string());
    }
    let (before, _) = parse_and_check(src);
    let (after, _) = parse_and_check(&out);
    if !after.errors.is_empty() || print_file(&before.file, src) != print_file(&after.file, &out) {
        return Err("formatting would change the tree; not applied (this is a bug)".to_string());
    }
    Ok(if crlf { out.replace('\n', "\r\n") } else { out })
}

/// The tokens that must survive: kind and text, with every newline run reduced to one newline.
/// Newlines inside brackets and a comma right before a closing bracket do not count: breaking a
/// long list adds them.
fn signature(src: &str, tokens: &[Token]) -> Vec<(TokenKind, String)> {
    let mut depth = 0usize;
    let mut kept: Vec<&Token> = Vec::new();
    for t in tokens.iter().filter(|t| t.kind != TokenKind::Eof) {
        if is_opener(t.kind) {
            depth += 1;
        } else if is_closer(t.kind) {
            depth = depth.saturating_sub(1);
            if kept
                .last()
                .is_some_and(|p| p.kind == TokenKind::Punct(Punct::Comma))
            {
                kept.pop();
            }
        } else if t.kind == TokenKind::Newline && depth > 0 {
            continue;
        }
        kept.push(t);
    }
    kept.into_iter()
        .map(|t| {
            let text = &src[t.span.start as usize..t.span.end as usize];
            let text = if t.kind == TokenKind::Newline {
                "\n".to_string()
            } else {
                text.to_string()
            };
            (t.kind, text)
        })
        .collect()
}

fn layout(src: &str, tokens: &[Token], config: &FmtConfig) -> String {
    let text = |t: &Token| &src[t.span.start as usize..t.span.end as usize];
    let mut out = String::with_capacity(src.len());
    // The lines on which the brackets still open were opened.
    let mut open: Vec<usize> = Vec::new();
    let mut line = 0usize;
    // Newlines waiting to be written: 0 on a line, 1 for a break, 2 for a blank line too.
    let mut pending = 0usize;
    let mut prev: Option<&Token> = None;
    // The last token that was not a comment, to see whether the line above goes on.
    let mut last_code: Option<&Token> = None;
    for t in tokens {
        match t.kind {
            TokenKind::Eof => break,
            TokenKind::Newline => {
                pending = if text(t).matches('\n').count() > 1 {
                    2
                } else {
                    1
                };
                continue;
            }
            _ => {}
        }
        let closer = is_closer(t.kind);
        let after_opener = last_code.is_some_and(|p| is_opener(p.kind));
        if prev.is_none() {
            // The start of the file: no blank lines first.
        } else if pending > 0 {
            if closer || after_opener {
                pending = 1;
            }
            for _ in 0..pending {
                out.push('\n');
            }
            line += pending;
            // Indentation.
            // Brackets opened on the same line as the one this closes (`foo(x, {` ... `})`) do not
            // add a level to this line.
            let mut shown = open.clone();
            if closer {
                if let Some(l) = shown.pop() {
                    while shown.last() == Some(&l) {
                        shown.pop();
                    }
                }
                open.pop();
            }
            let mut depth = {
                shown.dedup();
                shown.len()
            };
            // What the brackets cannot tell: a `case` body, a `.method()` chain, the arms of a
            // ternary. Keep the levels the author added on top of the bracket depth.
            depth += extra_levels(src, t, depth, config.indent);
            for _ in 0..depth {
                out.extend(std::iter::repeat_n(' ', config.indent));
            }
            pending = 0;
        } else if let Some(p) = prev {
            let gap = &src[p.span.end as usize..t.span.start as usize];
            let comma_next = t.kind == TokenKind::Punct(Punct::Comma);
            let after_comma = p.kind == TokenKind::Punct(Punct::Comma);
            let trailing_comment =
                matches!(t.kind, TokenKind::LineComment | TokenKind::DocLineComment);
            if comma_next {
                // Nothing before a comma.
            } else if after_comma && !closer {
                out.push(' ');
            } else if !gap.is_empty() && gap.bytes().all(|b| b == b' ') {
                // Runs of spaces are kept: they align things on purpose (`else     {`).
                out.push_str(gap);
            } else if trailing_comment || !gap.is_empty() {
                out.push(' ');
            }
            if closer {
                open.pop();
            }
        }
        out.push_str(text(t));
        if is_opener(t.kind) {
            open.push(line);
        }
        if !t.kind.is_trivia() {
            last_code = Some(t);
        }
        prev = Some(t);
    }
    out.push('\n');
    out
}

mod imports;
mod wrap;

#[cfg(test)]
mod tests;
