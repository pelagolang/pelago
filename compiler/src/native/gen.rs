//! The x86-64 code generator driver: it lays out the runtime, the data and the stop routines, then hands
//! each function to the machine IR (`x86_select.rs`, `ralloc.rs`, `x86_emit.rs`; see
//! `docs/design/mir.md`) and writes the executable. The rules for division, shifts, overflow and NaN are
//! in `docs/design/backend.md`.

use super::elf::{self, Blob, Symbol};
use super::rt::{self, Runtime};
use super::x86::*;
use super::{pe, Os};
use crate::ir::{Func, IrTy, Module};

const INT_REGS: [Reg; 6] = [RDI, RSI, RDX, RCX, R8, R9];
pub(super) const MAX_FLOAT_REGS: usize = 8;

// Tests can shrink the register set (the number of registers of each kind the allocator may use) to
// push the allocator through its spilling and splitting paths with ordinary programs.
#[cfg(test)]
thread_local! {
    pub static REG_LIMIT: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
}

pub(super) fn reg_limit() -> usize {
    #[cfg(test)]
    {
        REG_LIMIT.with(|l| l.get())
    }
    #[cfg(not(test))]
    {
        // For debugging the allocators: `PELAGO_REG_LIMIT=2` keeps two registers of each kind.
        static LIMIT: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        *LIMIT.get_or_init(|| {
            std::env::var("PELAGO_REG_LIMIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(usize::MAX)
        })
    }
}

/// What to build.
pub struct Options {
    /// Report leaked blocks at exit (status 3): for tests.
    pub leak_check: bool,
    pub target: Os,
    /// Keep the section headers and the symbol table (for gdb, objdump, callgrind and `perf`).
    pub symbols: bool,
}

/// A loop that stores at least this many bytes of one value is a `rep stos`: it starts slowly.
pub(super) const FILL_BYTES: i128 = 512;

fn is_float(t: IrTy) -> bool {
    matches!(t, IrTy::F32 | IrTy::F64)
}

/// Where an argument or parameter travels.
pub(super) enum Loc {
    Int(usize),
    Float(usize),
    Stack(usize),
}

pub(super) fn classify(tys: &[IrTy]) -> Vec<Loc> {
    let (mut i, mut f, mut s) = (0, 0, 0);
    tys.iter()
        .map(|t| {
            if is_float(*t) {
                if f < MAX_FLOAT_REGS {
                    f += 1;
                    return Loc::Float(f - 1);
                }
            } else if i < INT_REGS.len() {
                i += 1;
                return Loc::Int(i - 1);
            }
            s += 1;
            Loc::Stack(s - 1)
        })
        .collect()
}

/// Compiles the module to an executable whose entry runs the function named `entry`.
pub fn compile(m: &Module, entry: &str, opts: &Options) -> Result<Vec<u8>, String> {
    if !matches!(opts.target, super::Os::Linux | super::Os::Windows) && super::uses_process(m) {
        return Err(super::PROCESS_REFUSED.to_string());
    }
    let main_fn = m
        .funcs
        .iter()
        .position(|f| f.name == entry)
        .ok_or_else(|| format!("no function `{entry}`"))?;
    let mut g = Gen::new(m);
    let main_label = g.funcs[main_fn];
    let rt = rt::emit_for(
        &mut g.a,
        &mut g.ro,
        &mut g.data,
        main_label,
        opts.leak_check,
        opts.target,
        super::Uses::of(m),
        &used_files(m),
    );
    g.rt = Some(rt);
    let bss = g.place_data(opts.target == Os::Linux);
    // Callees are selected and allocated first, so that a call can be told what its callee writes;
    // the code is laid out in the order of the module.
    let mut sets: Vec<Option<super::mir::RegSet>> = vec![None; m.funcs.len()];
    let mut mfuncs: Vec<Option<super::mir::MFunc>> = (0..m.funcs.len()).map(|_| None).collect();
    for i in super::clobber::order(m) {
        let (mf, w) = g.prepare(&m.funcs[i], &sets)?;
        sets[i] = Some(w);
        mfuncs[i] = Some(mf);
    }
    let mut syms = Vec::new();
    for (i, f) in m.funcs.iter().enumerate() {
        // A function starts 16 bytes before a cache line: where the hot code of a function falls
        // in its lines (and so what a call and return cost) must not depend on how big the runtime
        // in front of it is. Measured on `fib` and `tak`, a start 48 bytes into a line is the best
        // of the four (the worst of them is 25 percent slower on `fib`).
        g.a.align(64, 63);
        g.a.nops(48);
        let start = g.a.here();
        g.function(i, mfuncs[i].take().expect("every function is prepared"));
        syms.push(Symbol {
            name: f.name.clone(),
            offset: start as u64,
            size: (g.a.here() - start) as u64,
        });
    }
    // After the functions, so only the stubs that a check jumps to are emitted.
    g.stop_stubs();
    match opts.target {
        Os::Linux => elf::write(&g.a, &g.ro, &g.data, bss, opts.symbols.then_some(&syms[..])),
        Os::Windows => pe::write(&g.a, &g.ro, &g.data, 0),
        Os::MacOs => Err("macOS programs are built for AArch64 only (use --arch=aarch64)".into()),
    }
}

struct Gen<'m> {
    m: &'m Module,
    a: Asm,
    ro: Blob,
    data: Blob,
    funcs: Vec<Label>,
    data_off: Vec<u32>,
    rt: Option<Runtime>,
    stop_div0: Label,
    stop_shift: Label,
    stop_overflow: Label,
    stop_unreachable: Label,
    /// The entries of those routines that take the place in `rdx`, by `SYM_*` number.
    stop_at: [Label; 4],
    traps: super::x86_emit::Traps,
}

impl<'m> Gen<'m> {
    fn new(m: &'m Module) -> Gen<'m> {
        let mut a = Asm::new();
        a.pad_jumps = true;
        let funcs = m.funcs.iter().map(|_| a.new_label()).collect();
        let (d, s, o, u) = (a.new_label(), a.new_label(), a.new_label(), a.new_label());
        let stop_at = [(); 4].map(|_| a.new_label());
        Gen {
            m,
            a,
            ro: Blob::default(),
            data: Blob::default(),
            funcs,
            data_off: Vec::new(),
            rt: None,
            stop_div0: d,
            stop_shift: s,
            stop_overflow: o,
            stop_unreachable: u,
            stop_at,
            traps: Default::default(),
        }
    }

    fn rt(&self) -> &Runtime {
        self.rt.as_ref().expect("the runtime is emitted first")
    }

    /// All static data goes into `.data` (the lowering writes to some tables at run time).
    fn place_data(&mut self, bss: bool) -> usize {
        let (offs, zeroed) = super::place_module_data(&self.m.data, &mut self.data, bss);
        self.data_off = offs;
        zeroed
    }

    /// The shared exits for the checks that stop the program.
    fn stop_stubs(&mut self) {
        // The stubs that say where first: each loads its place and joins the shared routine at its `_at` entry.
        let places = std::mem::take(&mut self.traps.order);
        for (sym, loc, label) in places {
            self.a.bind(label);
            self.a.mov_ri(RDX, i64::from(loc));
            let at = self.stop_at[sym as usize];
            self.a.jmp(at);
        }
        // Only reached by `jcc`; a stub that no check names is left out, with its message.
        for (i, (label, msg)) in [
            (self.stop_div0, "division by zero"),
            (self.stop_shift, "shifted by the width or more"),
            (self.stop_overflow, "arithmetic overflow"),
            (self.stop_unreachable, "reached unreachable code"),
        ]
        .into_iter()
        .enumerate()
        {
            let at = self.stop_at[i];
            let by_place = self.a.is_referenced(at);
            if !self.a.is_referenced(label) && !by_place {
                continue;
            }
            let off = self.ro.add(msg.as_bytes(), 1);
            if self.a.is_referenced(label) {
                // A check with no place: the place is none.
                self.a.bind(label);
                self.a.alu_rr(Alu::Xor, RDX, RDX, 4);
            }
            self.a.bind(at);
            self.a.lea_sect(RDI, Sect::Rodata, off);
            self.a.mov_ri(RSI, msg.len() as i64);
            let stop = self.rt().stop_at;
            self.a.call(stop);
        }
    }

    /// The `Sym` numbers of the module's rarely called functions (see `MFunc::cold_syms`).
    pub(super) fn cold_syms(&self, base: u32) -> Vec<u32> {
        (0..self.m.funcs.len() as u32)
            .filter(|i| crate::ir::opt::is_cold(&self.m.funcs[*i as usize].name))
            .map(|i| base + i)
            .collect()
    }

    /// Selects and allocates `f`; also gives the registers a call of it destroys.
    fn prepare(
        &mut self,
        f: &Func,
        sets: &[Option<super::mir::RegSet>],
    ) -> Result<(super::mir::MFunc, super::mir::RegSet), String> {
        let Some(mut mf) = super::x86_select::select(f) else {
            return Err(format!(
                "`{}` uses a value the x86-64 selector does not handle",
                f.name
            ));
        };
        mf.cold_syms = self.cold_syms(super::x86_mir::SYM_FUNC);
        let target = super::x86_mir::X86::new();
        super::clobber::narrow_calls(
            &mut mf,
            &target,
            super::x86_mir::op::CALL,
            super::x86_mir::SYM_FUNC,
            sets,
        );
        if cfg!(debug_assertions) {
            super::mir::verify(&mf, &target)?;
        }
        let dump = std::env::var("PELAGO_MIR_DUMP").is_ok_and(|v| v == "1");
        if dump {
            eprintln!("{}", mf.print(&target));
        }
        super::ralloc::allocate(&mut mf, &target, reg_limit());
        if dump {
            eprintln!("after allocation:\n{}", mf.print(&target));
        }
        // The probe of a big frame writes `r10` and `r11` without saying so.
        let hidden = super::mir::RegSet::of(&[super::x86_mir::R10, super::x86_mir::R11]);
        let w = super::clobber::written(&mf, &target, hidden);
        Ok((mf, w))
    }

    fn function(&mut self, fi: usize, mf: super::mir::MFunc) {
        let label = self.funcs[fi];
        let mut env = super::x86_emit::Env {
            a: &mut self.a,
            ro: &mut self.ro,
            funcs: &self.funcs,
            data_off: &self.data_off,
            stops: [
                self.stop_div0,
                self.stop_shift,
                self.stop_overflow,
                self.stop_unreachable,
            ],
            rt: self.rt.as_ref().expect("the runtime is emitted first"),
            traps: &mut self.traps,
        };
        super::x86_emit::emit(&mut env, &mf, label);
    }
}

/// The names of the source files that a stop of the module can name, in file-number order; an empty name for a
/// file none of them names (the runtime keeps no text for it).
pub(super) fn used_files(m: &Module) -> Vec<String> {
    use crate::ir::{BinOp, Op, Term};
    let mut used = vec![false; m.files.len()];
    let mut mark = |loc: crate::span::Span| {
        if let Some((f, _)) = loc.place() {
            if let Some(u) = used.get_mut(f as usize - 1) {
                *u = true;
            }
        }
    };
    for f in &m.funcs {
        for i in &f.insts {
            let traps = match &i.op {
                Op::Checked(..) | Op::CheckedIf(..) => true,
                Op::Bin(o, ..) => matches!(
                    o,
                    BinOp::Div
                        | BinOp::Rem
                        | BinOp::UDiv
                        | BinOp::URem
                        | BinOp::Shl
                        | BinOp::Shr
                        | BinOp::UShr
                ),
                _ => false,
            };
            if traps {
                mark(i.span);
            }
        }
        for b in &f.blocks {
            if let Some(Term::Stop(_, at)) = &b.term {
                mark(*at);
            }
        }
    }
    m.files
        .iter()
        .zip(used)
        .map(|(n, u)| if u { n.clone() } else { String::new() })
        .collect()
}
