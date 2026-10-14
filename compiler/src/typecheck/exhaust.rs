//! Exhaustiveness: does a `switch` handle every value, and does a pattern in `const`, `var` or `for`
//! always match?
//!
//! Patterns are turned into a small normal form and checked with the classic matrix method:
//! pick the first column, try each constructor of its type (a case of an enum, `some` / `none`,
//! `true` / `false`, the one constructor of a tuple or struct, each list length up to one past the
//! longest pattern), keep the rows that can match it, and recurse on what is left. Numbers, text
//! and characters have too many values to list, so they are only covered by a name or `_`.
//! An arm with a guard may fail, so it does not count.

use super::*;

/// What a variant-like pattern resolved to, recorded while the pattern was typed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum PatCtor {
    Variant(DefId),
    Some,
    Ok,
    Err,
}

pub(crate) enum MatchKind {
    Switch,
    Binding,
}

pub(crate) struct PendingMatch<'p> {
    pub file: ModuleId,
    pub span: Span,
    pub ty: Ty,
    pub kind: MatchKind,
    /// The patterns that count (arms with a guard are left out).
    pub pats: Vec<&'p Pattern>,
    /// An arm without a pattern (`default`).
    pub has_default: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum Ctor {
    Variant(DefId),
    Bool(bool),
    Some,
    None,
    Ok,
    Err,
    /// The one constructor of a tuple, a struct or `void`.
    Single,
}

#[derive(Clone, Debug)]
enum Pat {
    Wild,
    /// Matches some values and not others, in a way the checker does not reason about
    /// (a literal, a range, `is dog`).
    Opaque,
    Ctor(Ctor, Vec<Pat>),
    List {
        prefix: Vec<Pat>,
        suffix: Vec<Pat>,
        rest: bool,
    },
}

/// The constructors of a type.
enum Sig {
    Ctors(Vec<(Ctor, String, Vec<Ty>)>),
    List(Ty),
    /// Too many values to list.
    Infinite,
}

impl<'p> Checker<'p> {
    pub(crate) fn record_ctor(&mut self, p: &Pattern, c: PatCtor) {
        self.pat_ctors
            .insert((self.file, p.span.start, p.span.end), c);
    }

    /// A pattern that must always match (`const (a, b) = pair`, `for (k, v) of m`).
    pub(crate) fn check_irrefutable(&mut self, p: &'p Pattern, ty: Ty) {
        self.check_pattern(p, ty);
        self.pending_matches.push(PendingMatch {
            file: self.file,
            span: p.span,
            ty,
            kind: MatchKind::Binding,
            pats: vec![p],
            has_default: false,
        });
    }

    pub(crate) fn flush_matches(&mut self) {
        let pending = std::mem::take(&mut self.pending_matches);
        for m in pending {
            let ty = self.tt.zonk(m.ty);
            if matches!(
                self.tt.kind(self.tt.shallow(ty)),
                TyKind::Unknown | TyKind::Var(_) | TyKind::Error
            ) {
                continue;
            }
            let saved = self.file;
            self.file = m.file;
            let mut rows: Vec<Vec<Pat>> = Vec::new();
            if m.has_default {
                rows.push(vec![Pat::Wild]);
            }
            for p in &m.pats {
                let lowered = self.lower_pat(p, ty);
                rows.push(vec![lowered]);
            }
            if let Some(w) = self.missing(&rows, &[ty], 0) {
                let w = w.first().cloned().unwrap_or_else(|| "_".to_string());
                let msg = match m.kind {
                    MatchKind::Switch => format!(
                        "this `switch` does not handle `{w}`; add a case for it or a `default`"
                    ),
                    MatchKind::Binding => format!(
                        "this pattern does not always match (it misses `{w}`); use `switch`, `if ... is` or `guard`"
                    ),
                };
                self.err(m.span, msg);
            }
            self.file = saved;
        }
    }

    // ---- Normal form --------------------------------------------------------------------------------

    fn lower_pat(&mut self, p: &'p Pattern, ty: Ty) -> Pat {
        let ty = self.tt.shallow(ty);
        match &p.kind {
            PatternKind::Wildcard | PatternKind::Binding { .. } | PatternKind::Error => Pat::Wild,
            PatternKind::As { pattern, .. } => self.lower_pat(pattern, ty),
            PatternKind::Literal(e) => match e.kind {
                ExprKind::Bool(b) => Pat::Ctor(Ctor::Bool(b), Vec::new()),
                _ => Pat::Opaque,
            },
            PatternKind::Range { .. } | PatternKind::TypeTest(_) => Pat::Opaque,
            PatternKind::None => Pat::Ctor(Ctor::None, Vec::new()),
            PatternKind::Tuple(ps) => {
                if ps.is_empty() {
                    return Pat::Wild;
                }
                match self.kind_of(ty) {
                    TyKind::Tuple(ts) if ts.len() == ps.len() => {
                        let subs = ps
                            .iter()
                            .zip(ts.iter())
                            .map(|(s, t)| self.lower_pat(s, *t))
                            .collect();
                        Pat::Ctor(Ctor::Single, subs)
                    }
                    _ => Pat::Opaque,
                }
            }
            PatternKind::List(items) => {
                let elem = match self.kind_of(ty) {
                    TyKind::List(e) | TyKind::Array(e, _) => e,
                    _ => return Pat::Opaque,
                };
                let mut prefix = Vec::new();
                let mut suffix = Vec::new();
                let mut rest = false;
                for it in items {
                    match it {
                        ListPatItem::Pattern(sub) => {
                            let l = self.lower_pat(sub, elem);
                            if rest {
                                suffix.push(l);
                            } else {
                                prefix.push(l);
                            }
                        }
                        ListPatItem::Rest { .. } => rest = true,
                    }
                }
                Pat::List {
                    prefix,
                    suffix,
                    rest,
                }
            }
            PatternKind::Variant { args, .. } => {
                let Some(c) = self
                    .pat_ctors
                    .get(&(self.file, p.span.start, p.span.end))
                    .copied()
                else {
                    return Pat::Opaque;
                };
                match c {
                    PatCtor::Some | PatCtor::Ok | PatCtor::Err => {
                        let payload = match (c, self.kind_of(ty)) {
                            (PatCtor::Some, TyKind::Option(i)) => i,
                            (PatCtor::Ok, TyKind::Result(a, _)) => a,
                            (PatCtor::Err, TyKind::Result(_, b)) => b,
                            _ => return Pat::Opaque,
                        };
                        let sub = match args.as_deref() {
                            Some([one]) => self.lower_pat(&one.pattern, payload),
                            _ => Pat::Wild,
                        };
                        let ctor = match c {
                            PatCtor::Some => Ctor::Some,
                            PatCtor::Ok => Ctor::Ok,
                            _ => Ctor::Err,
                        };
                        Pat::Ctor(ctor, vec![sub])
                    }
                    PatCtor::Variant(v) => {
                        let TyKind::Adt(d, targs) = self.kind_of(ty) else {
                            return Pat::Opaque;
                        };
                        let Some(owner) = self.def(v).parent else {
                            return Pat::Opaque;
                        };
                        if owner != d {
                            return Pat::Opaque;
                        }
                        let map = self.adt_map_of(d, &targs);
                        let fields = self.variant_fields(v);
                        let mut subs = vec![Pat::Wild; fields.len()];
                        if let Some(args) = args {
                            if args.len() == fields.len() {
                                for (i, a) in args.iter().enumerate() {
                                    let idx = a
                                        .label
                                        .as_ref()
                                        .and_then(|l| {
                                            let ln = self.text(l);
                                            fields
                                                .iter()
                                                .position(|(fl, _)| fl.as_deref() == Some(ln))
                                        })
                                        .unwrap_or(i);
                                    let ft = self.tt.subst(fields[idx].1, &map);
                                    subs[idx] = self.lower_pat(&a.pattern, ft);
                                }
                            }
                        }
                        Pat::Ctor(Ctor::Variant(v), subs)
                    }
                }
            }
            PatternKind::Struct { path, fields, .. } => {
                let TyKind::Adt(d, targs) = self.kind_of(ty) else {
                    return Pat::Opaque;
                };
                let named = path.segments.last().and_then(|l| self.used(l));
                if named != Some(d) {
                    return Pat::Opaque;
                }
                let all = self.collect_fields(d, &targs);
                let mut subs = vec![Pat::Wild; all.len()];
                for f in fields {
                    let name = self.text(&f.name);
                    let Some(pos) = all.iter().position(|(fd, _)| self.def(*fd).name == name)
                    else {
                        continue;
                    };
                    if let Some(sub) = &f.pattern {
                        subs[pos] = self.lower_pat(sub, all[pos].1);
                    }
                }
                Pat::Ctor(Ctor::Single, subs)
            }
        }
    }

    // ---- The matrix method ---------------------------------------------------------------------------

    fn signature(&mut self, ty: Ty) -> Sig {
        let ty = self.tt.shallow(ty);
        match self.kind_of(ty) {
            TyKind::Bool => Sig::Ctors(vec![
                (Ctor::Bool(true), "true".into(), Vec::new()),
                (Ctor::Bool(false), "false".into(), Vec::new()),
            ]),
            TyKind::Option(e) => Sig::Ctors(vec![
                (Ctor::Some, "some".into(), vec![e]),
                (Ctor::None, "none".into(), Vec::new()),
            ]),
            TyKind::Result(a, b) => Sig::Ctors(vec![
                (Ctor::Ok, "ok".into(), vec![a]),
                (Ctor::Err, "err".into(), vec![b]),
            ]),
            TyKind::Tuple(ts) => Sig::Ctors(vec![(Ctor::Single, String::new(), ts)]),
            TyKind::Void => Sig::Ctors(vec![(Ctor::Single, "()".into(), Vec::new())]),
            TyKind::List(e) | TyKind::Array(e, _) => Sig::List(e),
            TyKind::Adt(d, targs) => {
                let map = self.adt_map_of(d, &targs);
                match self.def(d).kind {
                    DefKind::Enum => {
                        let members: Vec<DefId> = self.def(d).members.clone();
                        let mut out = Vec::new();
                        for m in members {
                            if self.def(m).kind != DefKind::Variant {
                                continue;
                            }
                            let tys: Vec<Ty> = self
                                .variant_fields(m)
                                .into_iter()
                                .map(|(_, t)| self.tt.subst(t, &map))
                                .collect();
                            out.push((Ctor::Variant(m), self.def(m).name.clone(), tys));
                        }
                        Sig::Ctors(out)
                    }
                    DefKind::Struct | DefKind::Obj => {
                        let tys: Vec<Ty> = self
                            .collect_fields(d, &targs)
                            .into_iter()
                            .map(|(_, t)| t)
                            .collect();
                        Sig::Ctors(vec![(Ctor::Single, self.def(d).name.clone(), tys)])
                    }
                    _ => Sig::Infinite,
                }
            }
            _ => Sig::Infinite,
        }
    }

    /// A witness (one text per column) of a value no row matches, or `None` when all are covered.
    fn missing(&mut self, rows: &[Vec<Pat>], tys: &[Ty], depth: u32) -> Option<Vec<String>> {
        if tys.is_empty() {
            return if rows.is_empty() {
                Some(Vec::new())
            } else {
                None
            };
        }
        if depth > 40 {
            return None;
        }
        let rest_tys = &tys[1..];
        // Nothing matches: the first constructor, with anything inside, is a witness.
        if rows.is_empty() {
            let mut out = Vec::new();
            match self.signature(tys[0]) {
                Sig::Ctors(ctors) if !ctors.is_empty() => {
                    let (c, name, args) = &ctors[0];
                    let subs = vec!["_".to_string(); args.len()];
                    out.push(ctor_text(c, name, &subs));
                }
                _ => out.push("_".to_string()),
            }
            out.extend(vec!["_".to_string(); rest_tys.len()]);
            return Some(out);
        }
        // Only wildcards in the first column: the column does not matter.
        if rows.iter().all(|r| matches!(r[0], Pat::Wild)) {
            let def: Vec<Vec<Pat>> = rows.iter().map(|r| r[1..].to_vec()).collect();
            let w = self.missing(&def, rest_tys, depth + 1);
            return w.map(|w| {
                let mut out = vec!["_".to_string()];
                out.extend(w);
                out
            });
        }
        match self.signature(tys[0]) {
            Sig::Ctors(ctors) => {
                for (c, name, args) in ctors {
                    let sp: Vec<Vec<Pat>> = rows
                        .iter()
                        .filter_map(|r| specialize(r, &c, args.len()))
                        .collect();
                    let mut sub_tys = args.clone();
                    sub_tys.extend_from_slice(rest_tys);
                    if let Some(w) = self.missing(&sp, &sub_tys, depth + 1) {
                        let (head, tail) = w.split_at(args.len());
                        let mut out = vec![ctor_text(&c, &name, head)];
                        out.extend_from_slice(tail);
                        return Some(out);
                    }
                }
                None
            }
            Sig::List(elem) => {
                let longest = rows
                    .iter()
                    .map(|r| match &r[0] {
                        Pat::List { prefix, suffix, .. } => prefix.len() + suffix.len(),
                        _ => 0,
                    })
                    .max()
                    .unwrap_or(0);
                for n in 0..=longest + 1 {
                    let sp: Vec<Vec<Pat>> =
                        rows.iter().filter_map(|r| specialize_list(r, n)).collect();
                    let mut sub_tys = vec![elem; n];
                    sub_tys.extend_from_slice(rest_tys);
                    if let Some(w) = self.missing(&sp, &sub_tys, depth + 1) {
                        let (head, tail) = w.split_at(n);
                        let mut text = head.join(", ");
                        if n == longest + 1 {
                            if !text.is_empty() {
                                text.push_str(", ");
                            }
                            text.push_str("...");
                        }
                        let mut out = vec![format!("[{text}]")];
                        out.extend_from_slice(tail);
                        return Some(out);
                    }
                }
                None
            }
            Sig::Infinite => {
                let def: Vec<Vec<Pat>> = rows
                    .iter()
                    .filter(|r| matches!(r[0], Pat::Wild))
                    .map(|r| r[1..].to_vec())
                    .collect();
                let w = self.missing(&def, rest_tys, depth + 1)?;
                let mut out = vec!["_".to_string()];
                out.extend(w);
                Some(out)
            }
        }
    }
}

fn ctor_text(c: &Ctor, name: &str, subs: &[String]) -> String {
    let inner = subs.join(", ");
    match c {
        Ctor::Variant(_) if subs.is_empty() => format!(".{name}"),
        Ctor::Variant(_) => format!(".{name}({inner})"),
        Ctor::Bool(_) | Ctor::None => name.to_string(),
        Ctor::Some | Ctor::Ok | Ctor::Err => format!("{name}({inner})"),
        Ctor::Single if name.is_empty() => format!("({inner})"),
        Ctor::Single if subs.is_empty() => name.to_string(),
        Ctor::Single => format!("{name} {{ .. }}"),
    }
}

/// The row as seen when its first column is known to be `c`; `None` if it cannot match that.
fn specialize(row: &[Pat], c: &Ctor, arity: usize) -> Option<Vec<Pat>> {
    match &row[0] {
        Pat::Wild => {
            let mut out = vec![Pat::Wild; arity];
            out.extend_from_slice(&row[1..]);
            Some(out)
        }
        Pat::Ctor(rc, subs) if rc == c => {
            let mut out = subs.clone();
            out.resize(arity, Pat::Wild);
            out.extend_from_slice(&row[1..]);
            Some(out)
        }
        _ => None,
    }
}

/// The row as seen when its first column is a list of exactly `n` items.
fn specialize_list(row: &[Pat], n: usize) -> Option<Vec<Pat>> {
    match &row[0] {
        Pat::Wild => {
            let mut out = vec![Pat::Wild; n];
            out.extend_from_slice(&row[1..]);
            Some(out)
        }
        Pat::List {
            prefix,
            suffix,
            rest,
        } => {
            let fixed = prefix.len() + suffix.len();
            if (!*rest && fixed != n) || (*rest && fixed > n) {
                return None;
            }
            let mut out = prefix.clone();
            out.extend(vec![Pat::Wild; n - fixed]);
            out.extend(suffix.iter().cloned());
            out.extend_from_slice(&row[1..]);
            Some(out)
        }
        _ => None,
    }
}
