//! Tests of `mir.rs` and `ralloc.rs` on a toy target: a small machine with three scratch registers,
//! three caller-saved and three callee-saved registers for integers, and a few for floats. A small
//! interpreter runs a function before allocation (virtual registers) and after it (physical ones);
//! calls scramble every register they destroy, so a value that was not saved shows up as a wrong
//! answer. Random programs with loops, branches, calls, memory and permutations of registers run
//! with every register limit from none to all.

use super::mir::*;
use super::ralloc::{allocate, Stats};
use std::collections::HashMap;

const LI: u16 = 16; // def, imm
const ADD2: u16 = 17; // usedef, use
const ADD3: u16 = 18; // def, use, use
const MUL3: u16 = 19;
const SUB3: u16 = 20;
const LOAD: u16 = 21; // def, mem
const STORE: u16 = 22; // mem, use
const CALL: u16 = 23; // sym, use a0, use a1, def r0
const BLT: u16 = 24; // use, use, block
const JMP: u16 = 25; // block
const RET: u16 = 26; // use r0
const FADD3: u16 = 27;
const FLI: u16 = 28;
const FTOI: u16 = 29; // def int, use float
const ITOF: u16 = 30; // def float, use int
const MULE: u16 = 31; // def early, use, use: the result may not share a register with an operand

// Registers.
const R0: PReg = PReg(0);
const R1: PReg = PReg(1);
const A0: PReg = PReg(2);
const A1: PReg = PReg(3);
const T0: PReg = PReg(4);
const S0: PReg = PReg(5);
const S1: PReg = PReg(6);
const S2: PReg = PReg(7);
const R8: PReg = PReg(8);
const F0: PReg = PReg(9);
const F1: PReg = PReg(10);
const F2: PReg = PReg(11);
const F3: PReg = PReg(12);
const F4: PReg = PReg(13);
const F5: PReg = PReg(14);
const F6: PReg = PReg(15);
const NREGS: usize = 16;

struct Toy {
    regs: Vec<RegInfo>,
    int: Vec<PReg>,
    float: Vec<PReg>,
    int_scratch: Vec<PReg>,
    float_scratch: Vec<PReg>,
}

fn toy() -> Toy {
    let mk = |name, class, callee_saved| RegInfo {
        name,
        class,
        callee_saved,
    };
    let regs = vec![
        mk("r0", Class::Int, false),
        mk("r1", Class::Int, false),
        mk("a0", Class::Int, false),
        mk("a1", Class::Int, false),
        mk("t0", Class::Int, false),
        mk("s0", Class::Int, true),
        mk("s1", Class::Int, true),
        mk("s2", Class::Int, true),
        mk("r8", Class::Int, false),
        mk("f0", Class::Float, false),
        mk("f1", Class::Float, false),
        mk("f2", Class::Float, false),
        mk("f3", Class::Float, false),
        mk("f4", Class::Float, false),
        mk("f5", Class::Float, true),
        mk("f6", Class::Float, false),
    ];
    assert_eq!(regs.len(), NREGS);
    Toy {
        regs,
        int: vec![A0, A1, T0, S0, S1, S2],
        float: vec![F2, F3, F4, F5],
        int_scratch: vec![R0, R1, R8],
        float_scratch: vec![F0, F1, F6],
    }
}

impl Target for Toy {
    fn regs(&self) -> &[RegInfo] {
        &self.regs
    }
    fn allocatable(&self, c: Class) -> &[PReg] {
        match c {
            Class::Int => &self.int,
            Class::Float => &self.float,
            Class::Vec => &[],
        }
    }
    fn scratch(&self, c: Class) -> &[PReg] {
        match c {
            Class::Int => &self.int_scratch,
            _ => &self.float_scratch,
        }
    }
    fn op_name(&self, op: u16) -> &'static str {
        match op {
            LI => "li",
            ADD2 => "add2",
            ADD3 => "add",
            MUL3 => "mul",
            SUB3 => "sub",
            LOAD => "load",
            STORE => "store",
            CALL => "call",
            BLT => "blt",
            JMP => "jmp",
            RET => "ret",
            FADD3 => "fadd",
            FLI => "fli",
            FTOI => "ftoi",
            ITOF => "itof",
            MULE => "mule",
            _ => "?",
        }
    }
    fn mov(&self, _c: Class, dst: PReg, src: PReg) -> MInst {
        MInst::copy(Reg::Phys(dst), Operand::use_(Reg::Phys(src)))
    }
    fn mov_imm(&self, dst: PReg, imm: i64) -> MInst {
        MInst::new(LI, vec![Operand::def(Reg::Phys(dst)), Operand::Imm(imm)])
    }
    fn lea(&self, dst: PReg, f: FrameId, disp: i32) -> MInst {
        MInst::copy(Reg::Phys(dst), Operand::Frame(f, disp))
    }
    fn load(&self, _c: Class, dst: PReg, f: FrameId) -> MInst {
        MInst::new(
            LOAD,
            vec![
                Operand::def(Reg::Phys(dst)),
                Operand::Mem {
                    base: MemBase::Frame(f),
                    index: None,
                    disp: 0,
                },
            ],
        )
    }
    fn store(&self, _c: Class, f: FrameId, src: PReg) -> MInst {
        MInst::new(
            STORE,
            vec![
                Operand::Mem {
                    base: MemBase::Frame(f),
                    index: None,
                    disp: 0,
                },
                Operand::use_(Reg::Phys(src)),
            ],
        )
    }
}

fn caller_saved() -> RegSet {
    RegSet::of(&[R0, R1, A0, A1, T0, R8, F0, F1, F2, F3, F4, F6])
}

// ---- An interpreter ----------------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    ret: i64,
    log: Vec<(u32, i64, i64)>,
    heap: Vec<i64>,
}

fn run(f: &MFunc, allocated: bool) -> Result<Outcome, String> {
    let mut phys = [0i64; NREGS];
    let mut virt = vec![0i64; f.vclass.len()];
    let mut mem: HashMap<(u32, i64), i64> = HashMap::new();
    let mut heap = vec![0i64; 64];
    let mut log = Vec::new();
    let mut scramble = 0x5ca1_ab1e_i64;
    let (mut bi, mut k) = (0usize, 0usize);
    for _ in 0..400_000 {
        if k >= f.blocks[bi].insts.len() {
            bi += 1;
            k = 0;
            if bi >= f.blocks.len() {
                return Err("fell off the end".into());
            }
            continue;
        }
        let inst = &f.blocks[bi].insts[k];
        k += 1;
        macro_rules! rd {
            ($o:expr) => {
                match $o {
                    Operand::Reg(Reg::Phys(p), _) => phys[p.0 as usize],
                    Operand::Reg(Reg::Virt(v), _) => {
                        if allocated {
                            return Err("a virtual register is left".into());
                        }
                        virt[*v as usize]
                    }
                    Operand::Imm(n) => *n,
                    Operand::Frame(id, d) => 0x1000 + id.0 as i64 * 0x100 + *d as i64,
                    o => return Err(format!("read of {o:?}")),
                }
            };
        }
        macro_rules! wr {
            ($o:expr, $val:expr) => {{
                let val = $val;
                match $o {
                    Operand::Reg(Reg::Phys(p), _) => phys[p.0 as usize] = val,
                    Operand::Reg(Reg::Virt(v), _) => virt[*v as usize] = val,
                    o => return Err(format!("write of {o:?}")),
                }
            }};
        }
        let addr = |o: &Operand, phys: &[i64; NREGS], virt: &[i64]| -> (u32, i64, bool) {
            // (frame id or u32::MAX for the heap, offset, is_heap)
            let rv = |r: &Reg| match r {
                Reg::Phys(p) => phys[p.0 as usize],
                Reg::Virt(v) => virt[*v as usize],
            };
            match o {
                Operand::Mem {
                    base: MemBase::Frame(id),
                    index,
                    disp,
                } => {
                    let x = index.map_or(0, |(r, s)| rv(&r).wrapping_mul(s as i64));
                    (id.0, x + *disp as i64, false)
                }
                Operand::Mem {
                    base: MemBase::Reg(b),
                    index,
                    disp,
                } => {
                    let x = index.map_or(0, |(r, s)| rv(&r).wrapping_mul(s as i64));
                    (
                        u32::MAX,
                        rv(b).wrapping_add(x).wrapping_add(*disp as i64),
                        true,
                    )
                }
                _ => panic!("not a memory operand"),
            }
        };
        match inst.op {
            COPY => {
                let v = rd!(&inst.ops[1]);
                wr!(&inst.ops[0], v);
            }
            PARCOPY => {
                let n = inst.ops.len() / 2;
                let mut vals: Vec<i64> = Vec::new();
                for o in &inst.ops[n..] {
                    vals.push(rd!(o));
                }
                for (d, v) in inst.ops[..n].iter().zip(vals) {
                    wr!(d, v);
                }
            }
            LI | FLI => {
                let v = rd!(&inst.ops[1]);
                wr!(&inst.ops[0], v);
            }
            ADD2 => {
                let v = rd!(&inst.ops[0]).wrapping_add(rd!(&inst.ops[1]));
                wr!(&inst.ops[0], v);
            }
            ADD3 | FADD3 => {
                let v = rd!(&inst.ops[1]).wrapping_add(rd!(&inst.ops[2]));
                wr!(&inst.ops[0], v);
            }
            MUL3 | MULE => {
                let v = rd!(&inst.ops[1]).wrapping_mul(rd!(&inst.ops[2]));
                wr!(&inst.ops[0], v);
            }
            SUB3 => {
                let v = rd!(&inst.ops[1]).wrapping_sub(rd!(&inst.ops[2]));
                wr!(&inst.ops[0], v);
            }
            FTOI => {
                let v = rd!(&inst.ops[1]) ^ 0x55;
                wr!(&inst.ops[0], v);
            }
            ITOF => {
                let v = rd!(&inst.ops[1]).wrapping_add(7);
                wr!(&inst.ops[0], v);
            }
            LOAD => {
                let (id, off, heap_) = addr(&inst.ops[1], &phys, &virt);
                let v = if heap_ {
                    heap[off.rem_euclid(64) as usize]
                } else {
                    *mem.get(&(id, off)).unwrap_or(&0)
                };
                wr!(&inst.ops[0], v);
            }
            STORE => {
                let (id, off, heap_) = addr(&inst.ops[0], &phys, &virt);
                let v = rd!(&inst.ops[1]);
                if heap_ {
                    heap[off.rem_euclid(64) as usize] = v;
                } else {
                    mem.insert((id, off), v);
                }
            }
            CALL => {
                let Operand::Sym(n) = inst.ops[0] else {
                    return Err("call without a symbol".into());
                };
                let (a, b) = (phys[A0.0 as usize], phys[A1.0 as usize]);
                log.push((n, a, b));
                for r in inst.clobbers.iter() {
                    scramble = scramble
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    phys[r.0 as usize] = scramble;
                }
                phys[R0.0 as usize] = a.wrapping_mul(3).wrapping_add(b).wrapping_add(n as i64);
            }
            BLT => {
                if rd!(&inst.ops[0]) < rd!(&inst.ops[1]) {
                    let Operand::Block(t) = inst.ops[2] else {
                        return Err("blt target".into());
                    };
                    bi = t as usize;
                    k = 0;
                }
            }
            JMP => {
                let Operand::Block(t) = inst.ops[0] else {
                    return Err("jmp target".into());
                };
                bi = t as usize;
                k = 0;
            }
            RET => {
                return Ok(Outcome {
                    ret: phys[R0.0 as usize],
                    log,
                    heap,
                });
            }
            op => return Err(format!("unknown op {op}")),
        }
    }
    Err("did not stop".into())
}

// ---- Building programs -------------------------------------------------------------------------------

fn v(r: Reg) -> Operand {
    Operand::use_(r)
}
fn p(r: PReg) -> Reg {
    Reg::Phys(r)
}

fn call(n: u32) -> MInst {
    MInst::new(
        CALL,
        vec![
            Operand::Sym(n),
            Operand::use_(p(A0)),
            Operand::use_(p(A1)),
            Operand::def(p(R0)),
        ],
    )
    .call(caller_saved())
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

struct Gen {
    f: MFunc,
    rng: Rng,
    ints: Vec<Reg>,
    floats: Vec<Reg>,
    frame: FrameId,
    cur: u32,
    calls: u32,
}

impl Gen {
    fn int(&mut self) -> Reg {
        self.ints[self.rng.below(self.ints.len())]
    }
    fn flt(&mut self) -> Option<Reg> {
        if self.floats.is_empty() {
            None
        } else {
            Some(self.floats[self.rng.below(self.floats.len())])
        }
    }
    fn emit(&mut self, i: MInst) {
        let b = self.cur;
        self.f.push(b, i);
    }
    fn new_block(&mut self) -> u32 {
        let b = self.f.new_block();
        self.cur = b;
        b
    }

    fn stmt(&mut self, depth: u32) {
        let (a, b, c) = (self.int(), self.int(), self.int());
        match self.rng.below(if depth < 2 { 13 } else { 10 }) {
            0 => self.emit(MInst::new(ADD3, vec![Operand::def(a), v(b), v(c)])),
            1 => self.emit(MInst::new(MUL3, vec![Operand::def(a), v(b), v(c)])),
            2 => self.emit(MInst::new(SUB3, vec![Operand::def(a), v(b), v(c)])),
            3 => self.emit(MInst::new(ADD2, vec![Operand::use_def(a), v(b)])),
            4 => {
                let n = self.rng.below(100) as i64 - 20;
                self.emit(MInst::new(LI, vec![Operand::def(a), Operand::Imm(n)]));
            }
            5 => {
                let m = Operand::Mem {
                    base: MemBase::Frame(self.frame),
                    index: None,
                    disp: 8 * self.rng.below(8) as i32,
                };
                if self.rng.below(2) == 0 {
                    self.emit(MInst::new(STORE, vec![m, v(a)]));
                } else {
                    self.emit(MInst::new(LOAD, vec![Operand::def(a), m]));
                }
            }
            6 => {
                // The heap: the base is a small value, so the word index stays in range.
                let m = Operand::Mem {
                    base: MemBase::Reg(b),
                    index: Some((c, 1)),
                    disp: self.rng.below(5) as i32,
                };
                if self.rng.below(2) == 0 {
                    self.emit(MInst::new(STORE, vec![m, v(a)]));
                } else {
                    self.emit(MInst::new(LOAD, vec![Operand::def(a), m]));
                }
            }
            7 => {
                let n = self.calls;
                self.calls += 1;
                let src1 = if self.rng.below(3) == 0 {
                    Operand::Imm(self.rng.below(9) as i64)
                } else {
                    v(b)
                };
                self.emit(MInst::parcopy(vec![(p(A0), v(a)), (p(A1), src1)]));
                self.emit(call(n));
                if self.rng.below(2) == 0 {
                    self.emit(MInst::copy(c, v(p(R0))));
                } else {
                    // The result register next to a value that may be in memory: the reload must not use `r0`.
                    self.emit(MInst::new(ADD3, vec![Operand::def(c), v(p(R0)), v(b)]));
                }
            }
            8 => {
                // A permutation of up to four registers (swaps make cycles), maybe with a constant.
                let mut regs = vec![a, b, c, self.int()];
                regs.sort();
                regs.dedup();
                let mut srcs: Vec<Operand> = regs.iter().map(|r| v(*r)).collect();
                let k = self.rng.below(srcs.len());
                srcs.rotate_left(k);
                if self.rng.below(4) == 0 {
                    srcs[0] = Operand::Imm(5);
                }
                self.emit(MInst::parcopy(regs.into_iter().zip(srcs).collect()));
            }
            9 => match (self.flt(), self.flt(), self.flt()) {
                (Some(x), Some(y), Some(z)) => match self.rng.below(4) {
                    0 => self.emit(MInst::new(FADD3, vec![Operand::def(x), v(y), v(z)])),
                    1 => self.emit(MInst::new(FTOI, vec![Operand::def(a), v(x)])),
                    2 => self.emit(MInst::new(ITOF, vec![Operand::def(x), v(a)])),
                    _ => self.emit(MInst::new(
                        MULE,
                        vec![Operand::Reg(a, Role::EarlyDef), v(b), v(c)],
                    )),
                },
                _ => self.emit(MInst::new(
                    MULE,
                    vec![Operand::Reg(a, Role::EarlyDef), v(b), v(c)],
                )),
            },
            10 => {
                // if (b < c) { ... } else { ... }
                let blt = {
                    let cur = self.cur;
                    self.f.push(
                        cur,
                        MInst::new(BLT, vec![v(b), v(c), Operand::Block(0)]).branch(),
                    );
                    self.f.blocks[cur as usize].insts.len() - 1
                };
                let head = self.cur;
                self.new_block();
                self.body(depth + 1);
                let jmp = {
                    let cur = self.cur;
                    self.f.push(
                        cur,
                        MInst::new(JMP, vec![Operand::Block(0)]).branch().nofall(),
                    );
                    (cur, self.f.blocks[cur as usize].insts.len() - 1)
                };
                let els = self.new_block();
                self.body(depth + 1);
                let end = self.new_block();
                self.f.blocks[head as usize].insts[blt].ops[2] = Operand::Block(els);
                self.f.blocks[jmp.0 as usize].insts[jmp.1].ops[0] = Operand::Block(end);
            }
            _ => {
                // A loop that runs a few times: counter, bound and step are registers of their own.
                let counter = self.f.new_vreg(Class::Int);
                let bound = self.f.new_vreg(Class::Int);
                let one = self.f.new_vreg(Class::Int);
                let n = 2 + self.rng.below(3) as i64;
                self.emit(MInst::new(LI, vec![Operand::def(counter), Operand::Imm(0)]));
                self.emit(MInst::new(LI, vec![Operand::def(bound), Operand::Imm(n)]));
                self.emit(MInst::new(LI, vec![Operand::def(one), Operand::Imm(1)]));
                let body = self.new_block();
                self.body(depth + 1);
                self.emit(MInst::new(
                    ADD3,
                    vec![Operand::def(counter), v(counter), v(one)],
                ));
                self.emit(
                    MInst::new(BLT, vec![v(counter), v(bound), Operand::Block(body)]).branch(),
                );
                self.new_block();
            }
        }
    }

    fn body(&mut self, depth: u32) {
        for _ in 0..1 + self.rng.below(5) {
            self.stmt(depth);
        }
    }
}

fn random_program(seed: u64) -> MFunc {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    for _ in 0..4 {
        rng.next();
    }
    let nint = 2 + rng.below(18);
    let nflt = rng.below(5);
    let mut f = MFunc {
        name: format!("random{seed}"),
        ..MFunc::default()
    };
    let frame = f.new_frame(64, 8, FrameKind::Slot);
    let ints: Vec<Reg> = (0..nint).map(|_| f.new_vreg(Class::Int)).collect();
    let floats: Vec<Reg> = (0..nflt).map(|_| f.new_vreg(Class::Float)).collect();
    let entry = f.new_block();
    let mut g = Gen {
        f,
        rng,
        ints: ints.clone(),
        floats: floats.clone(),
        frame,
        cur: entry,
        calls: 0,
    };
    for (k, r) in ints.iter().enumerate() {
        let n = g.rng.below(7) as i64 + k as i64;
        g.emit(MInst::new(LI, vec![Operand::def(*r), Operand::Imm(n)]));
    }
    for r in &floats {
        let n = g.rng.below(9) as i64;
        g.emit(MInst::new(FLI, vec![Operand::def(*r), Operand::Imm(n)]));
    }
    for _ in 0..3 + g.rng.below(8) {
        g.stmt(0);
    }
    // The answer: every register folded into one.
    let acc = g.f.new_vreg(Class::Int);
    g.emit(MInst::new(LI, vec![Operand::def(acc), Operand::Imm(0)]));
    for r in ints {
        g.emit(MInst::new(ADD3, vec![Operand::def(acc), v(acc), v(r)]));
    }
    for r in floats {
        let t = g.f.new_vreg(Class::Int);
        g.emit(MInst::new(FTOI, vec![Operand::def(t), v(r)]));
        g.emit(MInst::new(ADD3, vec![Operand::def(acc), v(acc), v(t)]));
    }
    g.emit(MInst::copy(p(R0), v(acc)));
    g.emit(MInst::new(RET, vec![Operand::use_(p(R0))]).nofall());
    g.f
}

// ---- Tests -------------------------------------------------------------------------------------------

#[test]
fn random_programs_agree_with_every_register_limit() {
    let t = toy();
    let mut total = Stats::default();
    for seed in 1..=300u64 {
        let f = random_program(seed);
        verify(&f, &t).unwrap_or_else(|e| panic!("seed {seed}: {e}\n{}", f.print(&t)));
        let want = run(&f, false).unwrap_or_else(|e| panic!("seed {seed}: {e}\n{}", f.print(&t)));
        for limit in [usize::MAX, 6, 4, 3, 2, 1, 0] {
            let mut g = f.clone();
            let stats = allocate(&mut g, &t, limit);
            let got = run(&g, true)
                .unwrap_or_else(|e| panic!("seed {seed} limit {limit}: {e}\n{}", g.print(&t)));
            assert_eq!(
                got,
                want,
                "seed {seed} limit {limit}\nbefore:\n{}\nafter:\n{}",
                f.print(&t),
                g.print(&t)
            );
            for r in g.used_callee_saved.iter() {
                assert!(t.regs[r.0 as usize].callee_saved);
            }
            if limit == usize::MAX {
                total.spilled += stats.spilled;
                total.split_around_calls += stats.split_around_calls;
                total.copies_removed += stats.copies_removed;
            }
        }
    }
    // The programs are big enough to need every path.
    assert!(total.spilled > 0, "{total:?}");
    assert!(total.split_around_calls > 0, "{total:?}");
    assert!(total.copies_removed > 0, "{total:?}");
}

#[test]
fn a_value_that_lives_across_a_call_is_saved_or_kept_in_a_callee_saved_register() {
    // x = 5; call; return x + 1
    let t = toy();
    let mut f = MFunc {
        name: "across".into(),
        ..MFunc::default()
    };
    let x = f.new_vreg(Class::Int);
    let y = f.new_vreg(Class::Int);
    let b = f.new_block();
    f.push(b, MInst::new(LI, vec![Operand::def(x), Operand::Imm(5)]));
    f.push(
        b,
        MInst::parcopy(vec![(p(A0), Operand::Imm(1)), (p(A1), Operand::Imm(2))]),
    );
    f.push(b, call(9));
    f.push(
        b,
        MInst::new(ADD3, vec![Operand::def(y), v(x), Operand::Imm(1)]),
    );
    f.push(b, MInst::copy(p(R0), v(y)));
    f.push(b, MInst::new(RET, vec![Operand::use_(p(R0))]).nofall());
    verify(&f, &t).unwrap();
    let want = run(&f, false).unwrap();
    assert_eq!(want.ret, 6);
    // Only caller-saved registers: the value is split around the call.
    let mut g = f.clone();
    let stats = allocate(&mut g, &t, 3);
    assert_eq!(
        stats.split_around_calls + stats.spilled,
        1,
        "{}",
        g.print(&t)
    );
    assert_eq!(run(&g, true).unwrap(), want);
    // With a callee-saved register available nothing is stored around the call.
    let mut g = f.clone();
    let stats = allocate(&mut g, &t, usize::MAX);
    assert_eq!(
        stats,
        Stats {
            spilled: 0,
            split_around_calls: 0,
            copies_removed: stats.copies_removed
        }
    );
    assert!(
        g.used_callee_saved.contains(S0)
            || g.used_callee_saved.contains(S1)
            || g.used_callee_saved.contains(S2)
    );
    assert_eq!(run(&g, true).unwrap(), want);
}

#[test]
fn a_swap_is_a_cycle_through_the_scratch_register() {
    // a, b = b, a, in registers and with everything in memory.
    let t = toy();
    let mut f = MFunc {
        name: "swap".into(),
        ..MFunc::default()
    };
    let (a, b, d) = (
        f.new_vreg(Class::Int),
        f.new_vreg(Class::Int),
        f.new_vreg(Class::Int),
    );
    let bl = f.new_block();
    f.push(bl, MInst::new(LI, vec![Operand::def(a), Operand::Imm(3)]));
    f.push(bl, MInst::new(LI, vec![Operand::def(b), Operand::Imm(10)]));
    f.push(bl, MInst::parcopy(vec![(a, v(b)), (b, v(a))]));
    f.push(bl, MInst::new(SUB3, vec![Operand::def(d), v(a), v(b)]));
    f.push(bl, MInst::copy(p(R0), v(d)));
    f.push(bl, MInst::new(RET, vec![Operand::use_(p(R0))]).nofall());
    let want = run(&f, false).unwrap();
    assert_eq!(want.ret, 7);
    for limit in [usize::MAX, 1, 0] {
        let mut g = f.clone();
        allocate(&mut g, &t, limit);
        assert_eq!(
            run(&g, true).unwrap(),
            want,
            "limit {limit}\n{}",
            g.print(&t)
        );
    }
}

#[test]
fn an_early_def_does_not_share_a_register_with_an_operand_that_dies() {
    let t = toy();
    let mut f = MFunc {
        name: "early".into(),
        ..MFunc::default()
    };
    let (a, b, d) = (
        f.new_vreg(Class::Int),
        f.new_vreg(Class::Int),
        f.new_vreg(Class::Int),
    );
    let bl = f.new_block();
    f.push(bl, MInst::new(LI, vec![Operand::def(a), Operand::Imm(3)]));
    f.push(bl, MInst::new(LI, vec![Operand::def(b), Operand::Imm(4)]));
    f.push(
        bl,
        MInst::new(MULE, vec![Operand::Reg(d, Role::EarlyDef), v(a), v(b)]),
    );
    f.push(bl, MInst::copy(p(R0), v(d)));
    f.push(bl, MInst::new(RET, vec![Operand::use_(p(R0))]).nofall());
    let mut g = f.clone();
    allocate(&mut g, &t, usize::MAX);
    let mule = g.blocks[0].insts.iter().find(|i| i.op == MULE).unwrap();
    let regs: Vec<&Operand> = mule.ops.iter().collect();
    let reg = |o: &Operand| match o {
        Operand::Reg(r, _) => *r,
        o => panic!("{o:?}"),
    };
    assert_ne!(reg(regs[0]), reg(regs[1]), "{}", g.print(&t));
    assert_ne!(reg(regs[0]), reg(regs[2]), "{}", g.print(&t));
    // The ordinary `Def` of the same shape reuses an operand's register: no copy, no new register.
    let mut h = f.clone();
    h.blocks[0].insts[2].ops[0] = Operand::def(d);
    allocate(&mut h, &t, usize::MAX);
    let mul = h.blocks[0].insts.iter().find(|i| i.op == MULE).unwrap();
    let reg = |o: &Operand| match o {
        Operand::Reg(r, _) => *r,
        o => panic!("{o:?}"),
    };
    assert!(
        reg(&mul.ops[0]) == reg(&mul.ops[1]) || reg(&mul.ops[0]) == reg(&mul.ops[2]),
        "{}",
        h.print(&t)
    );
}

#[test]
fn copies_between_registers_that_agree_disappear() {
    let t = toy();
    let mut f = MFunc {
        name: "copies".into(),
        ..MFunc::default()
    };
    let (a, b) = (f.new_vreg(Class::Int), f.new_vreg(Class::Int));
    let bl = f.new_block();
    f.push(bl, MInst::new(LI, vec![Operand::def(a), Operand::Imm(3)]));
    f.push(bl, MInst::copy(b, v(a)));
    f.push(bl, MInst::copy(p(R0), v(b)));
    f.push(bl, MInst::new(RET, vec![Operand::use_(p(R0))]).nofall());
    let mut g = f.clone();
    let stats = allocate(&mut g, &t, usize::MAX);
    assert_eq!(stats.copies_removed, 1, "{}", g.print(&t));
    assert_eq!(run(&g, true).unwrap().ret, 3);
}

#[test]
fn the_verifier_names_what_is_wrong() {
    let t = toy();
    // A register read before any write.
    let mut f = MFunc {
        name: "undef".into(),
        ..MFunc::default()
    };
    let a = f.new_vreg(Class::Int);
    let b = f.new_block();
    f.push(b, MInst::copy(p(R0), v(a)));
    f.push(b, MInst::new(RET, vec![Operand::use_(p(R0))]).nofall());
    assert!(verify(&f, &t).unwrap_err().contains("never written"));
    // The last block falls off the end.
    let mut f = MFunc {
        name: "off".into(),
        ..MFunc::default()
    };
    let b = f.new_block();
    f.push(
        b,
        MInst::new(LI, vec![Operand::def(p(R0)), Operand::Imm(1)]),
    );
    assert!(verify(&f, &t).unwrap_err().contains("falls off"));
    // Something after a jump.
    let mut f = MFunc {
        name: "after".into(),
        ..MFunc::default()
    };
    let b = f.new_block();
    f.push(
        b,
        MInst::new(JMP, vec![Operand::Block(0)]).branch().nofall(),
    );
    f.push(b, MInst::new(RET, vec![Operand::use_(p(R0))]).nofall());
    assert!(verify(&f, &t).is_err());
    // A copy between classes.
    let mut f = MFunc {
        name: "classes".into(),
        ..MFunc::default()
    };
    let (a, x) = (f.new_vreg(Class::Int), f.new_vreg(Class::Float));
    let b = f.new_block();
    f.push(b, MInst::new(LI, vec![Operand::def(a), Operand::Imm(1)]));
    f.push(b, MInst::copy(x, v(a)));
    f.push(b, MInst::new(RET, vec![Operand::use_(p(R0))]).nofall());
    assert!(verify(&f, &t).unwrap_err().contains("between classes"));
}

#[test]
fn the_printer_shows_roles_memory_and_clobbers() {
    let t = toy();
    let mut f = MFunc {
        name: "show".into(),
        ..MFunc::default()
    };
    let fr = f.new_frame(16, 8, FrameKind::Slot);
    let (a, x) = (f.new_vreg(Class::Int), f.new_vreg(Class::Float));
    let b = f.new_block();
    f.push(b, MInst::new(LI, vec![Operand::def(a), Operand::Imm(-2)]));
    f.push(
        b,
        MInst::new(ADD2, vec![Operand::use_def(a), Operand::Imm(1)]),
    );
    f.push(
        b,
        MInst::new(
            LOAD,
            vec![
                Operand::def(x),
                Operand::Mem {
                    base: MemBase::Frame(fr),
                    index: Some((a, 8)),
                    disp: 4,
                },
            ],
        ),
    );
    f.push(b, call(3));
    f.push(b, MInst::new(RET, vec![Operand::use_(p(R0))]).nofall());
    let text = f.print(&t);
    assert_eq!(
        text,
        "mfunc show:\n  frame0: 16 bytes align 8 Slot\nb0:\n  li %0, #-2\n  add2 %0!, #1\n  load %f1, [frame0+%0*8+4]\n  call @3, a0, a1, r0  ; clobbers r0 r1 a0 a1 t0 r8 f0 f1 f2 f3 f4 f6\n  ret r0\n"
    );
}
