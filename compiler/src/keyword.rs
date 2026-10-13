//! The keyword table.
//!
//! One list, declared once, drives everything: the enum, the text of each
//! keyword, and the lookup. The lookup is a `match` on the string, which the
//! compiler turns into efficient code, so there is no hash map to build at start-up.
//!
//! Source of truth for the design is `docs/design/keywords.md`. Keep them in step.
//!
//! # Hard and soft keywords
//!
//! * A **hard** keyword is reserved everywhere. It can never be a variable name.
//! * A **soft** keyword is only special in one position and is an ordinary
//!   identifier everywhere else. Words such as `from`, `to`, `step`, `init`,
//!   `type`, `ext` and `union` are common names for variables and methods
//!   (a set has a `union` method; a function has `from` and `to` parameters),
//!   so reserving them everywhere would be a nuisance.
//!
//! The lexer produces [`Keyword`] tokens only for hard keywords. For a soft
//! keyword it produces an identifier and the parser checks
//! [`Keyword::from_ident`] where the word would be meaningful.
//!
//! # Not in this table
//!
//! * Operators written as words (`and`, `or`, `not`): the language uses symbols.
//! * Attribute names (`@inline`, `@derive`, `@packed`): they follow an `@`.
//! * Built-in type names (`int`, `string`, ...): see [`PRELUDE_TYPES`].
//! * Built-in functions (`print`, `assert`, ...): see [`PRELUDE_FUNCTIONS`].

/// Whether a keyword is reserved everywhere or only in one position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Strength {
    Hard,
    Soft,
}

macro_rules! keywords {
    (
        $(
            $(#[$doc:meta])*
            $variant:ident => $text:literal, $strength:ident;
        )*
    ) => {
        /// Every word that has a special meaning somewhere in the language.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Keyword {
            $( $(#[$doc])* $variant, )*
        }

        impl Keyword {
            /// All keywords, in declaration order.
            pub const ALL: &'static [Keyword] = &[ $( Keyword::$variant, )* ];

            /// The source text of the keyword.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $( Keyword::$variant => $text, )*
                }
            }

            /// Whether the keyword is reserved everywhere or only in one position.
            pub const fn strength(self) -> Strength {
                match self {
                    $( Keyword::$variant => Strength::$strength, )*
                }
            }

            /// Looks a word up. Returns `None` for ordinary identifiers.
            pub fn from_ident(word: &str) -> Option<Keyword> {
                match word {
                    $( $text => Some(Keyword::$variant), )*
                    _ => None,
                }
            }
        }
    };
}

keywords! {
    // ---- Declarations ----------------------------------------------------
    /// A variable that can be changed.
    Var => "var", Hard;
    /// A variable that cannot be changed, all the way down.
    Const => "const", Hard;
    /// Declares a function.
    Fn => "fn", Hard;
    /// A bundle of named fields; a value.
    Struct => "struct", Hard;
    /// A type that is exactly one of several named cases.
    Enum => "enum", Hard;
    /// A list of functions a type promises to provide.
    Interface => "interface", Hard;
    /// States that a type provides an interface.
    Impl => "impl", Hard;
    /// A heap object with inheritance (`obj dog : animal`).
    Obj => "obj", Hard;
    /// Gives a new name to an existing type.
    Type => "type", Soft;
    /// A module, usually one file.
    Mod => "mod", Hard;
    /// Reserved: a bundle of modules that is built and shared together.
    Pkg => "pkg", Hard;
    Import => "import", Hard;
    /// Alternative spelling of `pub` for re-exports.
    Export => "export", Hard;
    /// Alternative spelling of `import`.
    Use => "use", Hard;
    /// Visible outside its module.
    Pub => "pub", Hard;
    /// Private to its module. The default, so writing it is optional.
    Priv => "priv", Hard;
    /// Declares something defined outside the language (C interop).
    Ext => "ext", Soft;
    /// Compile-time code: `meta expr`, `meta fn`, `meta N: int`, `meta if`.
    Meta => "meta", Hard;
    /// Reserved.
    Macro => "macro", Hard;
    /// One memory slot that holds one of several layouts.
    Union => "union", Soft;
    /// A value that lives for the whole program, or belongs to a type.
    Static => "static", Soft;
    /// Defines what an operator means for a type.
    Operator => "operator", Soft;
    /// Runs when a value is first created.
    Init => "init", Soft;
    /// Runs when a value goes away.
    Deinit => "deinit", Hard;
    /// Puts a value on the heap and gives back an owned box.
    New => "new", Soft;
    /// Frees a boxed value now.
    Delete => "delete", Soft;

    // ---- Inheritance modifiers ------------------------------------------
    /// This `obj` may be extended, or this method overridden.
    Open => "open", Soft;
    Abstract => "abstract", Soft;
    Override => "override", Soft;
    Final => "final", Soft;
    Super => "super", Hard;

    // ---- Access modes ----------------------------------------------------
    /// Read-only access. The silent default; writing it is optional.
    Let => "let", Hard;
    /// Exclusive, changeable access for the duration of a call.
    Inout => "inout", Hard;
    /// Takes the value away from the caller.
    Sink => "sink", Hard;
    /// Makes an independent copy of an owning value.
    Copy => "copy", Hard;
    /// "Any type that provides this interface" (dynamic dispatch).
    Any => "any", Soft;

    // ---- Conditionals and loops ------------------------------------------
    If => "if", Hard;
    Else => "else", Hard;
    /// Runs the block when the condition is false.
    Unless => "unless", Hard;
    Switch => "switch", Hard;
    Case => "case", Hard;
    /// Only special inside `switch`.
    Default => "default", Soft;
    /// "Leave early unless this is true."
    Guard => "guard", Hard;
    For => "for", Hard;
    /// Keys, indexes and ranges: `for i in 0..10`.
    In => "in", Hard;
    /// Values: `for x of items`.
    Of => "of", Hard;
    While => "while", Hard;
    Do => "do", Hard;
    Loop => "loop", Hard;
    Break => "break", Hard;
    Continue => "continue", Hard;
    Return => "return", Hard;
    Defer => "defer", Hard;
    /// A guaranteed tail call.
    Become => "become", Hard;
    Yield => "yield", Hard;
    /// Only special after a range: `0..10 step 2`.
    Step => "step", Soft;
    /// Tests an enum case or a type.
    Is => "is", Hard;
    /// Conversion, only special after an expression: `x to float`.
    To => "to", Soft;
    /// Only special after `import` or `use`.
    From => "from", Soft;
    /// The implicit closure parameter, only special inside a closure.
    It => "it", Soft;
    /// Only special after a function or type signature.
    Where => "where", Soft;

    // ---- Values and names ------------------------------------------------
    True => "true", Hard;
    False => "false", Hard;
    /// The absence of a value in an `option<T>`. There is no null.
    None => "none", Hard;
    /// The value a method is working on.
    SelfValue => "self", Hard;
    /// The type of `self`.
    SelfType => "Self", Hard;

    // ---- Safety and the edges --------------------------------------------
    Unsafe => "unsafe", Hard;
    /// Marks a type whose values must be consumed exactly once. Reserved.
    Linear => "linear", Soft;
    Async => "async", Hard;
    Await => "await", Hard;
    /// A test block that only compiles in test mode.
    Test => "test", Soft;
    Asm => "asm", Soft;
    Volatile => "volatile", Soft;
    Atomic => "atomic", Soft;

    // ---- Concurrency (reserved) ------------------------------------------
    Spawn => "spawn", Soft;
    Chan => "chan", Soft;
    Select => "select", Soft;
}

impl Keyword {
    /// True for keywords the lexer turns into tokens by itself.
    pub const fn is_hard(self) -> bool {
        matches!(self.strength(), Strength::Hard)
    }
}

/// Names of the built-in types. Not keywords, but reserved by the standard library.
pub const PRELUDE_TYPES: &[&str] = &[
    "int", "int8", "int16", "int32", "int64", "uint", "uint8", "uint16", "uint32", "uint64",
    "byte", "float", "float32", "float64", "bool", "char", "string", "str", "map", "set", "range",
    "option", "result", "box", "soa", "rc", "ptr", "void", "never", "error",
];

/// Names of the built-in functions. Not keywords.
pub const PRELUDE_FUNCTIONS: &[&str] = &["print", "assert", "panic", "todo", "unreachable"];

/// Words from other languages that are **not** keywords here. The lexer treats them as
/// ordinary identifiers, and the compiler can use this to turn a confusing error into a
/// helpful one ("`class` is not a keyword here; use `obj`").
pub fn foreign_keyword_hint(word: &str) -> Option<&'static str> {
    Some(match word {
        "class" => "there is no `class`; use `struct` for plain values or `obj` for heap objects with inheritance",
        "extends" | "implements" | "inherits" => "inheritance is written with a colon: `obj dog : animal`",
        "null" | "nil" | "undefined" => "there is no null; use `none` with `option<T>`",
        "try" | "catch" | "throw" | "throws" | "finally" => "there are no exceptions; return `result<T, E>` and pass failures up with `?`",
        "function" | "func" | "def" | "fun" => "functions are declared with `fn`",
        "lambda" => "closures are written `x ~> x * 2`",
        "this" => "use `self`",
        "goto" => "there is no `goto`; use loops with `break` and `continue`",
        "elif" | "elseif" => "write `else if`",
        "and" | "or" | "not" | "xor" => "operators are symbols: `&&`, `||`, `!`, `^`",
        "foreach" => "write `for x of items`",
        "module" | "namespace" | "package" => "modules are declared with `mod`",
        "extern" => "use `ext`",
        "comptime" | "constexpr" => "compile-time code uses `meta`",
        "public" => "use `pub`",
        "private" => "use `priv`",
        "protocol" | "trait" => "interfaces are declared with `interface`",
        "mut" | "mutable" => "use `var` for a variable that can change",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn texts_are_unique() {
        let mut seen = HashSet::new();
        for kw in Keyword::ALL {
            assert!(
                seen.insert(kw.as_str()),
                "duplicate keyword text {:?}",
                kw.as_str()
            );
        }
    }

    #[test]
    fn texts_look_like_words() {
        for kw in Keyword::ALL {
            let s = kw.as_str();
            assert!(!s.is_empty());
            assert!(
                s.chars().all(|c| c.is_ascii_alphabetic()),
                "{s:?} is not a plain word"
            );
        }
    }

    #[test]
    fn lookup_round_trips() {
        for &kw in Keyword::ALL {
            assert_eq!(Keyword::from_ident(kw.as_str()), Some(kw));
        }
    }

    #[test]
    fn ordinary_identifiers_are_not_keywords() {
        for word in ["foo", "Var", "SELF", "selfish", "", "x1", "Fn"] {
            assert_eq!(Keyword::from_ident(word), None, "{word:?}");
        }
    }

    #[test]
    fn keywords_do_not_collide_with_prelude_names() {
        for name in PRELUDE_TYPES.iter().chain(PRELUDE_FUNCTIONS) {
            assert_eq!(
                Keyword::from_ident(name),
                None,
                "{name:?} is both a keyword and a prelude name"
            );
        }
    }

    #[test]
    fn foreign_words_are_not_keywords() {
        for word in ["class", "null", "try", "this", "function", "extends", "and"] {
            assert_eq!(Keyword::from_ident(word), None, "{word:?}");
            assert!(foreign_keyword_hint(word).is_some(), "{word:?} has no hint");
        }
    }

    #[test]
    fn real_keywords_have_no_foreign_hint() {
        for kw in Keyword::ALL {
            assert!(
                foreign_keyword_hint(kw.as_str()).is_none(),
                "{:?} is a keyword",
                kw.as_str()
            );
        }
    }

    #[test]
    fn decided_words_exist() {
        // Spot checks against docs/design/keywords.md.
        for word in [
            "var", "const", "fn", "obj", "meta", "inout", "sink", "become", "unless", "of",
        ] {
            assert!(
                Keyword::from_ident(word).is_some(),
                "{word:?} should be a keyword"
            );
        }
        // Dropped words must not creep back in.
        for word in ["class", "when", "match", "extern", "comptime", "inline"] {
            assert!(Keyword::from_ident(word).is_none(), "{word:?} was dropped");
        }
    }

    #[test]
    fn common_names_stay_soft() {
        for word in [
            "from", "to", "step", "it", "type", "union", "init", "default", "test", "ext",
        ] {
            let kw = Keyword::from_ident(word).unwrap();
            assert!(
                !kw.is_hard(),
                "{word:?} is too common a name to reserve everywhere"
            );
        }
    }
}
