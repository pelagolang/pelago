//! The syntax tree.
//!
//! One node type per construct in the design docs. Nodes carry a [`Span`] and never copy source
//! text: an [`Ident`] is just a span, and `ident.text(src)` slices the source. Operators are kept as
//! the [`Punct`] they were written with, so the tree stays small and the printer prints them back
//! exactly.
//!
//! The tree is plain `Box` / `Vec` for now. An arena (and interned names) is a later speed-up that
//! changes this file and the parser only.
//!
//! The parser is forgiving: when input is wrong it records an error and puts an `Error` node in the
//! tree, so one mistake does not hide the rest.

use crate::span::Span;
use crate::token::Punct;

/// A name, as a span of the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ident {
    pub span: Span,
}

impl Ident {
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        src.get(self.span.start as usize..self.span.end as usize)
            .unwrap_or("")
    }
}

/// A dotted name such as `std.io.file`.
#[derive(Clone, Debug, PartialEq)]
pub struct Path {
    pub segments: Vec<Ident>,
    pub span: Span,
}

/// A whole source file. Statements are allowed at the top level so that sample files can be
/// scripts; a later pass restricts what a real module may contain.
#[derive(Clone, Debug, PartialEq)]
pub struct File {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

/// How a name is bound: the three access modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Let,
    Inout,
    Sink,
}

impl Mode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Mode::Let => "let",
            Mode::Inout => "inout",
            Mode::Sink => "sink",
        }
    }
}

// ---- Attributes and modifiers ------------------------------------------------

/// `@inline`, `@derive(eq, order)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Attr {
    pub name: Ident,
    pub args: Vec<Expr>,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Vis {
    /// Nothing written: private to the module.
    #[default]
    Default,
    Pub,
    /// `pub pkg`: visible inside the package, not outside.
    PubPkg,
    Priv,
    /// `export`, the alternative spelling of `pub`.
    Export,
}

/// The words that can come in front of a declaration.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub vis: Vis,
    pub is_open: bool,
    pub is_abstract: bool,
    pub is_final: bool,
    pub is_override: bool,
    pub is_meta: bool,
    pub is_async: bool,
    pub is_unsafe: bool,
    pub is_linear: bool,
    pub is_static: bool,
    /// `ext`: defined outside the language.
    pub is_ext: bool,
    /// The optional ABI string after `ext`: `ext "C" fn ...`.
    pub ext_abi: Option<Span>,
}

impl Modifiers {
    pub fn is_empty(&self) -> bool {
        *self == Modifiers::default()
    }
}

// ---- Types -------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Type {
    pub kind: TypeKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TypeKind {
    /// `int`, `std.file`, `map<string, int>`.
    Path {
        path: Path,
        args: Vec<GenericArg>,
    },
    SelfType,
    /// `[T]`
    List(Box<Type>),
    /// `[T; N]`
    Array(Box<Type>, Box<Expr>),
    /// `(A, B)`; `()` is the empty tuple.
    Tuple(Vec<Type>),
    /// `(A, B; N)`: an alternating fixed-size tuple.
    FixedTuple(Vec<Type>, Box<Expr>),
    /// `T?`
    Option(Box<Type>),
    /// `T ! E`
    Result(Box<Type>, Box<Type>),
    /// `fn(int) -> int`
    Fn {
        params: Vec<FnTypeParam>,
        ret: Option<Box<Type>>,
    },
    /// `any shape_like`
    Any(Box<Type>),
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FnTypeParam {
    pub mode: Option<Mode>,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GenericArg {
    Type(Type),
    /// A value, for `meta N: int` parameters: `soa<point, 8>`.
    Const(Expr),
}

// ---- Patterns ----------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Pattern {
    pub kind: PatternKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PatternKind {
    /// `_`
    Wildcard,
    /// A name that binds the value: `n`, `inout w`, `sink r`.
    Binding {
        mode: Option<Mode>,
        name: Ident,
    },
    /// `name is pattern`
    As {
        name: Ident,
        pattern: Box<Pattern>,
    },
    /// A literal: `3`, `-1`, `"start"`, `'a'`, `true`.
    Literal(Box<Expr>),
    /// `0..10`, `90..=100`, `10..`, `..0`, `..`
    Range {
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
        inclusive: bool,
    },
    Tuple(Vec<Pattern>),
    /// `[first, ...rest]`
    List(Vec<ListPatItem>),
    /// `.circle(r)`, `shape.rect(w, h)`, `ok(n)`, `some(v)`, `.empty`.
    Variant {
        leading_dot: bool,
        path: Path,
        args: Option<Vec<PatArg>>,
    },
    /// `none`
    None,
    /// The type after `is`: `pet is dog`.
    TypeTest(Path),
    /// `point { x: 0, y, .. }`
    Struct {
        path: Path,
        fields: Vec<FieldPat>,
        has_rest: bool,
    },
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PatArg {
    /// `.rect(w: width)`
    pub label: Option<Ident>,
    pub pattern: Pattern,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ListPatItem {
    Pattern(Pattern),
    /// `...` or `...rest` or `...inout rest`
    Rest {
        mode: Option<Mode>,
        name: Option<Ident>,
        span: Span,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldPat {
    pub mode: Option<Mode>,
    pub name: Ident,
    /// `None` binds the field under its own name.
    pub pattern: Option<Pattern>,
}

// ---- Expressions -------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallMark {
    /// `c.show()`
    None,
    /// `c.bump!()`: the call changes the receiver.
    Changes,
    /// `c.close!!()`: the call consumes the receiver.
    Consumes,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StrPart {
    /// Literal text (still containing escapes), as a span of the source.
    Text(Span),
    /// `#name` or `#(expr)`
    Interp(Box<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    pub mode: Option<Mode>,
    pub label: Option<Ident>,
    pub spread: bool,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListItem {
    pub spread: bool,
    pub value: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MapItem {
    Pair(Expr, Expr),
    Spread(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldInit {
    pub name: Ident,
    /// `None` is the shorthand `point { x, y }`.
    pub value: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StructInit {
    Field(FieldInit),
    /// `...base`
    Spread(Expr),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuilderKind {
    List,
    Set,
    Map,
    /// Brackets left out, as the argument of a call: a lazy stream.
    Stream,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Clause {
    /// `i, x of xs`
    Of { pats: Vec<Pattern>, source: Expr },
    /// `if cond`
    If(Expr),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Builder {
    pub kind: BuilderKind,
    pub clauses: Vec<Clause>,
    /// After `~>`. Missing means "keep the item itself".
    pub result: Option<Expr>,
    /// For maps: the part after `:` in `key : value`.
    pub value: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Capture {
    pub mode: Option<Mode>,
    pub name: Ident,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClosureParam {
    pub pattern: Pattern,
    pub ty: Option<Type>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Closure {
    pub captures: Vec<Capture>,
    pub params: Vec<ClosureParam>,
    /// `(a: int) -> int ~> a + 1`
    pub ret: Option<Type>,
    pub body: Expr,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SwitchArm {
    /// Empty for `default`.
    pub patterns: Vec<Pattern>,
    pub is_default: bool,
    pub guard: Option<Expr>,
    pub body: Block,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Switch {
    pub mode: Option<Mode>,
    pub scrutinee: Expr,
    pub arms: Vec<SwitchArm>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SelectArm {
    /// `case v = a.recv() { ... }`
    pub bind: Option<Ident>,
    /// `None` for `default`.
    pub op: Option<Expr>,
    pub body: Block,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IfExpr {
    pub cond: Expr,
    pub then: Block,
    /// A block, or another `if`.
    pub else_: Option<Expr>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ForHead {
    /// `for i, x of items`
    Of { pats: Vec<Pattern>, iter: Expr },
    /// `for k in map`, `for i in 0..10`
    In { pat: Pattern, iter: Expr },
    /// `for (var i = 0; i < n; i += 1)`
    C {
        init: Option<Box<Stmt>>,
        cond: Option<Expr>,
        step: Option<Expr>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ForExpr {
    pub head: ForHead,
    pub body: Block,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    Int,
    Float,
    Char,
    Bool(bool),
    /// `none`
    None,
    Str(Vec<StrPart>),
    Name(Ident),
    /// `self`
    SelfValue,
    /// `Self`
    SelfType,
    /// `_`
    Underscore,
    /// `.circle`: the type comes from context.
    DotName(Ident),
    Paren(Box<Expr>),
    Tuple(Vec<Expr>),
    List(Vec<ListItem>),
    /// `[0; 9]`
    Repeat {
        value: Box<Expr>,
        count: Box<Expr>,
    },
    /// `map { "a": 1 }`; `{:}` is an empty one.
    Map(Vec<MapItem>),
    Set(Vec<ListItem>),
    Builder(Box<Builder>),
    /// `point { x: 1.0, y: 2.0 }`
    StructLit {
        ty: Box<Type>,
        fields: Vec<StructInit>,
    },
    Field {
        base: Box<Expr>,
        name: Ident,
        /// `a?.b`
        optional: bool,
    },
    /// `t.0`
    TupleIndex {
        base: Box<Expr>,
        index: Span,
    },
    Index {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Arg>,
        mark: CallMark,
    },
    /// `max<int>`: explicit generic arguments.
    TypeApply {
        base: Box<Expr>,
        args: Vec<GenericArg>,
    },
    Unary {
        op: Punct,
        operand: Box<Expr>,
    },
    /// `copy xs`
    Copy(Box<Expr>),
    Binary {
        op: Punct,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `x in 1..10`
    In {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Assign {
        op: Punct,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `i++` / `i--`, statements only.
    IncDec {
        op: Punct,
        operand: Box<Expr>,
    },
    Range {
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
        inclusive: bool,
        step: Option<Box<Expr>>,
    },
    Ternary {
        cond: Box<Expr>,
        then: Box<Expr>,
        else_: Box<Expr>,
    },
    /// Postfix `?`
    Try(Box<Expr>),
    /// `value is pattern`
    Is {
        value: Box<Expr>,
        pattern: Box<Pattern>,
    },
    /// `x to float`; `checked` is `x to? dog`.
    Convert {
        value: Box<Expr>,
        ty: Box<Type>,
        checked: bool,
    },
    Closure(Box<Closure>),
    If(Box<IfExpr>),
    /// `unless cond { ... } else { ... }`
    Unless(Box<IfExpr>),
    Switch(Box<Switch>),
    Select(Vec<SelectArm>),
    /// `scope s { ... }`
    Scope {
        name: Ident,
        body: Block,
    },
    Block(Block),
    Unsafe(Block),
    Loop(Block),
    While {
        cond: Box<Expr>,
        body: Block,
    },
    DoWhile {
        body: Block,
        cond: Box<Expr>,
    },
    For(Box<ForExpr>),
    Guard {
        cond: Box<Expr>,
        else_: Block,
    },
    Return(Option<Box<Expr>>),
    /// `break`, `break value`, `break outer`, `break outer value`.
    Break {
        label: Option<Ident>,
        value: Option<Box<Expr>>,
    },
    /// `continue` or `continue outer`.
    Continue {
        label: Option<Ident>,
    },
    /// `outer: for x of xs { ... }`: a loop or block that `break` and `continue` can name.
    Labeled {
        label: Ident,
        body: Box<Expr>,
    },
    Become(Box<Expr>),
    Defer(Box<Expr>),
    Yield(Option<Box<Expr>>),
    /// `meta expr`: run now, while compiling.
    Meta(Box<Expr>),
    /// `soa [a, b]`: a struct-of-arrays collection literal.
    Soa(Vec<ListItem>),
    /// `meta if cond { ... } else { ... }`: include this code only when the condition holds.
    MetaIf(Box<IfExpr>),
    Error,
}

// ---- Statements --------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    /// The last expression, when it is not followed by `;`: the block's value.
    pub tail: Option<Box<Expr>>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StmtKind {
    Decl(Box<Decl>),
    Expr(Expr),
    Item(Box<Item>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclKind {
    Var,
    Const,
}

/// `const x = 1`, `var p: point = ...`, `const (q, r) = divmod(7, 2)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Decl {
    pub kind: DeclKind,
    pub pattern: Pattern,
    pub ty: Option<Type>,
    pub init: Option<Expr>,
}

// ---- Items -------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// The `///` and `///* ... *///` comments right before the declaration.
    pub docs: Vec<Span>,
    pub attrs: Vec<Attr>,
    pub mods: Modifiers,
    pub kind: ItemKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ItemKind {
    /// `mod name` or `mod name { ... }`
    Mod {
        path: Path,
        body: Option<Vec<Stmt>>,
    },
    Import(Import),
    Decl(Decl),
    Fn(Box<FnDecl>),
    Struct(Box<TypeDecl>),
    Obj(Box<TypeDecl>),
    Enum(Box<EnumDecl>),
    Interface(Box<TypeDecl>),
    Impl(Box<ImplDecl>),
    /// `type id = int64`
    TypeAlias {
        name: Ident,
        generics: Generics,
        ty: Type,
    },
    /// `test "name" { ... }`
    Test {
        name: Option<Expr>,
        body: Block,
    },
}

/// `import std.io`, `import fmt = std.io`, `import { read, write: put, ... } = std.io`.
#[derive(Clone, Debug, PartialEq)]
pub struct Import {
    /// `use` is another spelling of `import`.
    pub is_use: bool,
    pub kind: ImportKind,
    /// Where from. `levels` counts leading dots: 0 is a path from a package name (`std.io`),
    /// 1 is relative to this file's directory (`.sibling`), 2 goes up one (`..util`).
    pub levels: u8,
    pub path: Path,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ImportKind {
    /// `import std.io`: the module, under its last name.
    Module,
    /// `import fmt = std.io`: the module under another name.
    Alias(Ident),
    /// `import { read, write: put, ... } = std.io`
    Names {
        names: Vec<ImportName>,
        /// A `...` in the list: everything else the module makes public.
        glob: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImportName {
    pub name: Ident,
    /// `write: put` brings `write` in as `put`.
    pub alias: Option<Ident>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Generics {
    pub params: Vec<GenericParam>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GenericParam {
    /// `meta N: int`
    pub is_meta: bool,
    pub name: Ident,
    /// Interfaces the type must provide: `T: ordered + hash`. For a `meta` parameter this holds
    /// its type.
    pub bounds: Vec<Type>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WherePred {
    pub ty: Type,
    pub bounds: Vec<Type>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FnKind {
    Fn,
    Init,
    Deinit,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub mode: Option<Mode>,
    pub name: Ident,
    /// `self`
    pub is_self: bool,
    /// `...nums: int`
    pub variadic: bool,
    pub ty: Option<Type>,
    pub default: Option<Expr>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FnBody {
    Block(Block),
    /// `fn double(x: int) -> int = x * 2`
    Expr(Expr),
    /// An interface method or an `ext` function.
    None,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FnDecl {
    pub kind: FnKind,
    pub name: Ident,
    pub generics: Generics,
    pub params: Vec<Param>,
    pub ret: Option<Type>,
    pub where_: Vec<WherePred>,
    pub body: FnBody,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldDecl {
    pub docs: Vec<Span>,
    pub attrs: Vec<Attr>,
    pub vis: Vis,
    pub name: Ident,
    pub ty: Type,
    pub default: Option<Expr>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Member {
    Field(FieldDecl),
    /// Methods, `init` and `deinit`, associated functions.
    Item(Item),
}

/// `struct`, `obj` and `interface` declarations.
#[derive(Clone, Debug, PartialEq)]
pub struct TypeDecl {
    pub name: Ident,
    pub generics: Generics,
    /// After `:`: the base `obj` and/or interfaces.
    pub bases: Vec<Type>,
    pub where_: Vec<WherePred>,
    pub members: Vec<Member>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub docs: Vec<Span>,
    pub name: Ident,
    pub fields: Vec<VariantField>,
    pub discriminant: Option<Expr>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VariantField {
    pub label: Option<Ident>,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnumDecl {
    pub name: Ident,
    pub generics: Generics,
    pub bases: Vec<Type>,
    pub where_: Vec<WherePred>,
    pub variants: Vec<Variant>,
    /// Methods and other items inside the enum.
    pub items: Vec<Item>,
}

/// `impl iface for type { ... }` or `impl type { ... }`.
#[derive(Clone, Debug, PartialEq)]
pub struct ImplDecl {
    pub generics: Generics,
    pub target: Type,
    /// The type after `for`, when there is one. Then `target` is the interface.
    pub for_type: Option<Type>,
    pub where_: Vec<WherePred>,
    pub members: Vec<Member>,
}

/// The text of doc comments, with the `///` markers (or `///*` and `*///`) and one leading space
/// removed from each line, joined by newlines.
pub fn doc_text(src: &str, docs: &[Span]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for span in docs {
        let raw = src
            .get(span.start as usize..span.end as usize)
            .unwrap_or("");
        if let Some(body) = raw.strip_prefix("///*") {
            let body = body.strip_suffix("*///").unwrap_or(body);
            for line in body.trim().lines() {
                lines.push(line.trim().to_string());
            }
        } else {
            let body = raw.strip_prefix("///").unwrap_or(raw);
            lines.push(
                body.strip_prefix(' ')
                    .unwrap_or(body)
                    .trim_end()
                    .to_string(),
            );
        }
    }
    lines.join("\n")
}
