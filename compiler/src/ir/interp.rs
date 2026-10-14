//! A small interpreter for the IR (IR2). It runs a function and returns its result or the message of a
//! stop. Lowering is tested by running programs through it, and it will later run `meta` code.
//!
//! Integers are kept as bits masked to their width; operations that care about signedness (division,
//! comparison, shifts, checked arithmetic) read them as signed or unsigned.

use super::*;
use std::collections::BTreeMap;

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

/// Function addresses are numbers far outside memory, so a stray load cannot hit one.
const FUNC_BASE: usize = 1 << 40;
const STACK_SIZE: usize = 1 << 20;

struct Machine<'m> {
    m: &'m Module,
    mem: Vec<u8>,
    /// Where each piece of static data starts.
    data_at: Vec<usize>,
    /// The stack lives in `stack_base..stack_base + STACK_SIZE`; `sp` is its top.
    stack_base: usize,
    sp: usize,
    heap_base: usize,
    /// Heap blocks by start address: size and whether still alive.
    heap: BTreeMap<usize, (usize, bool)>,
    out: Vec<u8>,
    steps: u64,
    depth: u32,
}

/// What a run gave: the result (or the message of a stop), the text written, and the bytes of heap
/// memory never freed.
#[derive(Debug)]
pub struct Outcome {
    pub result: Result<Val, String>,
    pub output: String,
    pub leaked: usize,
}

/// Runs the function called `name`. `Err` holds the message of a stop.
pub fn run(m: &Module, name: &str, args: &[Val]) -> Result<Val, String> {
    run_full(m, name, args).result
}

pub fn run_full(m: &Module, name: &str, args: &[Val]) -> Outcome {
    let Some(i) = m.funcs.iter().position(|f| f.name == name) else {
        return Outcome {
            result: Err(format!("no function `{name}`")),
            output: String::new(),
            leaked: 0,
        };
    };
    let mut mem = vec![0u8; 16];
    let mut data_at = Vec::new();
    for d in &m.data {
        while !mem.len().is_multiple_of(16) {
            mem.push(0);
        }
        data_at.push(mem.len());
        mem.extend_from_slice(d);
    }
    while !mem.len().is_multiple_of(16) {
        mem.push(0);
    }
    let stack_base = mem.len();
    mem.resize(stack_base + STACK_SIZE, 0);
    let heap_base = mem.len();
    let mut mc = Machine {
        m,
        mem,
        data_at,
        stack_base,
        sp: stack_base,
        heap_base,
        heap: BTreeMap::new(),
        out: Vec::new(),
        steps: 0,
        depth: 0,
    };
    let result = mc.call(i, args);
    let leaked = mc
        .heap
        .values()
        .filter(|(_, live)| *live)
        .map(|(s, _)| *s)
        .sum();
    Outcome {
        result,
        output: String::from_utf8_lossy(&mc.out).into_owned(),
        leaked,
    }
}

impl Machine<'_> {
    fn call(&mut self, fi: usize, args: &[Val]) -> Result<Val, String> {
        self.depth += 1;
        if self.depth > 5000 {
            return Err("the stack is full".to_string());
        }
        let f = &self.m.funcs[fi];
        let frame = self.sp;
        let mut vals: Vec<Val> = vec![Val::Unit; f.values.len()];
        let mut block = 0usize;
        for (p, a) in f.blocks[0].params.iter().zip(args.iter()) {
            vals[p.0 as usize] = *a;
        }
        let result = loop {
            let b = &f.blocks[block];
            for &id in &b.insts {
                self.steps += 1;
                if self.steps > 200_000_000 {
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
        self.sp = frame;
        self.depth -= 1;
        Ok(result)
    }

    // ---- Memory ---------------------------------------------------------------------------------

    /// Checks that `len` bytes at `addr` may be touched.
    fn check(&self, addr: usize, len: usize) -> Result<(), String> {
        if addr < 16 {
            return Err("used a null address".to_string());
        }
        let end = addr.checked_add(len).ok_or("a bad address")?;
        if end > self.mem.len() {
            return Err("used an address outside memory".to_string());
        }
        if addr >= self.stack_base && addr < self.heap_base {
            if end > self.sp {
                return Err("used the stack of a function that has returned".to_string());
            }
            return Ok(());
        }
        if addr >= self.heap_base {
            return match self.heap.range(..=addr).next_back() {
                Some((start, (size, live))) if addr < start + size.max(&1) || len == 0 => {
                    if !live {
                        Err("used memory after it was freed".to_string())
                    } else if end > start + size {
                        Err("used memory past the end of its block".to_string())
                    } else {
                        Ok(())
                    }
                }
                _ => Err("used an address that is not in any block".to_string()),
            };
        }
        Ok(())
    }

    fn read(&self, ty: IrTy, addr: usize) -> Result<Val, String> {
        let n = ty.size() as usize;
        self.check(addr, n)?;
        let b = &self.mem[addr..addr + n];
        Ok(match ty {
            IrTy::Int(_) => {
                let mut x = 0u128;
                for (i, byte) in b.iter().enumerate() {
                    x |= (*byte as u128) << (8 * i);
                }
                Val::Int(x)
            }
            IrTy::F32 => Val::Float(f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64),
            IrTy::F64 => Val::Float(f64::from_le_bytes(b.try_into().unwrap_or([0; 8]))),
            IrTy::Bool => Val::Bool(b[0] != 0),
            IrTy::Ptr => Val::Ptr(u64::from_le_bytes(b.try_into().unwrap_or([0; 8])) as usize),
            IrTy::Void => Val::Unit,
        })
    }

    fn write(&mut self, addr: usize, ty: IrTy, v: Val) -> Result<(), String> {
        let n = ty.size() as usize;
        self.check(addr, n)?;
        let bytes: Vec<u8> = match (ty, v) {
            (IrTy::Int(_), Val::Int(x)) => (0..n).map(|i| (x >> (8 * i)) as u8).collect(),
            (IrTy::F32, Val::Float(x)) => (x as f32).to_le_bytes().to_vec(),
            (IrTy::F64, Val::Float(x)) => x.to_le_bytes().to_vec(),
            (IrTy::Bool, Val::Bool(b)) => vec![b as u8],
            (IrTy::Ptr, Val::Ptr(p)) => (p as u64).to_le_bytes().to_vec(),
            (IrTy::Ptr, Val::Int(p)) => (p as u64).to_le_bytes().to_vec(),
            (IrTy::Void, _) => Vec::new(),
            _ => return Err("stored a value of the wrong kind".to_string()),
        };
        self.mem[addr..addr + n].copy_from_slice(&bytes);
        Ok(())
    }

    fn alloc(&mut self, size: usize) -> usize {
        while !self.mem.len().is_multiple_of(16) {
            self.mem.push(0);
        }
        // A gap before each block catches writes just past the end of the one before.
        let addr = self.mem.len() + 16;
        self.mem.resize(addr + size, 0);
        self.heap.insert(addr, (size, true));
        addr
    }

    fn free(&mut self, p: Val) -> Result<(), String> {
        let Val::Ptr(addr) = p else {
            return Err("freed a value that is not an address".to_string());
        };
        if addr == 0 {
            return Ok(());
        }
        match self.heap.get_mut(&addr) {
            Some((_, live)) if !*live => Err("freed memory twice".to_string()),
            Some((size, live)) => {
                *live = false;
                let (a, n) = (addr, *size);
                for b in &mut self.mem[a..a + n] {
                    *b = 0xDD;
                }
                Ok(())
            }
            None => Err("freed an address that was not allocated".to_string()),
        }
    }

    fn exec(&mut self, f: &Func, vals: &[Val], op: &Op) -> Result<Val, String> {
        let get = |v: &Value| vals[v.0 as usize];
        let ty = |v: &Value| f.ty(*v);
        let addr = |v: &Val| -> Result<usize, String> {
            match v {
                Val::Ptr(p) => Ok(*p),
                Val::Int(p) => Ok(*p as usize),
                _ => Err("expected an address".to_string()),
            }
        };
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
            Op::CallPtr(_, _, fptr, args) => {
                let a = addr(&get(fptr))?;
                if a < FUNC_BASE || a - FUNC_BASE >= self.m.funcs.len() {
                    return Err("called a bad function address".to_string());
                }
                let argv: Vec<Val> = args.iter().map(get).collect();
                self.call(a - FUNC_BASE, &argv)?
            }
            Op::FuncAddr(fid) => Val::Ptr(FUNC_BASE + fid.0 as usize),
            Op::DataAddr(i) => Val::Ptr(self.data_at[*i as usize]),
            Op::Slot(size, align) => {
                let align = (*align as usize).max(1);
                let at = self.sp.div_ceil(align) * align;
                let end = at + *size as usize;
                if end > self.stack_base + STACK_SIZE {
                    return Err("the stack is full".to_string());
                }
                for b in &mut self.mem[at..end] {
                    *b = 0;
                }
                self.sp = end;
                Val::Ptr(at)
            }
            Op::PtrAdd(p, n) => {
                let base = addr(&get(p))? as i128;
                let off = get(n).signed(64);
                Val::Ptr((base + off) as usize)
            }
            Op::MemCopy(d, s, n) => {
                let (d, s) = (addr(&get(d))?, addr(&get(s))?);
                let n = get(n).signed(64) as usize;
                self.check(d, n)?;
                self.check(s, n)?;
                self.mem.copy_within(s..s + n, d);
                Val::Unit
            }
            Op::Alloc(n) => {
                let n = get(n).signed(64);
                if !(0..=(1 << 32)).contains(&n) {
                    return Err("allocated a bad size".to_string());
                }
                Val::Ptr(self.alloc(n as usize))
            }
            Op::Free(p) => {
                self.free(get(p))?;
                Val::Unit
            }
            Op::Load(t, p) => {
                let a = addr(&get(p))?;
                self.read(*t, a)?
            }
            Op::Store(p, x) => {
                let a = addr(&get(p))?;
                let t = ty(x);
                self.write(a, t, get(x))?;
                Val::Unit
            }
            Op::Print(p, n) => {
                let (a, n) = (addr(&get(p))?, get(n).signed(64) as usize);
                self.check(a, n)?;
                let bytes = self.mem[a..a + n].to_vec();
                self.out.extend_from_slice(&bytes);
                Val::Unit
            }
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
        (Val::Ptr(x), Val::Ptr(y)) => match o {
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
