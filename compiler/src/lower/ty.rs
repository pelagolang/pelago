//! Types for lowering: how each source type is stored, and how big it is.
//!
//! - **Scalars** are passed and kept in SSA values: integers, floats, `bool`, `char`, raw `ptr`, `box<T>`
//!   and `obj` references (all addresses).
//! - **Everything else** lives in memory and is handled by address: structs, tuples, enums, `option`,
//!   `result`, `string`, `str`, lists, maps, function values.
//! - **Zero-sized** types (`void`, `never`) have no value at all.
//!
//! Fields are laid out in declaration order with the padding their alignment needs. An enum is a tag
//! byte followed by the payload of the active case; `option` and `result` are enums with fixed cases.

use super::*;
use crate::types::{ArrayLen, TyKind};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    pub size: u32,
    pub align: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Repr {
    Zst,
    Scalar(IrTy),
    Mem(Layout),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Field {
    pub off: u32,
    pub ty: Ty,
}

#[derive(Debug)]
pub(crate) struct EnumInfo {
    pub tag: IrTy,
    pub payload_off: u32,
    /// The fields of each case, in case order.
    pub variants: Vec<Vec<Field>>,
    pub layout: Layout,
}

pub(crate) fn align_up(n: u32, a: u32) -> u32 {
    let a = a.max(1);
    n.div_ceil(a) * a
}

impl<'a> Lowerer<'a> {
    /// A type with the current function's type parameters replaced and inference variables followed.
    pub(crate) fn conc(&mut self, t: Ty) -> Ty {
        let t = if self.f.subst.is_empty() {
            t
        } else {
            let map = self.f.subst.clone();
            self.types.subst(t, &map)
        };
        self.types.zonk(t)
    }

    pub(crate) fn kind(&self, t: Ty) -> TyKind {
        self.types.kind_of(t).clone()
    }

    pub(crate) fn mk_ptr(&mut self) -> Ty {
        self.types.mk(TyKind::Ptr)
    }

    pub(crate) fn mk_int(&self) -> Ty {
        self.types.int
    }

    /// The scalar IR type, when the type is kept in a value.
    pub(crate) fn scalar_of(&mut self, t: Ty) -> Option<IrTy> {
        match self.repr(t) {
            Repr::Scalar(s) => Some(s),
            _ => None,
        }
    }

    pub(crate) fn repr(&mut self, t: Ty) -> Repr {
        let t = self.types.shallow(t);
        match self.kind(t) {
            TyKind::Int(k) => Repr::Scalar(IrTy::Int(k.bits() as u8)),
            TyKind::Float(FloatKind::F32) => Repr::Scalar(IrTy::F32),
            TyKind::Float(FloatKind::F64) => Repr::Scalar(IrTy::F64),
            TyKind::Bool => Repr::Scalar(IrTy::Bool),
            TyKind::Char => Repr::Scalar(IrTy::Int(32)),
            TyKind::Void | TyKind::Never => Repr::Zst,
            TyKind::Ptr | TyKind::Box(_) => Repr::Scalar(IrTy::Ptr),
            TyKind::Adt(d, _) if self.res.def(d).kind == DefKind::Obj => Repr::Scalar(IrTy::Ptr),
            TyKind::Unknown | TyKind::Var(_) | TyKind::Param(_) | TyKind::Error => {
                // Not a concrete type: the lowering cannot go on.
                Repr::Zst
            }
            _ => Repr::Mem(self.layout(t)),
        }
    }

    pub(crate) fn is_scalar(&mut self, t: Ty) -> bool {
        matches!(self.repr(t), Repr::Scalar(_))
    }

    pub(crate) fn is_mem(&mut self, t: Ty) -> bool {
        matches!(self.repr(t), Repr::Mem(_))
    }

    pub(crate) fn layout(&mut self, t: Ty) -> Layout {
        let t = self.types.shallow(t);
        if let Some(l) = self.layouts.get(&t) {
            return *l;
        }
        let l = self.compute_layout(t);
        self.layouts.insert(t, l);
        l
    }

    fn compute_layout(&mut self, t: Ty) -> Layout {
        match self.kind(t) {
            TyKind::Int(_)
            | TyKind::Float(_)
            | TyKind::Bool
            | TyKind::Char
            | TyKind::Ptr
            | TyKind::Box(_) => {
                let s = self.scalar_of(t).unwrap_or(IrTy::I64);
                Layout {
                    size: s.size(),
                    align: s.size().max(1),
                }
            }
            TyKind::Void | TyKind::Never => Layout { size: 0, align: 1 },
            TyKind::Adt(d, _) => match self.res.def(d).kind {
                DefKind::Obj => Layout { size: 8, align: 8 },
                DefKind::Enum => self
                    .enum_info(t)
                    .map_or(Layout { size: 0, align: 1 }, |e| e.layout),
                _ => self.struct_layout(t),
            },
            TyKind::Option(_) | TyKind::Result(..) => self
                .enum_info(t)
                .map_or(Layout { size: 0, align: 1 }, |e| e.layout),
            TyKind::Array(e, len) => {
                let n = match len {
                    ArrayLen::Known(n) => n as u32,
                    _ => 0,
                };
                let el = self.layout(e);
                Layout {
                    size: el.size * n,
                    align: el.align,
                }
            }
            _ => self.struct_layout(t),
        }
    }

    fn struct_layout(&mut self, t: Ty) -> Layout {
        let fields = self.struct_fields(t);
        self.fields_layout(&fields)
    }

    pub(crate) fn fields_layout(&mut self, fields: &[Field]) -> Layout {
        let mut size = 0;
        let mut align = 1;
        for f in fields {
            let l = self.layout(f.ty);
            size = size.max(f.off + l.size);
            align = align.max(l.align);
        }
        Layout {
            size: align_up(size, align),
            align,
        }
    }

    /// Offsets for a list of field types laid out in order, starting at `start`.
    pub(crate) fn place_fields(&mut self, start: u32, tys: &[Ty]) -> Vec<Field> {
        let mut off = start;
        let mut out = Vec::new();
        for &ty in tys {
            let l = self.layout(ty);
            off = align_up(off, l.align);
            out.push(Field { off, ty });
            off += l.size;
        }
        out
    }

    /// The type arguments of an ADT as a substitution for its type parameters.
    pub(crate) fn adt_subst(&mut self, d: DefId, args: &[Ty]) -> HashMap<DefId, Ty> {
        let gens = self
            .typed
            .adts
            .get(&d)
            .map(|a| a.generics.clone())
            .unwrap_or_default();
        gens.into_iter().zip(args.iter().copied()).collect()
    }

    /// The fields of a struct, tuple or built-in record type, with their offsets.
    pub(crate) fn struct_fields(&mut self, t: Ty) -> Vec<Field> {
        let t = self.types.shallow(t);
        if let Some(f) = self.fields_cache.get(&t) {
            return f.clone();
        }
        let ptr = self.mk_ptr();
        let int = self.mk_int();
        let tys: Vec<Ty> = match self.kind(t) {
            TyKind::Tuple(ts) => ts,
            TyKind::AltTuple(ts, _) => ts,
            TyKind::Adt(d, args) => {
                let map = self.adt_subst(d, &args);
                let fields = self
                    .typed
                    .adts
                    .get(&d)
                    .map(|a| a.fields.clone())
                    .unwrap_or_default();
                fields
                    .into_iter()
                    .map(|(_, ft)| {
                        let s = self.types.subst(ft, &map);
                        self.types.zonk(s)
                    })
                    .collect()
            }
            // string: pointer, length, capacity (capacity 0 means the text is borrowed or static).
            TyKind::String | TyKind::List(_) | TyKind::Iter(_) => vec![ptr, int, int],
            TyKind::Str => vec![ptr, int],
            TyKind::Map(..) | TyKind::Set(_) => vec![ptr, int, int, int],
            TyKind::Range(e) => vec![e, e, self.types.bool_],
            // A function value: code address and environment address.
            TyKind::Fn(..) => vec![ptr, ptr],
            // `any`: the value's address and its table of methods.
            TyKind::Any(_) => vec![ptr, ptr],
            // The catch-all error: its message.
            TyKind::Error => vec![self.types.string],
            _ => Vec::new(),
        };
        let fields = self.place_fields(0, &tys);
        self.fields_cache.insert(t, fields.clone());
        fields
    }

    /// The layout of an enum, `option` or `result`.
    pub(crate) fn enum_info(&mut self, t: Ty) -> Option<Rc<EnumInfo>> {
        let t = self.types.shallow(t);
        if let Some(e) = self.enums.get(&t) {
            return Some(e.clone());
        }
        let cases: Vec<Vec<Ty>> = match self.kind(t) {
            TyKind::Option(e) => vec![vec![], vec![e]],
            TyKind::Result(a, b) => vec![vec![a], vec![b]],
            TyKind::Adt(d, args) if self.res.def(d).kind == DefKind::Enum => {
                let map = self.adt_subst(d, &args);
                let variants = self
                    .typed
                    .adts
                    .get(&d)
                    .map(|a| a.variants.clone())
                    .unwrap_or_default();
                variants
                    .into_iter()
                    .map(|(_, fs)| {
                        fs.into_iter()
                            .map(|(_, ft)| {
                                let s = self.types.subst(ft, &map);
                                self.types.zonk(s)
                            })
                            .collect()
                    })
                    .collect()
            }
            _ => return None,
        };
        let tag = if cases.len() <= 256 {
            IrTy::Int(8)
        } else {
            IrTy::Int(32)
        };
        // The payloads of all cases start at the same offset.
        let mut max_align = tag.size();
        for case in &cases {
            for &ft in case {
                max_align = max_align.max(self.layout(ft).align);
            }
        }
        let payload_off = align_up(tag.size(), max_align);
        let mut variants = Vec::new();
        let mut end = payload_off;
        for case in &cases {
            let fs = self.place_fields(payload_off, case);
            for f in &fs {
                end = end.max(f.off + self.layout(f.ty).size);
            }
            variants.push(fs);
        }
        let info = Rc::new(EnumInfo {
            tag,
            payload_off,
            variants,
            layout: Layout {
                size: align_up(end, max_align),
                align: max_align,
            },
        });
        self.enums.insert(t, info.clone());
        Some(info)
    }

    /// The position of an enum case among the cases.
    pub(crate) fn variant_index(&self, enum_def: DefId, variant: DefId) -> Option<usize> {
        self.typed
            .adts
            .get(&enum_def)?
            .variants
            .iter()
            .position(|(v, _)| *v == variant)
    }

    /// Does a value of this type need to be destroyed when it goes away?
    pub(crate) fn needs_drop(&mut self, t: Ty) -> bool {
        let t = self.types.shallow(t);
        if let Some(b) = self.drop_cache.get(&t) {
            return *b;
        }
        // Break cycles: a recursive type through a `box` is dropped by its box.
        self.drop_cache.insert(t, false);
        let r = match self.kind(t) {
            TyKind::String
            | TyKind::List(_)
            | TyKind::Iter(_)
            | TyKind::Map(..)
            | TyKind::Set(_)
            | TyKind::Box(_) => true,
            TyKind::Error => true,
            TyKind::Any(_) => true,
            TyKind::Fn(..) => true,
            TyKind::Adt(d, _) if self.res.def(d).kind == DefKind::Obj => true,
            TyKind::Adt(d, _) => {
                if self.res.member(d, "deinit").is_some() {
                    true
                } else if self.res.def(d).kind == DefKind::Enum {
                    let info = self.enum_info(t);
                    let tys: Vec<Ty> = info
                        .map(|e| e.variants.iter().flatten().map(|f| f.ty).collect())
                        .unwrap_or_default();
                    tys.into_iter().any(|x| self.needs_drop(x))
                } else {
                    let fs = self.struct_fields(t);
                    fs.into_iter().any(|f| self.needs_drop(f.ty))
                }
            }
            TyKind::Option(_) | TyKind::Result(..) => {
                let info = self.enum_info(t);
                let tys: Vec<Ty> = info
                    .map(|e| e.variants.iter().flatten().map(|f| f.ty).collect())
                    .unwrap_or_default();
                tys.into_iter().any(|x| self.needs_drop(x))
            }
            TyKind::Tuple(ts) => ts.into_iter().any(|x| self.needs_drop(x)),
            TyKind::Array(e, _) => self.needs_drop(e),
            TyKind::Fn(..) => false,
            _ => false,
        };
        self.drop_cache.insert(t, r);
        r
    }
}
