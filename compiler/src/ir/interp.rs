//! A small interpreter for the IR (IR2). It runs a function and returns its result or the message of a
//! stop. Lowering is tested by running programs through it, and it will later run `meta` code.
//!
//! Integers are kept as bits masked to their width; operations that care about signedness (division,
//! comparison, shifts, checked arithmetic) read them as signed or unsigned.

use super::*;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Val {
    /// The bits of an integer, masked to its width.
    Int(u128),
    Float(f64),
    Bool(bool),
    /// An address: a numbered cell of interpreter memory.
    Ptr(usize),
    Unit,
}

impl Val {
    /// An integer read as signed, for the given width.
    pub fn signed(self, bits: u32) -> i128 {
        match self {
            Val::Int(n) => sign_extend(n, bits),
            _ => 0,
        }
    }

    pub fn as_i64(self) -> i64 {
        self.signed(64) as i64
    }

    pub fn int(n: i64) -> Val {
        Val::Int(mask(n as i128 as u128, 64))
    }
}

fn mask(n: u128, bits: u32) -> u128 {
    if bits >= 128 {
        n
    } else {
        n & ((1u128 << bits) - 1)
    }
}

fn sign_extend(n: u128, bits: u32) -> i128 {
    let n = mask(n, bits);
    if bits < 128 && n >> (bits - 1) & 1 == 1 {
        (n | !((1u128 << bits) - 1)) as i128
    } else {
        n as i128
    }
}

fn bits_of(t: IrTy) -> u32 {
    match t {
        IrTy::Int(b) => b as u32,
        IrTy::Bool => 1,
        _ => 64,
    }
}

struct Machine<'m> {
    m: &'m Module,
    mem: Vec<Val>,
    steps: u64,
    depth: u32,
}

/// Runs the function called `name`. `Err` holds the message of a stop.
pub fn run(m: &Module, name: &str, args: &[Val]) -> Result<Val, String> {
    let Some(i) = m.funcs.iter().position(|f| f.name == name) else {
        return Err(format!("no function `{name}`"));
    };
    let mut mc = Machine {
        m,
        mem: Vec::new(),
        steps: 0,
        depth: 0,
    };
    mc.call(i, args)
}

impl Machine<'_> {
    fn call(&mut self, fi: usize, args: &[Val]) -> Result<Val, String> {
        self.depth += 1;
        if self.depth > 5000 {
            return Err("the stack is full".to_string());
        }
        let f = &self.m.funcs[fi];
        let mut vals: Vec<Val> = vec![Val::Unit; f.values.len()];
        let mut block = 0usize;
        for (p, a) in f.blocks[0].params.iter().zip(args.iter()) {
            vals[p.0 as usize] = *a;
        }
        let result = loop {
            let b = &f.blocks[block];
            for &id in &b.insts {
                self.steps += 1;
                if self.steps > 50_000_000 {
                    return Err("the step limit was reached".to_string());
                }
                let inst = &f.insts[id];
                let v = self.exec(f, &vals, &inst.op)?;
                if let Some(r) = inst.result {
                    vals[r.0 as usize] = v;
                }
            }
            match b.term.as_ref().ok_or("a block has no terminator")? {
                Term::Jump(t) => block = enter(f, &mut vals, t),
                Term::Branch(c, a, e) => {
                    let t = if vals[c.0 as usize] == Val::Bool(true) {
                        a
                    } else {
                        e
                    };
                    block = enter(f, &mut vals, t);
                }
                Term::Return(x) => break x.map_or(Val::Unit, |x| vals[x.0 as usize]),
                Term::Stop(msg) => return Err(msg.clone()),
                Term::Unreachable => return Err("reached unreachable code".to_string()),
            }
        };
        self.depth -= 1;
        Ok(result)
    }

    fn exec(&mut self, f: &Func, vals: &[Val], op: &Op) -> Result<Val, String> {
        let get = |v: &Value| vals[v.0 as usize];
        let ty = |v: &Value| f.ty(*v);
        Ok(match op {
            Op::Const(t, c) => match c {
                Const::Int(n) => Val::Int(mask(*n as u128, bits_of(*t))),
                Const::Float(x) => Val::Float(*x),
                Const::Bool(b) => Val::Bool(*b),
            },
            Op::Bin(o, a, b) => bin(*o, get(a), get(b), ty(a))?,
            Op::Checked(o, signed, a, b) => {
                let bits = bits_of(ty(a));
                let (x, y) = (get(a), get(b));
                let (Val::Int(xu), Val::Int(yu)) = (x, y) else {
                    return Err("checked arithmetic needs integers".to_string());
                };
                let (lo, hi, xv, yv): (i128, i128, i128, i128) = if *signed {
                    (
                        -(1i128 << (bits - 1)),
                        (1i128 << (bits - 1)) - 1,
                        sign_extend(xu, bits),
                        sign_extend(yu, bits),
                    )
                } else {
                    (0, (1i128 << bits.min(126)) - 1, xu as i128, yu as i128)
                };
                let r = match o {
                    BinOp::Add => xv.checked_add(yv),
                    BinOp::Sub => xv.checked_sub(yv),
                    _ => xv.checked_mul(yv),
                };
                match r {
                    Some(r) if r >= lo && r <= hi => Val::Int(mask(r as u128, bits)),
                    _ => return Err("arithmetic overflow".to_string()),
                }
            }
            Op::Cmp(o, a, b) => Val::Bool(cmp(*o, get(a), get(b), bits_of(ty(a)))),
            Op::Neg(a) => match get(a) {
                Val::Float(x) => Val::Float(-x),
                Val::Int(n) => Val::Int(mask(0u128.wrapping_sub(n), bits_of(ty(a)))),
                other => other,
            },
            Op::Not(a) => match get(a) {
                Val::Bool(b) => Val::Bool(!b),
                Val::Int(n) => Val::Int(mask(!n, bits_of(ty(a)))),
                other => other,
            },
            Op::Cast(k, a, to) => cast(*k, get(a), bits_of(ty(a)), *to),
            Op::Call(callee, args) => {
                let argv: Vec<Val> = args.iter().map(get).collect();
                self.call(callee.0 as usize, &argv)?
            }
            Op::Slot(_) | Op::Alloc(_) => {
                self.mem.push(Val::Unit);
                Val::Ptr(self.mem.len() - 1)
            }
            Op::Load(_, p) => match get(p) {
                Val::Ptr(i) => self.mem[i],
                _ => return Err("load from a bad address".to_string()),
            },
            Op::Store(p, x) => {
                match get(p) {
                    Val::Ptr(i) => self.mem[i] = get(x),
                    _ => return Err("store to a bad address".to_string()),
                }
                Val::Unit
            }
            Op::Free(_) => Val::Unit,
        })
    }
}

fn enter(f: &Func, vals: &mut [Val], t: &Target) -> usize {
    let args: Vec<Val> = t.args.iter().map(|a| vals[a.0 as usize]).collect();
    for (p, a) in f.blocks[t.block.0 as usize].params.iter().zip(args) {
        vals[p.0 as usize] = a;
    }
    t.block.0 as usize
}

fn bin(o: BinOp, a: Val, b: Val, t: IrTy) -> Result<Val, String> {
    let bits = bits_of(t);
    match (a, b) {
        (Val::Int(x), Val::Int(y)) => {
            let (sx, sy) = (sign_extend(x, bits), sign_extend(y, bits));
            let r: u128 = match o {
                BinOp::Add => x.wrapping_add(y),
                BinOp::Sub => x.wrapping_sub(y),
                BinOp::Mul => x.wrapping_mul(y),
                BinOp::Div | BinOp::Rem | BinOp::UDiv | BinOp::URem if y == 0 => {
                    return Err("division by zero".to_string())
                }
                BinOp::Div => sx.wrapping_div(sy) as u128,
                BinOp::Rem => sx.wrapping_rem(sy) as u128,
                BinOp::UDiv => x / y,
                BinOp::URem => x % y,
                BinOp::And => x & y,
                BinOp::Or => x | y,
                BinOp::Xor => x ^ y,
                BinOp::Shl | BinOp::Shr | BinOp::UShr if y >= bits as u128 => {
                    return Err("shifted by the width or more".to_string())
                }
                BinOp::Shl => x << y,
                BinOp::Shr => (sx >> y) as u128,
                BinOp::UShr => x >> y,
            };
            Ok(Val::Int(mask(r, bits)))
        }
        (Val::Float(x), Val::Float(y)) => Ok(Val::Float(match o {
            BinOp::Add => x + y,
            BinOp::Sub => x - y,
            BinOp::Mul => x * y,
            BinOp::Div => x / y,
            BinOp::Rem => x % y,
            _ => return Err("not a float operation".to_string()),
        })),
        _ => Err("operands do not match".to_string()),
    }
}

fn cmp(o: CmpOp, a: Val, b: Val, bits: u32) -> bool {
    match (a, b) {
        (Val::Int(x), Val::Int(y)) => {
            let (sx, sy) = (sign_extend(x, bits), sign_extend(y, bits));
            match o {
                CmpOp::Eq => x == y,
                CmpOp::Ne => x != y,
                CmpOp::Lt => sx < sy,
                CmpOp::Le => sx <= sy,
                CmpOp::Gt => sx > sy,
                CmpOp::Ge => sx >= sy,
                CmpOp::ULt => x < y,
                CmpOp::ULe => x <= y,
                CmpOp::UGt => x > y,
                CmpOp::UGe => x >= y,
            }
        }
        (Val::Float(x), Val::Float(y)) => match o {
            CmpOp::Eq => x == y,
            CmpOp::Ne => x != y,
            CmpOp::Lt | CmpOp::ULt => x < y,
            CmpOp::Le | CmpOp::ULe => x <= y,
            CmpOp::Gt | CmpOp::UGt => x > y,
            CmpOp::Ge | CmpOp::UGe => x >= y,
        },
        _ => match o {
            CmpOp::Eq => a == b,
            CmpOp::Ne => a != b,
            _ => false,
        },
    }
}

fn cast(k: CastKind, v: Val, from_bits: u32, to: IrTy) -> Val {
    let tb = bits_of(to);
    match (k, v) {
        (CastKind::ZExt, Val::Int(n)) | (CastKind::Trunc, Val::Int(n)) => Val::Int(mask(n, tb)),
        (CastKind::SExt, Val::Int(n)) => Val::Int(mask(sign_extend(n, from_bits) as u128, tb)),
        (CastKind::IntToFloat, Val::Int(n)) => Val::Float(sign_extend(n, from_bits) as f64),
        (CastKind::FloatToInt, Val::Float(x)) => Val::Int(mask(x as i128 as u128, tb)),
        (CastKind::FloatResize, Val::Float(x)) => Val::Float(x),
        (CastKind::PtrToInt, Val::Ptr(i)) => Val::Int(i as u128),
        (CastKind::IntToPtr, Val::Int(n)) => Val::Ptr(n as usize),
        (_, other) => other,
    }
}
