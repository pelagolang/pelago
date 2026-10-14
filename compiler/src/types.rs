//! Types, as the checker sees them.
//!
//! Types are interned: equal types have the same [`Ty`] id, so comparing is a number compare.
//! Inference variables live in the same table and are solved by unification, with a trail so a
//! failed attempt can be undone (used when trying several `impl`s).

use crate::ast::Mode;
use crate::resolve::DefId;
use crate::span::Span;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct Ty(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum IntKind {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
}

impl IntKind {
    pub const fn bits(self) -> u32 {
        match self {
            IntKind::I8 | IntKind::U8 => 8,
            IntKind::I16 | IntKind::U16 => 16,
            IntKind::I32 | IntKind::U32 => 32,
            IntKind::I64 | IntKind::U64 => 64,
        }
    }

    pub const fn signed(self) -> bool {
        matches!(
            self,
            IntKind::I8 | IntKind::I16 | IntKind::I32 | IntKind::I64
        )
    }

    pub const fn name(self) -> &'static str {
        match self {
            IntKind::I8 => "int8",
            IntKind::I16 => "int16",
            IntKind::I32 => "int32",
            IntKind::I64 => "int",
            IntKind::U8 => "uint8",
            IntKind::U16 => "uint16",
            IntKind::U32 => "uint32",
            IntKind::U64 => "uint64",
        }
    }

    /// Does the value `magnitude` (negated if `negative`) fit?
    pub fn fits(self, magnitude: u128, negative: bool) -> bool {
        let bits = self.bits();
        if self.signed() {
            let max = (1u128 << (bits - 1)) - 1;
            if negative {
                magnitude <= max + 1
            } else {
                magnitude <= max
            }
        } else {
            (!negative || magnitude == 0) && magnitude < (1u128 << bits)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FloatKind {
    F32,
    F64,
}

/// The length of a fixed array: known, a `meta` parameter, or not worked out.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ArrayLen {
    Known(u64),
    Param(DefId),
    Unknown,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ParamTy {
    pub mode: Option<Mode>,
    pub ty: Ty,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum TyKind {
    Int(IntKind),
    Float(FloatKind),
    Bool,
    Char,
    /// `string`: owned text.
    String,
    /// `str`: a view of text.
    Str,
    Void,
    /// The type of expressions that never finish (`return`, `panic(...)`).
    Never,
    /// The catch-all `error` type.
    Error,
    /// A struct, obj or enum, with its type arguments.
    Adt(DefId, Vec<Ty>),
    /// A generic parameter, inside the generic item.
    Param(DefId),
    List(Ty),
    Array(Ty, ArrayLen),
    Tuple(Vec<Ty>),
    /// `(A, B; N)`
    AltTuple(Vec<Ty>, ArrayLen),
    Option(Ty),
    Result(Ty, Ty),
    Map(Ty, Ty),
    Set(Ty),
    Range(Ty),
    Iter(Ty),
    Box(Ty),
    Fn(Vec<ParamTy>, Ty),
    /// `any iface`
    Any(DefId),
    /// An inference variable.
    Var(u32),
    /// Something that already failed or is not modelled yet: unifies with everything quietly.
    Unknown,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VarKind {
    General,
    /// An integer literal's type until it is known: any integer type, `int` by default.
    Int,
    /// A float literal's type until it is known.
    Float,
}

pub struct TypeTable {
    kinds: Vec<TyKind>,
    map: HashMap<TyKind, Ty>,
    bind: Vec<Option<Ty>>,
    var_kind: Vec<VarKind>,
    var_origin: Vec<Span>,
    trail: Vec<u32>,
    pub unknown: Ty,
    pub void: Ty,
    pub never: Ty,
    pub bool_: Ty,
    pub int: Ty,
    pub float: Ty,
    pub string: Ty,
    pub str_: Ty,
    pub char_: Ty,
    pub error: Ty,
}

impl Default for TypeTable {
    fn default() -> TypeTable {
        TypeTable::new()
    }
}

impl TypeTable {
    pub fn new() -> TypeTable {
        let mut t = TypeTable {
            kinds: Vec::new(),
            map: HashMap::new(),
            bind: Vec::new(),
            var_kind: Vec::new(),
            var_origin: Vec::new(),
            trail: Vec::new(),
            unknown: Ty(0),
            void: Ty(0),
            never: Ty(0),
            bool_: Ty(0),
            int: Ty(0),
            float: Ty(0),
            string: Ty(0),
            str_: Ty(0),
            char_: Ty(0),
            error: Ty(0),
        };
        t.unknown = t.mk(TyKind::Unknown);
        t.void = t.mk(TyKind::Void);
        t.never = t.mk(TyKind::Never);
        t.bool_ = t.mk(TyKind::Bool);
        t.int = t.mk(TyKind::Int(IntKind::I64));
        t.float = t.mk(TyKind::Float(FloatKind::F64));
        t.string = t.mk(TyKind::String);
        t.str_ = t.mk(TyKind::Str);
        t.char_ = t.mk(TyKind::Char);
        t.error = t.mk(TyKind::Error);
        t
    }

    pub fn mk(&mut self, k: TyKind) -> Ty {
        if let Some(&t) = self.map.get(&k) {
            return t;
        }
        let t = Ty(self.kinds.len() as u32);
        self.kinds.push(k.clone());
        self.map.insert(k, t);
        t
    }

    /// The kind of a type, without following inference variables.
    pub fn kind(&self, t: Ty) -> &TyKind {
        &self.kinds[t.0 as usize]
    }

    /// The type with solved variables followed one level.
    pub fn shallow(&self, mut t: Ty) -> Ty {
        while let TyKind::Var(v) = self.kinds[t.0 as usize] {
            match self.bind[v as usize] {
                Some(next) => t = next,
                None => break,
            }
        }
        t
    }

    pub fn kind_of(&self, t: Ty) -> &TyKind {
        self.kind(self.shallow(t))
    }

    pub fn fresh(&mut self, kind: VarKind, origin: Span) -> Ty {
        let v = self.bind.len() as u32;
        self.bind.push(None);
        self.var_kind.push(kind);
        self.var_origin.push(origin);
        self.mk(TyKind::Var(v))
    }

    pub fn var_info(&self, t: Ty) -> Option<(VarKind, Span)> {
        match self.kind_of(t) {
            TyKind::Var(v) => Some((self.var_kind[*v as usize], self.var_origin[*v as usize])),
            _ => None,
        }
    }

    pub fn is_unknown(&self, t: Ty) -> bool {
        matches!(self.kind_of(t), TyKind::Unknown)
    }

    pub fn int_kind(&self, t: Ty) -> Option<IntKind> {
        match self.kind_of(t) {
            TyKind::Int(k) => Some(*k),
            _ => None,
        }
    }

    // ---- Constructors ---------------------------------------------------------------------

    pub fn list(&mut self, e: Ty) -> Ty {
        self.mk(TyKind::List(e))
    }
    pub fn option(&mut self, e: Ty) -> Ty {
        self.mk(TyKind::Option(e))
    }
    pub fn result(&mut self, t: Ty, e: Ty) -> Ty {
        self.mk(TyKind::Result(t, e))
    }
    pub fn tuple(&mut self, ts: Vec<Ty>) -> Ty {
        self.mk(TyKind::Tuple(ts))
    }

    // ---- Unification ----------------------------------------------------------------------

    pub fn snapshot(&self) -> usize {
        self.trail.len()
    }

    pub fn rollback(&mut self, mark: usize) {
        while self.trail.len() > mark {
            if let Some(v) = self.trail.pop() {
                self.bind[v as usize] = None;
            }
        }
    }

    fn occurs(&self, v: u32, t: Ty) -> bool {
        let t = self.shallow(t);
        match self.kind(t) {
            TyKind::Var(w) => *w == v,
            TyKind::Adt(_, args) | TyKind::Tuple(args) | TyKind::AltTuple(args, _) => {
                args.iter().any(|a| self.occurs(v, *a))
            }
            TyKind::List(a)
            | TyKind::Array(a, _)
            | TyKind::Option(a)
            | TyKind::Set(a)
            | TyKind::Range(a)
            | TyKind::Iter(a)
            | TyKind::Box(a) => self.occurs(v, *a),
            TyKind::Result(a, b) | TyKind::Map(a, b) => self.occurs(v, *a) || self.occurs(v, *b),
            TyKind::Fn(ps, r) => ps.iter().any(|p| self.occurs(v, p.ty)) || self.occurs(v, *r),
            _ => false,
        }
    }

    fn bind_var(&mut self, v: u32, t: Ty) -> bool {
        // Literal variables only accept their own kind of type.
        let t = self.shallow(t);
        match (self.var_kind[v as usize], self.kind(t).clone()) {
            (VarKind::General, TyKind::Var(w)) if w == v => return true,
            (VarKind::General, _) => {
                if self.occurs(v, t) {
                    return false;
                }
            }
            (VarKind::Int, TyKind::Int(_)) | (VarKind::Float, TyKind::Float(_)) => {}
            (k, TyKind::Var(w)) => {
                if w == v {
                    return true;
                }
                // Two variables: the more specific kind survives.
                let other = self.var_kind[w as usize];
                match (k, other) {
                    (a, b) if a == b => {}
                    (_, VarKind::General) => {
                        // Bind the general one to us instead.
                        self.bind[w as usize] = Some(self.kinds_ty_of_var(v));
                        self.trail.push(w);
                        return true;
                    }
                    (VarKind::General, _) => {}
                    _ => return false,
                }
            }
            (_, TyKind::Unknown) => return true,
            _ => return false,
        }
        self.bind[v as usize] = Some(t);
        self.trail.push(v);
        true
    }

    fn kinds_ty_of_var(&mut self, v: u32) -> Ty {
        self.mk(TyKind::Var(v))
    }

    /// Makes `a` and `b` the same type if possible. On failure, partial progress is undone.
    pub fn unify(&mut self, a: Ty, b: Ty) -> bool {
        let mark = self.snapshot();
        if self.unify_inner(a, b) {
            true
        } else {
            self.rollback(mark);
            false
        }
    }

    fn unify_inner(&mut self, a: Ty, b: Ty) -> bool {
        let a = self.shallow(a);
        let b = self.shallow(b);
        if a == b {
            return true;
        }
        let ka = self.kind(a).clone();
        let kb = self.kind(b).clone();
        match (&ka, &kb) {
            (TyKind::Unknown, _) | (_, TyKind::Unknown) => true,
            (TyKind::Var(v), _) => self.bind_var(*v, b),
            (_, TyKind::Var(w)) => self.bind_var(*w, a),
            (TyKind::Adt(d1, a1), TyKind::Adt(d2, a2)) => d1 == d2 && self.unify_all(a1, a2),
            (TyKind::List(x), TyKind::List(y))
            | (TyKind::Option(x), TyKind::Option(y))
            | (TyKind::Set(x), TyKind::Set(y))
            | (TyKind::Range(x), TyKind::Range(y))
            | (TyKind::Iter(x), TyKind::Iter(y))
            | (TyKind::Box(x), TyKind::Box(y)) => self.unify_inner(*x, *y),
            (TyKind::Array(x, n), TyKind::Array(y, m)) => {
                (n == m || *n == ArrayLen::Unknown || *m == ArrayLen::Unknown)
                    && self.unify_inner(*x, *y)
            }
            (TyKind::Tuple(x), TyKind::Tuple(y)) => self.unify_all(x, y),
            (TyKind::AltTuple(x, n), TyKind::AltTuple(y, m)) => n == m && self.unify_all(x, y),
            (TyKind::Result(t1, e1), TyKind::Result(t2, e2))
            | (TyKind::Map(t1, e1), TyKind::Map(t2, e2)) => {
                self.unify_inner(*t1, *t2) && self.unify_inner(*e1, *e2)
            }
            (TyKind::Fn(p1, r1), TyKind::Fn(p2, r2)) => {
                p1.len() == p2.len()
                    && p1
                        .iter()
                        .zip(p2.iter())
                        .all(|(x, y)| x.mode == y.mode && self.unify_inner(x.ty, y.ty))
                    && self.unify_inner(*r1, *r2)
            }
            _ => false,
        }
    }

    fn unify_all(&mut self, a: &[Ty], b: &[Ty]) -> bool {
        a.len() == b.len()
            && a.iter()
                .zip(b.iter())
                .all(|(x, y)| self.unify_inner(*x, *y))
    }

    // ---- Walking ------------------------------------------------------------------------------

    /// Replaces every solved variable by its type, all the way down.
    pub fn zonk(&mut self, t: Ty) -> Ty {
        let t = self.shallow(t);
        let k = self.kind(t).clone();
        let new = match k {
            TyKind::Adt(d, args) => {
                TyKind::Adt(d, args.into_iter().map(|a| self.zonk(a)).collect())
            }
            TyKind::Tuple(ts) => TyKind::Tuple(ts.into_iter().map(|a| self.zonk(a)).collect()),
            TyKind::AltTuple(ts, n) => {
                TyKind::AltTuple(ts.into_iter().map(|a| self.zonk(a)).collect(), n)
            }
            TyKind::List(a) => TyKind::List(self.zonk(a)),
            TyKind::Array(a, n) => TyKind::Array(self.zonk(a), n),
            TyKind::Option(a) => TyKind::Option(self.zonk(a)),
            TyKind::Set(a) => TyKind::Set(self.zonk(a)),
            TyKind::Range(a) => TyKind::Range(self.zonk(a)),
            TyKind::Iter(a) => TyKind::Iter(self.zonk(a)),
            TyKind::Box(a) => TyKind::Box(self.zonk(a)),
            TyKind::Result(a, b) => TyKind::Result(self.zonk(a), self.zonk(b)),
            TyKind::Map(a, b) => TyKind::Map(self.zonk(a), self.zonk(b)),
            TyKind::Fn(ps, r) => TyKind::Fn(
                ps.into_iter()
                    .map(|p| ParamTy {
                        mode: p.mode,
                        ty: self.zonk(p.ty),
                    })
                    .collect(),
                self.zonk(r),
            ),
            _ => return t,
        };
        self.mk(new)
    }

    /// Replaces generic parameters using `map`.
    pub fn subst(&mut self, t: Ty, map: &HashMap<DefId, Ty>) -> Ty {
        if map.is_empty() {
            return t;
        }
        let t = self.shallow(t);
        let k = self.kind(t).clone();
        let new = match k {
            TyKind::Param(d) => return map.get(&d).copied().unwrap_or(t),
            TyKind::Adt(d, args) => {
                TyKind::Adt(d, args.into_iter().map(|a| self.subst(a, map)).collect())
            }
            TyKind::Tuple(ts) => {
                TyKind::Tuple(ts.into_iter().map(|a| self.subst(a, map)).collect())
            }
            TyKind::AltTuple(ts, n) => {
                TyKind::AltTuple(ts.into_iter().map(|a| self.subst(a, map)).collect(), n)
            }
            TyKind::List(a) => TyKind::List(self.subst(a, map)),
            TyKind::Array(a, n) => TyKind::Array(self.subst(a, map), n),
            TyKind::Option(a) => TyKind::Option(self.subst(a, map)),
            TyKind::Set(a) => TyKind::Set(self.subst(a, map)),
            TyKind::Range(a) => TyKind::Range(self.subst(a, map)),
            TyKind::Iter(a) => TyKind::Iter(self.subst(a, map)),
            TyKind::Box(a) => TyKind::Box(self.subst(a, map)),
            TyKind::Result(a, b) => TyKind::Result(self.subst(a, map), self.subst(b, map)),
            TyKind::Map(a, b) => TyKind::Map(self.subst(a, map), self.subst(b, map)),
            TyKind::Fn(ps, r) => TyKind::Fn(
                ps.into_iter()
                    .map(|p| ParamTy {
                        mode: p.mode,
                        ty: self.subst(p.ty, map),
                    })
                    .collect(),
                self.subst(r, map),
            ),
            _ => return t,
        };
        self.mk(new)
    }

    /// Does the type still contain an unsolved variable?
    pub fn has_vars(&self, t: Ty) -> bool {
        let t = self.shallow(t);
        match self.kind(t) {
            TyKind::Var(_) => true,
            TyKind::Adt(_, args) | TyKind::Tuple(args) | TyKind::AltTuple(args, _) => {
                args.iter().any(|a| self.has_vars(*a))
            }
            TyKind::List(a)
            | TyKind::Array(a, _)
            | TyKind::Option(a)
            | TyKind::Set(a)
            | TyKind::Range(a)
            | TyKind::Iter(a)
            | TyKind::Box(a) => self.has_vars(*a),
            TyKind::Result(a, b) | TyKind::Map(a, b) => self.has_vars(*a) || self.has_vars(*b),
            TyKind::Fn(ps, r) => ps.iter().any(|p| self.has_vars(p.ty)) || self.has_vars(*r),
            _ => false,
        }
    }

    /// Binds every unsolved literal variable to its default (`int`, `float`).
    pub fn default_literals(&mut self) {
        for v in 0..self.bind.len() {
            if self.bind[v].is_none() {
                let default = match self.var_kind[v] {
                    VarKind::Int => Some(self.int),
                    VarKind::Float => Some(self.float),
                    VarKind::General => None,
                };
                if let Some(d) = default {
                    self.bind[v] = Some(d);
                }
            }
        }
    }

    /// How many variables exist so far.
    pub fn var_count(&self) -> u32 {
        self.bind.len() as u32
    }

    /// Unsolved general variables created at or after `from`, with where they were created.
    pub fn unsolved(&self, from: u32) -> Vec<(u32, Span)> {
        (from as usize..self.bind.len())
            .filter(|&v| self.bind[v].is_none() && self.var_kind[v] == VarKind::General)
            .map(|v| (v as u32, self.var_origin[v]))
            .collect()
    }

    /// Prints a type the way it is written in source.
    pub fn display(&self, t: Ty, name_of: &dyn Fn(DefId) -> String) -> String {
        let t = self.shallow(t);
        let d = |x: Ty| self.display(x, name_of);
        match self.kind(t) {
            TyKind::Int(k) => k.name().to_string(),
            TyKind::Float(FloatKind::F32) => "float32".to_string(),
            TyKind::Float(FloatKind::F64) => "float".to_string(),
            TyKind::Bool => "bool".to_string(),
            TyKind::Char => "char".to_string(),
            TyKind::String => "string".to_string(),
            TyKind::Str => "str".to_string(),
            TyKind::Void => "void".to_string(),
            TyKind::Never => "never".to_string(),
            TyKind::Error => "error".to_string(),
            TyKind::Unknown => "_".to_string(),
            TyKind::Adt(def, args) => {
                if args.is_empty() {
                    name_of(*def)
                } else {
                    format!(
                        "{}<{}>",
                        name_of(*def),
                        args.iter().map(|a| d(*a)).collect::<Vec<_>>().join(", ")
                    )
                }
            }
            TyKind::Param(def) => name_of(*def),
            TyKind::List(e) => format!("[{}]", d(*e)),
            TyKind::Array(e, n) => match n {
                ArrayLen::Known(n) => format!("[{}; {n}]", d(*e)),
                ArrayLen::Param(p) => format!("[{}; {}]", d(*e), name_of(*p)),
                ArrayLen::Unknown => format!("[{}; _]", d(*e)),
            },
            TyKind::Tuple(ts) => format!(
                "({})",
                ts.iter().map(|a| d(*a)).collect::<Vec<_>>().join(", ")
            ),
            TyKind::AltTuple(ts, n) => {
                let len = match n {
                    ArrayLen::Known(n) => n.to_string(),
                    ArrayLen::Param(p) => name_of(*p),
                    ArrayLen::Unknown => "_".to_string(),
                };
                format!(
                    "({}; {len})",
                    ts.iter().map(|a| d(*a)).collect::<Vec<_>>().join(", ")
                )
            }
            TyKind::Option(e) => {
                let inner = d(*e);
                if matches!(self.kind_of(*e), TyKind::Fn(..) | TyKind::Result(..)) {
                    format!("({inner})?")
                } else {
                    format!("{inner}?")
                }
            }
            TyKind::Result(a, b) => format!("{} ! {}", d(*a), d(*b)),
            TyKind::Map(k, v) => format!("map<{}, {}>", d(*k), d(*v)),
            TyKind::Set(e) => format!("set<{}>", d(*e)),
            TyKind::Range(e) => format!("range<{}>", d(*e)),
            TyKind::Iter(e) => format!("iter<{}>", d(*e)),
            TyKind::Box(e) => format!("box<{}>", d(*e)),
            TyKind::Fn(ps, r) => {
                let params: Vec<String> = ps
                    .iter()
                    .map(|p| match p.mode {
                        Some(m) => format!("{} {}", m.as_str(), d(p.ty)),
                        None => d(p.ty),
                    })
                    .collect();
                if self.kind_of(*r) == &TyKind::Void {
                    format!("fn({})", params.join(", "))
                } else {
                    format!("fn({}) -> {}", params.join(", "), d(*r))
                }
            }
            TyKind::Any(def) => format!("any {}", name_of(*def)),
            TyKind::Var(v) => match self.var_kind[*v as usize] {
                VarKind::Int => "{integer}".to_string(),
                VarKind::Float => "{float}".to_string(),
                VarKind::General => "_".to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_gives_equal_ids() {
        let mut t = TypeTable::new();
        let a = t.list(t.int);
        let b = t.list(t.int);
        assert_eq!(a, b);
        let c = t.list(t.string);
        assert_ne!(a, c);
    }

    #[test]
    fn variables_unify_and_roll_back() {
        let mut t = TypeTable::new();
        let v = t.fresh(VarKind::General, Span::new(0, 0));
        let l = t.list(v);
        let target = t.list(t.string);
        let mark = t.snapshot();
        assert!(t.unify(l, target));
        assert_eq!(t.shallow(v), t.string);
        t.rollback(mark);
        assert!(matches!(t.kind_of(v), TyKind::Var(_)));
    }

    #[test]
    fn a_failed_unify_leaves_nothing_behind() {
        let mut t = TypeTable::new();
        let v = t.fresh(VarKind::General, Span::new(0, 0));
        let a = t.tuple(vec![v, t.int]);
        let b = t.tuple(vec![t.string, t.string]);
        assert!(!t.unify(a, b));
        assert!(matches!(t.kind_of(v), TyKind::Var(_)));
    }

    #[test]
    fn literal_variables_only_take_their_kind() {
        let mut t = TypeTable::new();
        let i = t.fresh(VarKind::Int, Span::new(0, 0));
        assert!(!t.unify(i, t.string));
        assert!(!t.unify(i, t.float));
        let u8t = t.mk(TyKind::Int(IntKind::U8));
        assert!(t.unify(i, u8t));
        assert_eq!(t.shallow(i), u8t);
        let f = t.fresh(VarKind::Float, Span::new(0, 0));
        assert!(!t.unify(f, t.int));
        assert!(t.unify(f, t.float));
    }

    #[test]
    fn two_literal_variables_merge_and_default() {
        let mut t = TypeTable::new();
        let a = t.fresh(VarKind::Int, Span::new(0, 0));
        let b = t.fresh(VarKind::General, Span::new(0, 0));
        assert!(t.unify(b, a));
        t.default_literals();
        assert_eq!(t.zonk(b), t.int);
        assert_eq!(t.zonk(a), t.int);
    }

    #[test]
    fn integer_ranges() {
        assert!(IntKind::U8.fits(255, false));
        assert!(!IntKind::U8.fits(256, false));
        assert!(!IntKind::U8.fits(1, true));
        assert!(IntKind::I8.fits(128, true));
        assert!(!IntKind::I8.fits(129, true));
        assert!(!IntKind::I8.fits(128, false));
        assert!(IntKind::I64.fits(9_223_372_036_854_775_807, false));
        assert!(IntKind::U64.fits(u64::MAX as u128, false));
    }

    #[test]
    fn substitution_replaces_parameters() {
        let mut t = TypeTable::new();
        let p = t.mk(TyKind::Param(DefId(1)));
        let l = t.list(p);
        let mut m = HashMap::new();
        m.insert(DefId(1), t.int);
        let s = t.subst(l, &m);
        assert_eq!(s, t.list(t.int));
    }

    #[test]
    fn display_looks_like_source() {
        let mut t = TypeTable::new();
        let o = t.option(t.int);
        let r = t.result(t.int, t.error);
        let m = t.mk(TyKind::Map(t.string, o));
        let names = |_d: DefId| "x".to_string();
        assert_eq!(t.display(o, &names), "int?");
        assert_eq!(t.display(r, &names), "int ! error");
        assert_eq!(t.display(m, &names), "map<string, int?>");
    }
}
