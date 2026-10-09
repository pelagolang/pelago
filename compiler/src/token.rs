//! Tokens: the pieces the lexer cuts source text into.

use crate::keyword::Keyword;
use crate::span::Span;

macro_rules! puncts {
    ( $( $(#[$doc:meta])* $variant:ident => $text:literal; )* ) => {
        /// Operators and punctuation.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Punct {
            $( $(#[$doc])* $variant, )*
        }

        impl Punct {
            /// All punctuation, in declaration order.
            pub const ALL: &'static [Punct] = &[ $( Punct::$variant, )* ];

            /// The source text of the token.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $( Punct::$variant => $text, )*
                }
            }
        }
    };
}

// The lexer uses *longest match*: at each position it takes the longest text in this
// table, so `>>=` is one token, not `>` `>` `=`. The parser is responsible for
// splitting `>>`, `>=` and `>>=` again when they close generic argument lists
// (`map<string, list<int>>`).
//
// `!!` is one token because it marks a consuming call (`c.close!!()`). In prefix
// position the parser reads it as two `!` (double negation).
//
// Not here: `#` (only meaningful inside strings), `=>` (undecided, depends on the
// `switch` arm syntax), and `_` (a plain identifier-like token, see `TokenKind::Underscore`).
puncts! {
    // Arithmetic
    Plus => "+";
    Minus => "-";
    Star => "*";
    Slash => "/";
    Percent => "%";
    StarStar => "**";
    // Comparison
    EqEq => "==";
    NotEq => "!=";
    Lt => "<";
    Gt => ">";
    LtEq => "<=";
    GtEq => ">=";
    // Logical
    AndAnd => "&&";
    OrOr => "||";
    Bang => "!";
    /// Marks a call that consumes its receiver: `c.close!!()`.
    BangBang => "!!";
    // Bitwise
    Amp => "&";
    Pipe => "|";
    Caret => "^";
    Tilde => "~";
    Shl => "<<";
    Shr => ">>";
    // Assignment
    Eq => "=";
    PlusEq => "+=";
    MinusEq => "-=";
    StarEq => "*=";
    SlashEq => "/=";
    PercentEq => "%=";
    AmpEq => "&=";
    PipeEq => "|=";
    CaretEq => "^=";
    ShlEq => "<<=";
    ShrEq => ">>=";
    /// Statement-only increment and decrement.
    PlusPlus => "++";
    MinusMinus => "--";
    // Ranges and access
    DotDot => "..";
    DotDotEq => "..=";
    DotDotDot => "...";
    Dot => ".";
    // The question-mark family
    Question => "?";
    QuestionQuestion => "??";
    QuestionDot => "?.";
    QuestionQuestionEq => "??=";
    // Arrows
    /// Return type.
    Arrow => "->";
    /// Closure: `x ~> x * 2`.
    TildeArrow => "~>";
    /// Pipeline: `x |> trim |> parse`.
    PipeGt => "|>";
    // Separators
    Colon => ":";
    Comma => ",";
    Semi => ";";
    At => "@";
    LParen => "(";
    RParen => ")";
    LBracket => "[";
    RBracket => "]";
    LBrace => "{";
    RBrace => "}";
}

/// What a token is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// A name. Soft keywords are identifiers; see `Keyword::from_ident`.
    Ident,
    /// A hard keyword.
    Keyword(Keyword),
    /// A lone `_`.
    Underscore,
    /// `42`, `1_000_000`, `0xFF`, `0b1010`, `0o17`.
    Int,
    /// `3.14`, `1e-3`.
    Float,
    /// `'a'`.
    Char,
    /// `"text"`, including `#name` and `#(expr)` parts, which a later pass splits.
    Str,
    /// Operators and punctuation.
    Punct(Punct),
    /// A newline that ends a statement. The parser ignores it inside brackets and
    /// when the line obviously continues.
    Newline,
    /// `// ...`
    LineComment,
    /// `/* ... */`
    BlockComment,
    /// `/// ...`
    DocLineComment,
    /// `///* ... *///`
    DocBlockComment,
    /// End of input.
    Eof,
    /// Something the lexer could not read. The span says where.
    Error,
}

impl TokenKind {
    /// Comments are trivia: the parser skips them, the formatter keeps them.
    pub const fn is_trivia(self) -> bool {
        matches!(
            self,
            TokenKind::LineComment
                | TokenKind::BlockComment
                | TokenKind::DocLineComment
                | TokenKind::DocBlockComment
        )
    }
}

/// A token and where it came from. The text is not stored: slice the source with the span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub const fn new(kind: TokenKind, span: Span) -> Token {
        Token { kind, span }
    }

    /// The source text of the token.
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        src.get(self.span.start as usize..self.span.end as usize)
            .unwrap_or("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn punct_texts_are_unique() {
        let mut seen = HashSet::new();
        for p in Punct::ALL {
            assert!(
                seen.insert(p.as_str()),
                "duplicate punctuation {:?}",
                p.as_str()
            );
        }
    }

    #[test]
    fn punct_texts_are_ascii_symbols() {
        for p in Punct::ALL {
            let s = p.as_str();
            assert!(!s.is_empty() && s.len() <= 3);
            assert!(s.chars().all(|c| c.is_ascii_punctuation()), "{s:?}");
        }
    }

    #[test]
    fn longest_match_has_a_prefix_for_every_long_token() {
        // Every multi-character operator must be reachable by growing one character at a
        // time, or a longest-match lexer would need extra lookahead.
        let texts: HashSet<&str> = Punct::ALL.iter().map(|p| p.as_str()).collect();
        for t in &texts {
            if t.len() > 1 {
                let prefix = &t[..t.len() - 1];
                assert!(
                    texts.contains(prefix)
                        || matches!(*t, "..=" | "??=" | "<<=" | ">>=" | "~>" | "|>" | "->"),
                    "{t:?} has no shorter prefix token {prefix:?}"
                );
            }
        }
    }

    #[test]
    fn comments_are_trivia() {
        assert!(TokenKind::LineComment.is_trivia());
        assert!(TokenKind::DocBlockComment.is_trivia());
        assert!(!TokenKind::Ident.is_trivia());
        assert!(!TokenKind::Newline.is_trivia());
    }

    #[test]
    fn tokens_are_small() {
        // Tokens are created in huge numbers; keep them cheap.
        assert!(std::mem::size_of::<Token>() <= 16);
    }
}
