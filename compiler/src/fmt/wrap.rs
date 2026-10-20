//! Line breaking. A line longer than the width is broken at the rightmost outermost `( )` or
//! `[ ]` group that holds a comma: each element goes on its own line, indented one level, with
//! a trailing comma, and the closing bracket gets its own line. Lines that are still too long
//! are broken again, element by element. Lines with comments or with a token that spans lines
//! (a long string) are left alone, and so are lines no group can shorten. A group is not
//! broken when its commas might not separate elements: a `;` or `of` at its top level
//! (comprehensions, array types) or an unbalanced `<` or `>` (generic arguments).

use crate::config::FmtConfig;
use crate::keyword::Keyword;
use crate::lexer::tokenize;
use crate::token::{Punct, Token, TokenKind};

const MAX_PASSES: usize = 10_000;

pub fn wrap(src: &str, config: &FmtConfig) -> String {
    let mut text = src.to_string();
    // Lines that no group could shorten, by start offset; skipped on later passes.
    let mut stuck: Vec<usize> = Vec::new();
    for _ in 0..MAX_PASSES {
        match break_one_line(&text, config, &mut stuck) {
            Some(next) => text = next,
            None => break,
        }
    }
    text
}

fn break_one_line(src: &str, config: &FmtConfig, stuck: &mut Vec<usize>) -> Option<String> {
    let tokens = tokenize(src).tokens;
    let mut line_start = 0;
    for line in src.split('\n') {
        let start = line_start;
        line_start += line.len() + 1;
        if line.chars().count() <= config.width || stuck.contains(&start) {
            continue;
        }
        match break_line(src, &tokens, start, line, config) {
            Some(next) => return Some(next),
            None => stuck.push(start),
        }
    }
    None
}

fn is_open(k: TokenKind) -> bool {
    matches!(
        k,
        TokenKind::Punct(Punct::LParen | Punct::LBracket | Punct::LBrace)
    )
}

fn is_close(k: TokenKind) -> bool {
    matches!(
        k,
        TokenKind::Punct(Punct::RParen | Punct::RBracket | Punct::RBrace)
    )
}

fn break_line(
    src: &str,
    tokens: &[Token],
    start: usize,
    line: &str,
    config: &FmtConfig,
) -> Option<String> {
    let end = start + line.len();
    let on_line: Vec<&Token> = tokens
        .iter()
        .filter(|t| (t.span.end as usize) > start && (t.span.start as usize) < end)
        .filter(|t| t.kind != TokenKind::Eof)
        .collect();
    let spans_lines = |t: &&Token| {
        t.kind != TokenKind::Newline && (t.span.start as usize) < start
            || (t.span.end as usize) > end && t.kind != TokenKind::Newline
    };
    if on_line.iter().any(spans_lines)
        || on_line
            .iter()
            .any(|t| t.kind.is_trivia() || t.kind == TokenKind::Error)
    {
        return None;
    }
    // Outermost balanced `( )` and `[ ]` groups with a comma at their top level.
    let mut groups: Vec<(usize, usize)> = Vec::new();
    let mut stack: Vec<(usize, TokenKind)> = Vec::new();
    for (i, t) in on_line.iter().enumerate() {
        if is_open(t.kind) {
            stack.push((i, t.kind));
        } else if is_close(t.kind) {
            let Some((o, ok)) = stack.pop() else { continue };
            let matches = matches!(
                (ok, t.kind),
                (
                    TokenKind::Punct(Punct::LParen),
                    TokenKind::Punct(Punct::RParen)
                ) | (
                    TokenKind::Punct(Punct::LBracket),
                    TokenKind::Punct(Punct::RBracket)
                ) | (
                    TokenKind::Punct(Punct::LBrace),
                    TokenKind::Punct(Punct::RBrace)
                )
            );
            if !matches {
                return None;
            }
            if stack.is_empty() && ok != TokenKind::Punct(Punct::LBrace) {
                groups.push((o, i));
            }
        }
    }
    let indent = line.len() - line.trim_start_matches(' ').len();
    let prefix_fits =
        |o: usize| src[start..on_line[o].span.end as usize].chars().count() <= config.width;
    let usable: Vec<(usize, usize)> = groups
        .into_iter()
        .filter(|&(o, c)| elements(src, &on_line, o, c).is_some_and(|e| e.len() > 1))
        .collect();
    let Some(&(o, c)) = usable
        .iter()
        .rev()
        .find(|&&(o, _)| prefix_fits(o))
        .or(usable.first())
    else {
        return break_binary(src, &on_line, start, indent, config);
    };
    let elems = elements(src, &on_line, o, c)?;
    let pad = |n: usize| " ".repeat(n);
    let mut out = String::new();
    out.push_str(&src[..on_line[o].span.end as usize]);
    out.push('\n');
    for e in &elems {
        out.push_str(&pad(indent + config.indent));
        out.push_str(e);
        out.push_str(",\n");
    }
    out.push_str(&pad(indent));
    out.push_str(&src[on_line[c].span.start as usize..]);
    Some(out)
}

/// How loosely a binary operator binds, for the operators that are safe to break after: `||`
/// breaks before `&&`, before `??`, before `+` and `-`. `None` for anything else.
pub fn break_rank(k: TokenKind) -> Option<u8> {
    match k {
        TokenKind::Punct(Punct::OrOr) => Some(0),
        TokenKind::Punct(Punct::AndAnd) => Some(1),
        TokenKind::Punct(Punct::QuestionQuestion) => Some(2),
        TokenKind::Punct(Punct::Plus | Punct::Minus) => Some(3),
        _ => None,
    }
}

/// Whether a token can end an operand, so that a `+` or `-` after it is a binary operator.
fn ends_operand(k: TokenKind) -> bool {
    matches!(
        k,
        TokenKind::Ident
            | TokenKind::Int
            | TokenKind::Float
            | TokenKind::Str
            | TokenKind::Char
            | TokenKind::Punct(Punct::RParen | Punct::RBracket | Punct::RBrace | Punct::Question)
    )
}

/// Breaks the line after every top-level operator of the loosest kind it holds, the pieces
/// indented one level.
fn break_binary(
    src: &str,
    on_line: &[&Token],
    start: usize,
    indent: usize,
    config: &FmtConfig,
) -> Option<String> {
    let mut depth = 0usize;
    let mut ops: Vec<(usize, u8)> = Vec::new();
    for (i, t) in on_line.iter().enumerate() {
        if is_open(t.kind) {
            depth += 1;
        } else if is_close(t.kind) {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && i > 0 && i + 1 < on_line.len() {
            let Some(rank) = break_rank(t.kind) else {
                continue;
            };
            if rank == 3 && !ends_operand(on_line[i - 1].kind) {
                continue;
            }
            ops.push((i, rank));
        }
    }
    let loosest = ops.iter().map(|&(_, r)| r).min()?;
    let mut out = String::from(&src[..start]);
    let mut from = start;
    for &(i, _) in ops.iter().filter(|&&(_, r)| r == loosest) {
        let op_end = on_line[i].span.end as usize;
        let next = on_line[i + 1].span.start as usize;
        out.push_str(&src[from..op_end]);
        out.push('\n');
        out.push_str(&" ".repeat(indent + config.indent));
        from = next;
    }
    out.push_str(&src[from..]);
    Some(out)
}

/// The text of each element between the group's brackets, split at top-level commas.
fn elements(src: &str, on_line: &[&Token], o: usize, c: usize) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut first: Option<usize> = None;
    let mut last = 0usize;
    let mut angles = 0i32;
    for t in &on_line[o + 1..c] {
        let (s, e) = (t.span.start as usize, t.span.end as usize);
        if depth == 0 {
            match t.kind {
                TokenKind::Punct(Punct::Semi) | TokenKind::Keyword(Keyword::Of) => return None,
                TokenKind::Punct(Punct::Lt) => angles += 1,
                TokenKind::Punct(Punct::Gt) => angles -= 1,
                TokenKind::Punct(Punct::Shr) => angles -= 2,
                _ => {}
            }
        }
        if depth == 0 && t.kind == TokenKind::Punct(Punct::Comma) {
            if angles != 0 {
                return None;
            }
            if let Some(f) = first.take() {
                out.push(src[f..last].to_string());
            }
            continue;
        }
        if is_open(t.kind) {
            depth += 1;
        } else if is_close(t.kind) {
            depth = depth.saturating_sub(1);
        }
        first.get_or_insert(s);
        last = e;
    }
    if angles != 0 {
        return None;
    }
    if let Some(f) = first {
        out.push(src[f..last].to_string());
    }
    Some(out)
}
