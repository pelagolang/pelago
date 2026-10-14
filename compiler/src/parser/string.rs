//! Strings: splitting one `Str` token into text and `#name` / `#(expr)` parts.

use super::{parse_fragment, Parser};
use crate::ast::*;
use crate::lexer::{is_ident_continue, is_ident_start};
use crate::span::Span;

impl Parser<'_> {
    /// Builds the parts of the string token with span `span` (quotes included).
    pub(super) fn string_parts(&mut self, span: Span) -> Vec<StrPart> {
        let src = self.src;
        let bytes = src.as_bytes();
        let start = span.start as usize + 1;
        let end = (span.end as usize).saturating_sub(1).max(start);
        let mut parts = Vec::new();
        let mut text_start = start;
        let mut i = start;
        while i < end {
            match bytes[i] {
                b'\\' => i += 2,
                b'#' => {
                    let after = i + 1;
                    if bytes.get(after) == Some(&b'(') {
                        let close = skip_parens(bytes, after, end);
                        if i > text_start {
                            parts.push(StrPart::Text(Span::new(text_start as u32, i as u32)));
                        }
                        let inner_start = after + 1;
                        let inner_end = close.min(end).max(inner_start);
                        let text = src.get(inner_start..inner_end).unwrap_or("");
                        let (expr, errors) = parse_fragment(src, inner_start as u32, text);
                        self.errors.extend(errors);
                        parts.push(StrPart::Interp(Box::new(expr)));
                        i = (close + 1).min(end.max(close + 1));
                        text_start = i;
                    } else if let Some(name_end) = ident_end(src, after, end) {
                        if i > text_start {
                            parts.push(StrPart::Text(Span::new(text_start as u32, i as u32)));
                        }
                        let id = Ident {
                            span: Span::new(after as u32, name_end as u32),
                        };
                        parts.push(StrPart::Interp(Box::new(Expr {
                            kind: ExprKind::Name(id),
                            span: id.span,
                        })));
                        i = name_end;
                        text_start = i;
                    } else {
                        i += 1;
                    }
                }
                _ => i += 1,
            }
        }
        if text_start < end {
            parts.push(StrPart::Text(Span::new(text_start as u32, end as u32)));
        }
        parts
    }
}

/// Where the identifier starting at `from` ends, if there is one.
fn ident_end(src: &str, from: usize, limit: usize) -> Option<usize> {
    let rest = src.get(from..limit)?;
    let mut chars = rest.char_indices();
    let (_, first) = chars.next()?;
    if !is_ident_start(first) {
        return None;
    }
    let mut end = from + first.len_utf8();
    for (offset, c) in chars {
        if !is_ident_continue(c) {
            return Some(from + offset);
        }
        end = from + offset + c.len_utf8();
    }
    Some(end)
}

/// `open` is the index of a `(`. Returns the index of its matching `)`, skipping nested strings
/// and character literals. Returns `limit` when there is none.
fn skip_parens(bytes: &[u8], open: usize, limit: usize) -> usize {
    let mut depth = 0u32;
    let mut i = open;
    while i < limit {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            b'"' => i = skip_string(bytes, i, limit),
            b'\'' => i = skip_char(bytes, i, limit),
            _ => {}
        }
        i += 1;
    }
    limit
}

/// `quote` is the index of an opening `"`. Returns the index of the closing one.
fn skip_string(bytes: &[u8], quote: usize, limit: usize) -> usize {
    let mut i = quote + 1;
    while i < limit {
        match bytes[i] {
            b'\\' => i += 1,
            b'"' => return i,
            b'#' if bytes.get(i + 1) == Some(&b'(') => {
                i = skip_parens(bytes, i + 1, limit);
            }
            _ => {}
        }
        i += 1;
    }
    limit
}

/// `quote` is the index of an opening `'`. Returns the index of the closing one.
fn skip_char(bytes: &[u8], quote: usize, limit: usize) -> usize {
    let mut i = quote + 1;
    while i < limit {
        match bytes[i] {
            b'\\' => i += 1,
            b'\'' => return i,
            _ => {}
        }
        i += 1;
    }
    limit
}
