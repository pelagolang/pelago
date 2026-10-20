//! Syntax highlighting for `.pel` (card B-03): `pelago highlight`.
//!
//! The classification uses the real lexer, so strings, numbers, comments and every hard keyword are
//! exact. Soft keywords and the roles of plain names (type, function) are decided by the tokens around
//! them, the same positions the parser uses. Every byte of the input belongs to exactly one piece, so
//! rendering never loses or repeats text (a test checks this on all the examples).

use crate::keyword::{Keyword, Strength, PRELUDE_FUNCTIONS, PRELUDE_TYPES};
use crate::lexer::tokenize;
use crate::token::{Punct, TokenKind};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Class {
    Plain,
    /// `if else switch for while return ...`
    Control,
    /// `fn struct enum interface impl obj mod import pub meta ...`
    Declaration,
    /// `var const`
    Binding,
    /// `inout sink copy`: the access modes.
    Mode,
    /// `true false none`
    Constant,
    /// `self Self super`
    SelfName,
    /// `unsafe async await`
    OtherKeyword,
    /// A soft keyword where it acts as one.
    SoftKeyword,
    Type,
    Function,
    Number,
    Char,
    Str,
    Escape,
    /// `#name` and `#( ... )` inside a string.
    Interpolation,
    Comment,
    DocComment,
    Attribute,
    Operator,
    /// `!` and `!!` after a method name.
    Mark,
    /// `~>`
    Arrow,
    /// `? ?? ?. ?:`
    Optional,
    Error,
}

impl Class {
    pub const ALL: &'static [Class] = &[
        Class::Plain,
        Class::Control,
        Class::Declaration,
        Class::Binding,
        Class::Mode,
        Class::Constant,
        Class::SelfName,
        Class::OtherKeyword,
        Class::SoftKeyword,
        Class::Type,
        Class::Function,
        Class::Number,
        Class::Char,
        Class::Str,
        Class::Escape,
        Class::Interpolation,
        Class::Comment,
        Class::DocComment,
        Class::Attribute,
        Class::Operator,
        Class::Mark,
        Class::Arrow,
        Class::Optional,
        Class::Error,
    ];

    /// The CSS class used by the HTML output.
    pub fn css(self) -> &'static str {
        match self {
            Class::Plain => "t-plain",
            Class::Control => "t-control",
            Class::Declaration => "t-decl",
            Class::Binding => "t-binding",
            Class::Mode => "t-mode",
            Class::Constant => "t-const",
            Class::SelfName => "t-self",
            Class::OtherKeyword => "t-kw",
            Class::SoftKeyword => "t-soft",
            Class::Type => "t-type",
            Class::Function => "t-fn",
            Class::Number => "t-num",
            Class::Char => "t-char",
            Class::Str => "t-str",
            Class::Escape => "t-esc",
            Class::Interpolation => "t-interp",
            Class::Comment => "t-comment",
            Class::DocComment => "t-doc",
            Class::Attribute => "t-attr",
            Class::Operator => "t-op",
            Class::Mark => "t-mark",
            Class::Arrow => "t-arrow",
            Class::Optional => "t-opt",
            Class::Error => "t-error",
        }
    }
}

/// A piece of the source and what it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piece {
    pub start: usize,
    pub end: usize,
    pub class: Class,
}

/// The colour class of a hard keyword.
pub fn keyword_class(k: Keyword) -> Class {
    match k {
        Keyword::True | Keyword::False | Keyword::None => Class::Constant,
        Keyword::SelfValue | Keyword::SelfType | Keyword::Super => Class::SelfName,
        Keyword::Var | Keyword::Const => Class::Binding,
        Keyword::Inout | Keyword::Sink | Keyword::Copy => Class::Mode,
        Keyword::Unsafe | Keyword::Async | Keyword::Await => Class::OtherKeyword,
        Keyword::If
        | Keyword::Else
        | Keyword::Unless
        | Keyword::Switch
        | Keyword::Case
        | Keyword::Guard
        | Keyword::For
        | Keyword::In
        | Keyword::Of
        | Keyword::While
        | Keyword::Do
        | Keyword::Loop
        | Keyword::Break
        | Keyword::Continue
        | Keyword::Return
        | Keyword::Defer
        | Keyword::Become
        | Keyword::Yield
        | Keyword::Is => Class::Control,
        _ => Class::Declaration,
    }
}

/// Type names that are not in the prelude list but are written like types.
pub const MORE_TYPES: &[&str] = &[
    "list", "iter", "task", "chan", "sender", "receiver", "shared", "lock", "fn",
];

/// Cuts the source into classified pieces that cover every byte.
pub fn classify(src: &str) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    classify_into(src, 0, &mut out);
    out
}

fn push(out: &mut Vec<Piece>, start: usize, end: usize, class: Class) {
    if start < end {
        out.push(Piece { start, end, class });
    }
}

fn classify_into(src: &str, base: usize, out: &mut Vec<Piece>) {
    let lexed = tokenize(src);
    let toks: Vec<_> = lexed
        .tokens
        .iter()
        .filter(|t| !matches!(t.kind, TokenKind::Eof))
        .copied()
        .collect();
    let mut at = 0usize;
    let text = |i: usize| -> &str { &src[toks[i].span.start as usize..toks[i].span.end as usize] };
    // The previous and next tokens that are not comments (newlines count: they end a statement).
    let prev_sig = |i: usize| -> Option<usize> {
        (0..i).rev().find(|j| {
            !matches!(
                toks[*j].kind,
                TokenKind::LineComment
                    | TokenKind::BlockComment
                    | TokenKind::DocLineComment
                    | TokenKind::DocBlockComment
            )
        })
    };
    let next_sig = |i: usize| -> Option<usize> {
        (i + 1..toks.len()).find(|j| {
            !matches!(
                toks[*j].kind,
                TokenKind::LineComment
                    | TokenKind::BlockComment
                    | TokenKind::DocLineComment
                    | TokenKind::DocBlockComment
            )
        })
    };
    // The first token of the line that token `i` is on.
    let line_start = |i: usize| -> usize {
        let mut j = i;
        while j > 0 && toks[j - 1].kind != TokenKind::Newline {
            j -= 1;
        }
        j
    };
    let is = |i: Option<usize>, p: Punct| -> bool {
        i.is_some_and(|i| toks[i].kind == TokenKind::Punct(p))
    };
    let kw = |i: Option<usize>| -> Option<Keyword> {
        i.and_then(|i| match toks[i].kind {
            TokenKind::Keyword(k) => Some(k),
            _ => None,
        })
    };
    let mut skip: Option<usize> = None;
    for i in 0..toks.len() {
        if skip == Some(i) {
            skip = None;
            continue;
        }
        let (s, e) = (toks[i].span.start as usize, toks[i].span.end as usize);
        push(out, base + at, base + s, Class::Plain);
        at = e;
        let prev = prev_sig(i);
        let next = next_sig(i);
        match toks[i].kind {
            TokenKind::Newline | TokenKind::Eof => push(out, base + s, base + e, Class::Plain),
            TokenKind::LineComment | TokenKind::BlockComment => {
                push(out, base + s, base + e, Class::Comment)
            }
            TokenKind::DocLineComment | TokenKind::DocBlockComment => {
                push(out, base + s, base + e, Class::DocComment)
            }
            TokenKind::Int | TokenKind::Float => push(out, base + s, base + e, Class::Number),
            TokenKind::Char => push(out, base + s, base + e, Class::Char),
            TokenKind::Error => push(out, base + s, base + e, Class::Error),
            TokenKind::Str => string_pieces(src, s, e, base, out),
            TokenKind::Underscore => push(out, base + s, base + e, Class::Plain),
            TokenKind::Keyword(k) => {
                let c = keyword_class(k);
                push(out, base + s, base + e, c);
            }
            TokenKind::Punct(p) => {
                let c = match p {
                    Punct::TildeArrow => Class::Arrow,
                    Punct::Question
                    | Punct::QuestionQuestion
                    | Punct::QuestionDot
                    | Punct::QuestionQuestionEq => Class::Optional,
                    Punct::Bang | Punct::BangBang
                        if prev.is_some_and(|j| {
                            matches!(toks[j].kind, TokenKind::Ident)
                                && toks[j].span.end as usize == s
                        }) =>
                    {
                        Class::Mark
                    }
                    Punct::At => {
                        // `@name`: the attribute covers both.
                        if let Some(n) = next {
                            if toks[n].kind == TokenKind::Ident && toks[n].span.start as usize == e
                            {
                                push(
                                    out,
                                    base + s,
                                    base + toks[n].span.end as usize,
                                    Class::Attribute,
                                );
                                at = toks[n].span.end as usize;
                                skip = Some(n);
                                continue;
                            }
                        }
                        Class::Operator
                    }
                    Punct::LParen
                    | Punct::RParen
                    | Punct::LBracket
                    | Punct::RBracket
                    | Punct::LBrace
                    | Punct::RBrace
                    | Punct::Comma
                    | Punct::Semi
                    | Punct::Colon
                    | Punct::Dot => Class::Plain,
                    _ => Class::Operator,
                };
                push(out, base + s, base + e, c);
            }
            TokenKind::Ident => {
                let w = text(i);
                let adjacent_next = |n: Option<usize>, p: Punct| {
                    is(n, p) && n.is_some_and(|n| toks[n].span.start as usize == e)
                };
                let called = adjacent_next(next, Punct::LParen)
                    || (matches!(next, Some(n) if matches!(toks[n].kind, TokenKind::Punct(Punct::Bang | Punct::BangBang))
                        && toks[n].span.start as usize == e)
                        && next_sig(next.unwrap_or(i))
                            .is_some_and(|m| toks[m].kind == TokenKind::Punct(Punct::LParen)));
                let after_dot = is(prev, Punct::Dot) || is(prev, Punct::QuestionDot);
                let ls = line_start(i);
                #[allow(clippy::if_same_then_else)]
                let class = if after_dot {
                    if called {
                        Class::Function
                    } else {
                        Class::Plain
                    }
                } else if kw(prev) == Some(Keyword::Fn) {
                    Class::Function
                } else if matches!(
                    kw(prev),
                    Some(
                        Keyword::Struct
                            | Keyword::Obj
                            | Keyword::Enum
                            | Keyword::Interface
                            | Keyword::Impl
                    )
                ) || prev.is_some_and(|p| {
                    matches!(toks[p].kind, TokenKind::Ident)
                        && text(p) == "type"
                        && line_start(p) == p
                }) {
                    Class::Type
                } else if soft_keyword_here(src, &toks, i, w, ls, prev, next) {
                    Class::SoftKeyword
                } else if PRELUDE_TYPES.contains(&w) || MORE_TYPES.contains(&w) {
                    Class::Type
                } else if PRELUDE_FUNCTIONS.contains(&w) || called {
                    Class::Function
                } else if is(prev, Punct::Arrow)
                    || type_position(src, &toks, i, ls, prev)
                    || generic_argument(&toks, i, prev, &text)
                {
                    Class::Type
                } else if adjacent_next(next, Punct::Lt)
                    && next_sig(next.unwrap_or(i)).is_some_and(|m| {
                        matches!(
                            toks[m].kind,
                            TokenKind::Ident | TokenKind::Punct(Punct::LBracket)
                        )
                    })
                    && w.chars().next().is_some_and(|c| c.is_lowercase())
                    && type_like_generic(src, &toks, i)
                {
                    Class::Type
                } else {
                    Class::Plain
                };
                push(out, base + s, base + e, class);
            }
        }
    }
    push(out, base + at, base + src.len(), Class::Plain);
}

/// `tree` in `box<tree>`: the first argument right after the `<` of a built-in generic type.
fn generic_argument<'s>(
    toks: &[crate::token::Token],
    i: usize,
    prev: Option<usize>,
    text: &dyn Fn(usize) -> &'s str,
) -> bool {
    let Some(p) = prev else { return false };
    if toks[p].kind != TokenKind::Punct(Punct::Lt) || toks[p].span.end != toks[i].span.start {
        return false;
    }
    p > 0
        && matches!(toks[p - 1].kind, TokenKind::Ident)
        && toks[p - 1].span.end == toks[p].span.start
        && {
            let w = text(p - 1);
            PRELUDE_TYPES.contains(&w) || MORE_TYPES.contains(&w)
        }
}

/// `x: int` in a parameter list, a field declaration or a `let`-less binding with a type.
fn type_position(
    src: &str,
    toks: &[crate::token::Token],
    i: usize,
    line_start: usize,
    prev: Option<usize>,
) -> bool {
    let Some(p) = prev else { return false };
    if toks[p].kind != TokenKind::Punct(Punct::Colon) {
        return false;
    }
    // A field or parameter: the line starts with `fn`, or with `name:` (after `pub`/`priv`).
    let mut j = line_start;
    while j < toks.len()
        && matches!(
            toks[j].kind,
            TokenKind::Keyword(Keyword::Pub | Keyword::Priv | Keyword::Var | Keyword::Const)
        )
    {
        j += 1;
    }
    let starts_fn = (line_start..i).any(|k| toks[k].kind == TokenKind::Keyword(Keyword::Fn));
    let field = j + 1 == p && toks[j].kind == TokenKind::Ident;
    let _ = src;
    starts_fn || field
}

/// `name<` where what follows looks like a type argument list (no spaces, closes with `>`).
fn type_like_generic(src: &str, toks: &[crate::token::Token], i: usize) -> bool {
    let lt = toks[i + 1];
    let rest = &src[lt.span.end as usize..];
    let line = rest.lines().next().unwrap_or("");
    line.contains('>') && !line.starts_with(' ')
}

/// A soft keyword in the position where it acts as one.
fn soft_keyword_here(
    src: &str,
    toks: &[crate::token::Token],
    i: usize,
    w: &str,
    line_start: usize,
    prev: Option<usize>,
    next: Option<usize>,
) -> bool {
    let Some(k) = Keyword::from_ident(w) else {
        return false;
    };
    if k.strength() != Strength::Soft {
        return false;
    }
    let dot = prev.is_some_and(|p| {
        matches!(
            toks[p].kind,
            TokenKind::Punct(Punct::Dot | Punct::QuestionDot | Punct::Colon)
        )
    });
    let nk = next.map(|n| toks[n].kind);
    let first = line_start == i
        || (line_start..i).all(|j| {
            matches!(
                toks[j].kind,
                TokenKind::Keyword(Keyword::Pub | Keyword::Priv)
            ) || matches!(toks[j].kind, TokenKind::Ident)
                && Keyword::from_ident(&src[toks[j].span.start as usize..toks[j].span.end as usize])
                    .is_some_and(|k| k.strength() == Strength::Soft)
        });
    let line_has = |p: Punct| (line_start..i).any(|j| toks[j].kind == TokenKind::Punct(p));
    match w {
        "to" => {
            !dot && prev.is_some_and(|p| {
                matches!(
                    toks[p].kind,
                    TokenKind::Ident
                        | TokenKind::Int
                        | TokenKind::Float
                        | TokenKind::Str
                        | TokenKind::Char
                        | TokenKind::Punct(Punct::RParen | Punct::RBracket)
                )
            }) && matches!(nk, Some(TokenKind::Ident))
        }
        "step" => !dot && (line_has(Punct::DotDot) || line_has(Punct::DotDotEq)),
        "it" => !dot && line_has(Punct::TildeArrow),
        "init" => {
            !dot && first && matches!(nk, Some(TokenKind::Punct(Punct::LParen | Punct::LBrace)))
        }
        "type" => {
            !dot && first && matches!(nk, Some(TokenKind::Ident)) && {
                let n = next.unwrap_or(i);
                next_after_is_eq(toks, n)
            }
        }
        "ext" | "union" | "static" | "operator" | "open" | "abstract" | "override" | "final"
        | "linear" => {
            !dot && first
                && matches!(
                    nk,
                    Some(TokenKind::Keyword(_) | TokenKind::Ident | TokenKind::Str)
                )
        }
        "test" => !dot && first && matches!(nk, Some(TokenKind::Str)),
        "where" => !dot && (line_start..i).any(|j| toks[j].kind == TokenKind::Keyword(Keyword::Fn)),
        "any" => {
            prev.is_some_and(|p| {
                matches!(
                    toks[p].kind,
                    TokenKind::Punct(
                        Punct::Colon | Punct::Arrow | Punct::Lt | Punct::Comma | Punct::LBracket
                    )
                )
            }) && matches!(nk, Some(TokenKind::Ident))
        }
        "scope" | "select" | "spawn" => {
            !dot && matches!(
                nk,
                Some(TokenKind::Punct(Punct::LBrace | Punct::LParen)) | Some(TokenKind::Ident)
            ) && (w != "scope" || matches!(nk, Some(TokenKind::Ident)))
        }
        "default" => !dot && first && matches!(nk, Some(TokenKind::Punct(Punct::LBrace))),
        "from" => {
            !dot && (line_start..i).any(|j| toks[j].kind == TokenKind::Keyword(Keyword::Import))
        }
        _ => false,
    }
}

fn next_after_is_eq(toks: &[crate::token::Token], n: usize) -> bool {
    toks.get(n + 1)
        .is_some_and(|t| t.kind == TokenKind::Punct(Punct::Eq))
}

/// The pieces of a string literal: text, escapes and interpolations.
fn string_pieces(src: &str, s: usize, e: usize, base: usize, out: &mut Vec<Piece>) {
    let b = src.as_bytes();
    let mut i = s;
    let mut text_from = s;
    while i < e {
        match b[i] {
            b'\\' if i + 1 < e => {
                push(out, base + text_from, base + i, Class::Str);
                let mut j = i + 2;
                if b[i + 1] == b'u' && j < e && b[j] == b'{' {
                    while j < e && b[j] != b'}' {
                        j += 1;
                    }
                    j = (j + 1).min(e);
                } else if b[i + 1] == b'x' {
                    j = (i + 4).min(e);
                }
                push(out, base + i, base + j, Class::Escape);
                i = j;
                text_from = i;
            }
            b'#' if i + 1 < e && b[i + 1] == b'(' => {
                push(out, base + text_from, base + i, Class::Str);
                // Find the matching `)`, skipping nested strings.
                let mut depth = 0usize;
                let mut j = i + 1;
                let mut in_str = false;
                while j < e {
                    let c = b[j];
                    if in_str {
                        if c == b'\\' {
                            j += 1;
                        } else if c == b'"' {
                            in_str = false;
                        }
                    } else if c == b'"' {
                        in_str = true;
                    } else if c == b'(' {
                        depth += 1;
                    } else if c == b')' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    j += 1;
                }
                let close = j.min(e - 1);
                push(out, base + i, base + i + 2, Class::Interpolation);
                // The expression inside is highlighted as code.
                if close > i + 2 {
                    classify_into(&src[i + 2..close], base + i + 2, out);
                }
                push(out, base + close, base + close + 1, Class::Interpolation);
                i = close + 1;
                text_from = i;
            }
            b'#' if i + 1 < e && (b[i + 1].is_ascii_alphabetic() || b[i + 1] == b'_') => {
                push(out, base + text_from, base + i, Class::Str);
                let mut j = i + 1;
                while j < e && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                    j += 1;
                }
                push(out, base + i, base + j, Class::Interpolation);
                i = j;
                text_from = i;
            }
            _ => i += 1,
        }
    }
    push(out, base + text_from, base + e, Class::Str);
}

// ---- Rendering -----------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Theme {
    Dark,
    Light,
}

/// The colour of a class: a 256-colour code and a CSS colour, plus bold or italic.
struct Style {
    ansi: u8,
    css: &'static str,
    bold: bool,
    italic: bool,
}

fn style(class: Class, theme: Theme) -> Option<Style> {
    let d = theme == Theme::Dark;
    let (ansi, css_dark, css_light, bold, italic) = match class {
        Class::Plain => return None,
        Class::Control => (176, "#c586c0", "#af00db", false, false),
        Class::Declaration => (75, "#569cd6", "#0000ff", false, false),
        Class::Binding => (75, "#569cd6", "#0000ff", true, false),
        Class::Mode => (208, "#ff8c00", "#b45f06", true, false),
        Class::Constant => (75, "#4fc1ff", "#0070c1", false, false),
        Class::SelfName => (74, "#9cdcfe", "#001080", false, true),
        Class::OtherKeyword => (176, "#c586c0", "#af00db", false, false),
        Class::SoftKeyword => (110, "#7aa6da", "#2b6cb0", false, false),
        Class::Type => (79, "#4ec9b0", "#267f99", false, false),
        Class::Function => (222, "#dcdcaa", "#795e26", false, false),
        Class::Number => (151, "#b5cea8", "#098658", false, false),
        Class::Char => (180, "#ce9178", "#a31515", false, false),
        Class::Str => (180, "#ce9178", "#a31515", false, false),
        Class::Escape => (215, "#d7ba7d", "#ee0000", false, false),
        Class::Interpolation => (117, "#9cdcfe", "#0451a5", true, false),
        Class::Comment => (102, "#6a9955", "#008000", false, true),
        Class::DocComment => (108, "#7ca668", "#00692c", false, true),
        Class::Attribute => (179, "#d7ba7d", "#af5f00", false, false),
        Class::Operator => (252, "#d4d4d4", "#383838", false, false),
        Class::Mark => (203, "#f44747", "#cd3131", true, false),
        Class::Arrow => (203, "#f48771", "#d9480f", true, false),
        Class::Optional => (141, "#b392f0", "#6f42c1", false, false),
        Class::Error => (196, "#ff0000", "#ff0000", true, false),
    };
    Some(Style {
        ansi,
        css: if d { css_dark } else { css_light },
        bold,
        italic,
    })
}

/// The source with no colour at all (what the pieces were cut from).
pub fn render_plain(src: &str, pieces: &[Piece]) -> String {
    pieces.iter().map(|p| &src[p.start..p.end]).collect()
}

/// Terminal colours (256-colour escape codes). Each coloured piece is reset right after it.
pub fn render_ansi(src: &str, pieces: &[Piece], theme: Theme) -> String {
    let mut out = String::with_capacity(src.len() * 2);
    for p in pieces {
        let text = &src[p.start..p.end];
        match style(p.class, theme) {
            None => out.push_str(text),
            Some(st) => {
                // Colours do not carry across a newline, so each line is wrapped separately.
                for (i, line) in text.split('\n').enumerate() {
                    if i > 0 {
                        out.push('\n');
                    }
                    if line.is_empty() {
                        continue;
                    }
                    out.push_str("\x1b[");
                    if st.bold {
                        out.push_str("1;");
                    }
                    if st.italic {
                        out.push_str("3;");
                    }
                    out.push_str(&format!("38;5;{}m{line}\x1b[0m", st.ansi));
                }
            }
        }
    }
    out
}

fn escape_html(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
}

/// The CSS for the `t-*` classes in both themes (light by default, dark through `prefers-color-scheme`).
pub fn css() -> String {
    let mut s = String::new();
    s.push_str("pre.pelago { background: #ffffff; color: #1f1f1f; padding: 1em; overflow-x: auto; font-family: ui-monospace, Menlo, Consolas, monospace; font-size: 14px; line-height: 1.45; }\n");
    for c in Class::ALL {
        if let Some(st) = style(*c, Theme::Light) {
            s.push_str(&format!(
                "pre.pelago .{} {{ color: {};{}{} }}\n",
                c.css(),
                st.css,
                if st.bold { " font-weight: bold;" } else { "" },
                if st.italic {
                    " font-style: italic;"
                } else {
                    ""
                }
            ));
        }
    }
    s.push_str("@media (prefers-color-scheme: dark) {\n  pre.pelago { background: #1e1e1e; color: #d4d4d4; }\n");
    for c in Class::ALL {
        if let Some(st) = style(*c, Theme::Dark) {
            s.push_str(&format!(
                "  pre.pelago .{} {{ color: {}; }}\n",
                c.css(),
                st.css
            ));
        }
    }
    s.push_str("}\n");
    s
}

/// HTML: a `<pre class="pelago">` with a `<span class="t-...">` around each coloured piece.
/// With `page`, a whole document with the CSS inline.
pub fn render_html(src: &str, pieces: &[Piece], page: bool) -> String {
    let mut body = String::from("<pre class=\"pelago\"><code>");
    for p in pieces {
        let text = &src[p.start..p.end];
        if p.class == Class::Plain {
            escape_html(text, &mut body);
        } else {
            body.push_str(&format!("<span class=\"{}\">", p.class.css()));
            escape_html(text, &mut body);
            body.push_str("</span>");
        }
    }
    body.push_str("</code></pre>\n");
    if !page {
        return body;
    }
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>Pelago source</title>\n<style>\nbody {{ margin: 0; }}\n{}</style>\n</head>\n<body>\n{body}</body>\n</html>\n",
        css()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes(src: &str) -> Vec<(String, Class)> {
        classify(src)
            .into_iter()
            .filter(|p| p.class != Class::Plain)
            .map(|p| (src[p.start..p.end].to_string(), p.class))
            .collect()
    }

    fn has(src: &str, text: &str, class: Class) -> bool {
        classes(src).iter().any(|(t, c)| t == text && *c == class)
    }

    #[test]
    fn pieces_cover_the_source_exactly() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples");
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .chain(std::fs::read_dir(dir.join("programs")).unwrap())
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "pel"))
            .collect();
        files.push(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("core/core.pel"));
        assert!(files.len() > 5);
        for f in files {
            let src = std::fs::read_to_string(&f).unwrap();
            let pieces = classify(&src);
            assert_eq!(render_plain(&src, &pieces), src, "{}", f.display());
            // Pieces are in order and do not overlap.
            let mut at = 0;
            for p in &pieces {
                assert!(
                    p.start >= at && p.start < p.end,
                    "{} at {}",
                    f.display(),
                    p.start
                );
                at = p.end;
            }
            // The colour codes strip back to the same text.
            let ansi = render_ansi(&src, &pieces, Theme::Dark);
            let mut stripped = String::new();
            let mut chars = ansi.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\x1b' {
                    for d in chars.by_ref() {
                        if d == 'm' {
                            break;
                        }
                    }
                } else {
                    stripped.push(c);
                }
            }
            assert_eq!(stripped, src, "{}", f.display());
        }
    }

    #[test]
    fn keywords_and_modes() {
        let src = "fn bump(inout c: counter) -> int {\n    var n = 1\n    if n > 0 { return n } else { none }\n}\n";
        assert!(has(src, "fn", Class::Declaration));
        assert!(has(src, "inout", Class::Mode));
        assert!(has(src, "var", Class::Binding));
        assert!(has(src, "if", Class::Control));
        assert!(has(src, "return", Class::Control));
        assert!(has(src, "bump", Class::Function));
        assert!(has(src, "int", Class::Type));
        assert!(has(src, "1", Class::Number));
    }

    #[test]
    fn strings_have_escapes_and_interpolation() {
        let src = "const s = \"a\\n #name and #(f(\"x#y\") + 1) ##\"\n";
        let c = classes(src);
        assert!(c.iter().any(|(t, k)| t == "\\n" && *k == Class::Escape));
        assert!(c
            .iter()
            .any(|(t, k)| t == "#name" && *k == Class::Interpolation));
        assert!(c
            .iter()
            .any(|(t, k)| t == "#(" && *k == Class::Interpolation));
        assert!(c.iter().any(|(t, k)| t == "f" && *k == Class::Function));
        assert!(c.iter().any(|(t, k)| t == "1" && *k == Class::Number));
    }

    #[test]
    fn soft_keywords_only_where_they_act() {
        assert!(has("const n = 5 to string\n", "to", Class::SoftKeyword));
        assert!(!has("const to = 5\n", "to", Class::SoftKeyword));
        assert!(!has("print(p.to)\n", "to", Class::SoftKeyword));
        assert!(has(
            "for i of 0..10 step 2 { }\n",
            "step",
            Class::SoftKeyword
        ));
        assert!(!has("const step = 1\n", "step", Class::SoftKeyword));
        assert!(has(
            "struct a {\n    init(x: int) { }\n}\n",
            "init",
            Class::SoftKeyword
        ));
        assert!(!has("const it = 3\n", "it", Class::SoftKeyword));
        assert!(has("xs.map(~> it * 2)\n", "it", Class::SoftKeyword));
    }

    #[test]
    fn marks_arrows_and_optionals() {
        let src = "c.bump!()\nc.close!!()\nconst f = x ~> x + 1\nconst y = a ?? b\nconst z = a?.b\nconst k = !flag\n";
        let c = classes(src);
        assert!(c.iter().any(|(t, k)| t == "!" && *k == Class::Mark));
        assert!(c.iter().any(|(t, k)| t == "!!" && *k == Class::Mark));
        assert!(c.iter().any(|(t, k)| t == "~>" && *k == Class::Arrow));
        assert!(c.iter().any(|(t, k)| t == "??" && *k == Class::Optional));
        assert!(c.iter().any(|(t, k)| t == "?." && *k == Class::Optional));
        // A prefix `!` is an ordinary operator.
        assert!(c.iter().any(|(t, k)| t == "!" && *k == Class::Operator));
    }

    #[test]
    fn comments_attributes_and_types() {
        let src = "/// note\n//~ doc\n///* block ///* nested */// end *///\n@derive(eq)\nstruct point { x: float, y: [float] }\nfn f(xs: list<int>) -> int? { }\n";
        assert!(has(src, "/// note", Class::Comment));
        assert!(has(src, "//~ doc", Class::DocComment));
        assert!(has(
            src,
            "///* block ///* nested */// end *///",
            Class::Comment
        ));
        assert!(has(src, "@derive", Class::Attribute));
        assert!(has(src, "point", Class::Type));
        assert!(has(src, "float", Class::Type));
        assert!(has(src, "list", Class::Type));
        assert!(has(src, "int", Class::Type));
    }

    #[test]
    fn html_escapes_and_wraps() {
        let src = "const a = x < 3 && \"<b>\"\n";
        let html = render_html(src, &classify(src), false);
        assert!(html.contains("&lt;b&gt;"));
        assert!(html.contains("<span class=\"t-binding\">const</span>"));
        let page = render_html(src, &classify(src), true);
        assert!(page.starts_with("<!doctype html>") && page.contains("prefers-color-scheme"));
    }
}
