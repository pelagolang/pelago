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
    /// The two lanes of a `v2i64`.
    Vec([u64; 2]),
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
const STACK_SIZE: usize = 8 << 20;
/// Deep chains (a list of ten thousand boxes) recurse once per node; the host thread that runs the
/// machine gets a big stack for it.
const HOST_STACK: usize = 1 << 30;
const MAX_DEPTH: u32 = 200_000;
/// A context started by `CtxInit` runs on a host thread of its own; it gets a smaller host stack and call depth.
const CTX_HOST_STACK: usize = 64 << 20;
const CTX_MAX_DEPTH: u32 = 20_000;
/// Context handles are numbers like function addresses: far outside memory, 16 apart.
const CTX_BASE: usize = 1 << 41;
/// Starts the message of the error that unwinds a context thread after the run has ended.
const SHUTDOWN: &str = "\0shutdown";

/// Contexts (`CtxInit`, `CtxSwitch`) are host threads that pass a baton: exactly one runs at a time, so
/// the machine is never used by two threads at once and the order is fixed by the program.
struct Baton {
    /// The context allowed to run (0 is the one `main` started in).
    current: usize,
    /// The first failure of any context; every waiting context returns it.
    fail: Option<String>,
    done: bool,
}

struct Ctxs {
    baton: std::sync::Mutex<Baton>,
    cv: std::sync::Condvar,
}

struct CtxInfo {
    lo: usize,
    hi: usize,
    code: usize,
    env: Val,
    started: bool,
}

/// The machine's address, passed to a context thread. Only the thread holding the baton uses it.
struct MachinePtr(usize);

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
    max_steps: u64,
    depth: u32,
    max_depth: u32,
    trace: Vec<String>,
    /// The part of memory slots are taken from (`sp` moves inside it).
    stack_lo: usize,
    stack_hi: usize,
    ctxs: Vec<CtxInfo>,
    /// The context running now.
    me: usize,
    baton: std::sync::Arc<Ctxs>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

/// What a run gave: the result (or the message of a stop), the text written, and the bytes of heap
/// memory never freed.
#[derive(Debug)]
pub struct Outcome {
    pub result: Result<Val, String>,
    pub output: String,
    pub leaked: usize,
    /// Instructions executed: the metric for the optimiser.
    pub steps: u64,
    /// When the run stopped with an error: the functions that were running, innermost first (at most 12).
    pub trace: Vec<String>,
}

/// Runs the function called `name`. `Err` holds the message of a stop.
pub fn run(m: &Module, name: &str, args: &[Val]) -> Result<Val, String> {
    run_full(m, name, args).result
}

pub fn run_full(m: &Module, name: &str, args: &[Val]) -> Outcome {
    run_inner(m, name, args, DEFAULT_STEPS, None).0
}

/// A run that also keeps what is left in memory, for compile-time code (`meta`).
pub struct Capture {
    pub outcome: Outcome,
    /// All of memory when the run ended.
    pub image: Vec<u8>,
    /// Where the result block was (when one was asked for).
    pub ret_addr: usize,
}

/// Runs `name` with a step budget. With `ret_size`, a block of that many bytes is allocated first
/// and its address is the only argument (the function writes its result there, as functions
/// returning memory values do); the block does not count as a leak.
pub fn run_capture(m: &Module, name: &str, ret_size: Option<usize>, max_steps: u64) -> Capture {
    let (outcome, image, ret_addr) = run_inner(m, name, &[], max_steps, ret_size);
    Capture {
        outcome,
        image,
        ret_addr,
    }
}

const DEFAULT_STEPS: u64 = 200_000_000;

fn run_inner(
    m: &Module,
    name: &str,
    args: &[Val],
    max_steps: u64,
    ret_size: Option<usize>,
) -> (Outcome, Vec<u8>, usize) {
    let Some(i) = m.funcs.iter().position(|f| f.name == name) else {
        return (
            Outcome {
                result: Err(format!("no function `{name}`")),
                output: String::new(),
                leaked: 0,
                steps: 0,
                trace: Vec::new(),
            },
            Vec::new(),
            0,
        );
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
        max_steps,
        depth: 0,
        max_depth: MAX_DEPTH,
        trace: Vec::new(),
        stack_lo: stack_base,
        stack_hi: stack_base + STACK_SIZE,
        ctxs: vec![CtxInfo {
            lo: 0,
            hi: 0,
            code: 0,
            env: Val::Unit,
            started: true,
        }],
        me: 0,
        baton: std::sync::Arc::new(Ctxs {
            baton: std::sync::Mutex::new(Baton {
                current: 0,
                fail: None,
                done: false,
            }),
            cv: std::sync::Condvar::new(),
        }),
        threads: Vec::new(),
    };
    let mut call_args: Vec<Val> = args.to_vec();
    let mut ret_addr = 0;
    if let Some(n) = ret_size {
        ret_addr = mc.alloc(n.max(1));
        call_args = vec![Val::Ptr(ret_addr)];
    }
    let result = std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(HOST_STACK)
            .spawn_scoped(s, || {
                let r = mc.call(i, &call_args);
                mc.shutdown();
                r
            })
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err("the interpreter crashed".to_string()))
            })
            .unwrap_or_else(|_| Err("cannot start the interpreter thread".to_string()))
    });
    if ret_size.is_some() {
        if let Some(b) = mc.heap.get_mut(&ret_addr) {
            b.1 = false;
        }
    }
    let leaked = mc
        .heap
        .values()
        .filter(|(_, live)| *live)
        .map(|(s, _)| *s)
        .sum();
    let outcome = Outcome {
        result,
        output: String::from_utf8_lossy(&mc.out).into_owned(),
        leaked,
        steps: mc.steps,
        trace: std::mem::take(&mut mc.trace),
    };
    let image = if ret_size.is_some() {
        std::mem::take(&mut mc.mem)
    } else {
        Vec::new()
    };
    (outcome, image, ret_addr)
}

impl Machine<'_> {
    fn call(&mut self, fi: usize, args: &[Val]) -> Result<Val, String> {
        let r = self.call_inner(fi, args);
        if let Err(msg) = &r {
            if self.trace.len() < 12 && msg != SHUTDOWN {
                self.trace.push(self.m.funcs[fi].name.clone());
            }
        }
        r
    }

    fn call_inner(&mut self, fi: usize, args: &[Val]) -> Result<Val, String> {
        self.depth += 1;
        if self.depth > self.max_depth {
            return Err("the stack is full".to_string());
        }
        let f = &self.m.funcs[fi];
        let frame = self.sp;
        let mut vals: Vec<Val> = vec![Val::Unit; f.values.len()];
        let mut block = 0usize;
        // A slot is one piece of the frame: executed again (in a loop), it is zeroed, not allocated anew.
        let mut slot_at: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
        for (p, a) in f.blocks[0].params.iter().zip(args.iter()) {
            vals[p.0 as usize] = *a;
        }
        let result = loop {
            let b = &f.blocks[block];
            for &id in &b.insts {
                self.steps += 1;
                if self.steps > self.max_steps {
                    return Err("the step limit was reached".to_string());
                }
                let inst = &f.insts[id];
                let v = match &inst.op {
                    Op::Slot(size, align) => {
                        let at = match slot_at.get(&id) {
                            Some(at) => {
                                for b in &mut self.mem[*at..*at + *size as usize] {
                                    *b = 0;
                                }
                                *at
                            }
                            None => {
                                let Val::Ptr(at) = self.exec(f, &vals, &inst.op)? else {
                                    return Err("a slot gave no address".to_string());
                                };
                                slot_at.insert(id, at);
                                at
                            }
                        };
                        let _ = align;
                        Val::Ptr(at)
                    }
                    op => self.exec(f, &vals, op)?,
                };
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

    // ---- Contexts -------------------------------------------------------------------------------

    // These stay out of line: `exec` recurses once per interpreted call, so its frame must stay small.
    #[inline(never)]
    fn ctx_init(&mut self, lo: usize, size: i64, code: usize, env: Val) -> Result<Val, String> {
        if size < 0
            || lo
                .checked_add(size as usize)
                .is_none_or(|e| e > self.mem.len())
        {
            return Err("a context needs a stack inside memory".to_string());
        }
        if code < FUNC_BASE || code - FUNC_BASE >= self.m.funcs.len() {
            return Err("a context starts at a bad function address".to_string());
        }
        self.ctxs.push(CtxInfo {
            lo,
            hi: lo + size as usize,
            code: code - FUNC_BASE,
            env,
            started: false,
        });
        Ok(Val::Ptr(CTX_BASE + (self.ctxs.len() - 1) * 16))
    }

    #[inline(never)]
    fn ctx_switch(&mut self, save: usize, to: usize) -> Result<(), String> {
        let id = to.wrapping_sub(CTX_BASE) / 16;
        if to < CTX_BASE || !(to - CTX_BASE).is_multiple_of(16) || id >= self.ctxs.len() {
            return Err("switched to something that is not a context".to_string());
        }
        self.check(save, 8)?;
        let me = self.me;
        self.write(save, IrTy::Ptr, Val::Ptr(CTX_BASE + me * 16))?;
        if id != me {
            self.switch_to(id)?;
        }
        Ok(())
    }

    /// Hands the baton to context `id` (starting its thread the first time) and waits to get it back.
    #[inline(never)]
    fn switch_to(&mut self, id: usize) -> Result<(), String> {
        let me = self.me;
        let saved = (
            self.sp,
            self.stack_lo,
            self.stack_hi,
            self.depth,
            self.max_depth,
        );
        let baton = self.baton.clone();
        if !self.ctxs[id].started {
            self.ctxs[id].started = true;
            let info = &self.ctxs[id];
            let (lo, hi, code, env) = (info.lo, info.hi, info.code, info.env);
            let mp = MachinePtr(self as *mut Machine<'_> as usize);
            let b = baton.clone();
            baton.baton.lock().map_err(|_| "a context crashed")?.current = id;
            let h = std::thread::Builder::new()
                .stack_size(CTX_HOST_STACK)
                .spawn(move || {
                    let mp = mp;
                    // SAFETY: the machine outlives every context thread (`shutdown` joins them), and only
                    // the context holding the baton touches it; the spawner waits below.
                    let mc: &mut Machine<'_> = unsafe { &mut *(mp.0 as *mut Machine<'_>) };
                    mc.me = id;
                    mc.stack_lo = lo;
                    mc.stack_hi = hi;
                    mc.sp = lo;
                    mc.depth = 0;
                    mc.max_depth = CTX_MAX_DEPTH;
                    let r = mc.call(code, &[env]);
                    let msg = match r {
                        Err(m) => m,
                        Ok(_) => "a context returned; it must switch away instead".to_string(),
                    };
                    if let Ok(mut st) = b.baton.lock() {
                        if st.fail.is_none() {
                            st.fail = Some(msg);
                        }
                    }
                    b.cv.notify_all();
                })
                .map_err(|_| "cannot start a context thread".to_string())?;
            self.threads.push(h);
        } else {
            baton.baton.lock().map_err(|_| "a context crashed")?.current = id;
        }
        baton.cv.notify_all();
        let mut st = baton.baton.lock().map_err(|_| "a context crashed")?;
        loop {
            if let Some(m) = &st.fail {
                // Only the first context reports the failure; the others unwind without touching the machine.
                return Err(if me == 0 {
                    m.clone()
                } else {
                    SHUTDOWN.to_string()
                });
            }
            if st.done {
                return Err(SHUTDOWN.to_string());
            }
            if st.current == me {
                break;
            }
            st = baton.cv.wait(st).map_err(|_| "a context crashed")?;
        }
        drop(st);
        (
            self.sp,
            self.stack_lo,
            self.stack_hi,
            self.depth,
            self.max_depth,
        ) = saved;
        self.me = me;
        Ok(())
    }

    /// Ends the run: contexts still waiting unwind and their threads are joined.
    fn shutdown(&mut self) {
        if let Ok(mut st) = self.baton.baton.lock() {
            st.done = true;
        }
        self.baton.cv.notify_all();
        for h in std::mem::take(&mut self.threads) {
            let _ = h.join();
        }
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
            IrTy::V2I64 => Val::Vec([
                u64::from_le_bytes(b[0..8].try_into().unwrap_or([0; 8])),
                u64::from_le_bytes(b[8..16].try_into().unwrap_or([0; 8])),
            ]),
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
            (IrTy::V2I64, Val::Vec(l)) => l.iter().flat_map(|x| x.to_le_bytes()).collect(),
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
                Const::Float(x) => Val::Float(round_to(*t, *x)),
                Const::Bool(b) => Val::Bool(*b),
            },
            Op::Bin(o, a, b) => bin(*o, get(a), get(b), ty(a))?,
            Op::Checked(o, signed, a, b) => checked(*o, *signed, get(a), get(b), bits_of(ty(a)))?,
            Op::CheckedIf(o, signed, test, a, b) => {
                let stops = match test {
                    Test::Bool(c, when) => get(c) == Val::Bool(*when),
                    Test::Cmp(o, x, y) => cmp(*o, get(x), get(y), bits_of(ty(x))),
                };
                if stops {
                    checked(*o, *signed, get(a), get(b), bits_of(ty(a)))?
                } else {
                    bin(*o, get(a), get(b), ty(a))?
                }
            }
            Op::Select(c, a, b) => match get(c) {
                Val::Bool(true) => get(a),
                _ => get(b),
            },
            Op::Cmp(o, a, b) => Val::Bool(cmp(*o, get(a), get(b), bits_of(ty(a)))),
            Op::Fma(a, b, c) => match (get(a), get(b), get(c)) {
                (Val::Float(x), Val::Float(y), Val::Float(z)) => {
                    Val::Float(if ty(a) == IrTy::F32 {
                        (x as f32).mul_add(y as f32, z as f32) as f64
                    } else {
                        x.mul_add(y, z)
                    })
                }
                (other, ..) => other,
            },
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
            Op::VUn(o, a) => {
                let lanes = |v: Val| match v {
                    Val::Vec(l) => l,
                    _ => [0, 0],
                };
                let int = |v: Val| match v {
                    Val::Int(n) => n as u64,
                    _ => 0,
                };
                match o {
                    VUn::Splat => Val::Vec([int(get(a)); 2]),
                    VUn::Shl(k) => {
                        let l = lanes(get(a));
                        Val::Vec([l[0] << k, l[1] << k])
                    }
                    VUn::UShr(k) => {
                        let l = lanes(get(a));
                        Val::Vec([l[0] >> k, l[1] >> k])
                    }
                    VUn::ReduceAdd => {
                        let l = lanes(get(a));
                        Val::Int(l[0].wrapping_add(l[1]) as u128)
                    }
                }
            }
            Op::VBin(o, a, b) => {
                let (x, y) = (get(a), get(b));
                if let VBin::Build = o {
                    let int = |v: Val| match v {
                        Val::Int(n) => n as u64,
                        _ => 0,
                    };
                    Val::Vec([int(x), int(y)])
                } else {
                    let (Val::Vec(x), Val::Vec(y)) = (x, y) else {
                        return Err(
                            "a vector operation read a value that is not a vector".to_string()
                        );
                    };
                    let lane = |f: fn(u64, u64) -> u64| Val::Vec([f(x[0], y[0]), f(x[1], y[1])]);
                    match o {
                        VBin::Add => lane(u64::wrapping_add),
                        VBin::Sub => lane(u64::wrapping_sub),
                        VBin::And => lane(|a, b| a & b),
                        VBin::Or => lane(|a, b| a | b),
                        VBin::Xor => lane(|a, b| a ^ b),
                        VBin::Build => unreachable!(),
                    }
                }
            }
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
                if end > self.stack_hi {
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
                // Copying nothing touches nothing: cloning an empty list copies from and to null.
                if n > 0 {
                    self.check(d, n)?;
                    self.check(s, n)?;
                    self.mem.copy_within(s..s + n, d);
                }
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
            Op::CtxInit(base, size, code, env) => self.ctx_init(
                addr(&get(base))?,
                get(size).signed(64) as i64,
                addr(&get(code))?,
                get(env),
            )?,
            Op::CtxSwitch(save, to) => {
                self.ctx_switch(addr(&get(save))?, addr(&get(to))?)?;
                Val::Unit
            }
            Op::Realloc(p, n) => {
                let n = get(n).signed(64);
                if !(0..=(1 << 32)).contains(&n) {
                    return Err("allocated a bad size".to_string());
                }
                let old = get(p);
                let new = self.alloc(n as usize);
                if let Val::Ptr(a) = old {
                    if a != 0 {
                        match self.heap.get(&a) {
                            Some((size, true)) => {
                                let keep = (*size).min(n as usize);
                                self.mem.copy_within(a..a + keep, new);
                            }
                            _ => return Err("reallocated memory that is not live".to_string()),
                        }
                    }
                }
                self.free(old)?;
                Val::Ptr(new)
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

/// `add`, `sub` or `mul` that stops on overflow.
fn checked(o: BinOp, signed: bool, x: Val, y: Val, bits: u32) -> Result<Val, String> {
    let (Val::Int(xu), Val::Int(yu)) = (x, y) else {
        return Err("checked arithmetic needs integers".to_string());
    };
    let (lo, hi, xv, yv): (i128, i128, i128, i128) = if signed {
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
        Some(r) if r >= lo && r <= hi => Ok(Val::Int(mask(r as u128, bits))),
        _ => Err("arithmetic overflow".to_string()),
    }
}

/// The value of a pure operation on known operands, for the optimiser: `None` when the operation
/// would stop, is not pure, or its operands are not all known integers or booleans.
pub(crate) fn fold(op: &Op, f: &Func, get: &dyn Fn(Value) -> Option<Val>) -> Option<Val> {
    let known = |v: &Value| get(*v).filter(|x| matches!(x, Val::Int(_) | Val::Bool(_)));
    let r = match op {
        Op::Const(t, c) => match c {
            Const::Int(n) => Val::Int(mask(*n as u128, bits_of(*t))),
            Const::Bool(b) => Val::Bool(*b),
            Const::Float(_) => return None,
        },
        Op::Bin(o, a, b) => bin(*o, known(a)?, known(b)?, f.ty(*a)).ok()?,
        Op::Checked(o, signed, a, b) => {
            checked(*o, *signed, known(a)?, known(b)?, bits_of(f.ty(*a))).ok()?
        }
        Op::Select(c, a, b) => match known(c)? {
            Val::Bool(true) => known(a)?,
            _ => known(b)?,
        },
        Op::Cmp(o, a, b) => Val::Bool(cmp(*o, known(a)?, known(b)?, bits_of(f.ty(*a)))),
        Op::Neg(a) => match known(a)? {
            Val::Int(n) => Val::Int(mask(0u128.wrapping_sub(n), bits_of(f.ty(*a)))),
            _ => return None,
        },
        Op::Not(a) => match known(a)? {
            Val::Bool(b) => Val::Bool(!b),
            Val::Int(n) => Val::Int(mask(!n, bits_of(f.ty(*a)))),
            _ => return None,
        },
        Op::Cast(k, a, to) if !matches!(k, CastKind::PtrToInt | CastKind::IntToPtr) => {
            let v = cast(*k, known(a)?, bits_of(f.ty(*a)), *to);
            matches!(v, Val::Int(_) | Val::Bool(_)).then_some(v)?
        }
        _ => return None,
    };
    Some(r)
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
                BinOp::MulHi if bits <= 64 => ((sx * sy) >> bits) as u128,
                BinOp::UMulHi if bits <= 64 => (x * y) >> bits,
                BinOp::MulHi | BinOp::UMulHi => {
                    return Err("mulhi needs 64 bits or fewer".to_string())
                }
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
        (Val::Float(x), Val::Float(y)) => Ok(Val::Float(round_to(
            t,
            match o {
                BinOp::Add => x + y,
                BinOp::Sub => x - y,
                BinOp::Mul => x * y,
                BinOp::Div => x / y,
                BinOp::Rem => x % y,
                _ => return Err("not a float operation".to_string()),
            },
        ))),
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

/// A `float32` value is held as an `f64` and rounded to single precision after every operation.
fn round_to(t: IrTy, x: f64) -> f64 {
    if t == IrTy::F32 {
        x as f32 as f64
    } else {
        x
    }
}

/// Float to integer like Rust's `as i64`: truncates, saturates, NaN becomes 0. A narrower target
/// then keeps the low bits of that result (the lowering only converts to 64 bits).
fn float_to_int(x: f64, bits: u32) -> u128 {
    mask(x as i64 as i128 as u128, bits)
}

fn cast(k: CastKind, v: Val, from_bits: u32, to: IrTy) -> Val {
    let tb = bits_of(to);
    match (k, v) {
        (CastKind::ZExt, Val::Int(n)) | (CastKind::Trunc, Val::Int(n)) => Val::Int(mask(n, tb)),
        (CastKind::SExt, Val::Int(n)) => Val::Int(mask(sign_extend(n, from_bits) as u128, tb)),
        (CastKind::IntToFloat, Val::Int(n)) => {
            let i = sign_extend(n, from_bits);
            Val::Float(match to {
                // One rounding straight from the integer, not through `f64`.
                IrTy::F32 => i as f32 as f64,
                _ => i as f64,
            })
        }
        (CastKind::FloatToInt, Val::Float(x)) => Val::Int(float_to_int(x, tb)),
        (CastKind::FloatResize, Val::Float(x)) => Val::Float(round_to(to, x)),
        (CastKind::PtrToInt, Val::Ptr(i)) => Val::Int(i as u128),
        (CastKind::IntToPtr, Val::Int(n)) => Val::Ptr(n as usize),
        (_, other) => other,
    }
}
