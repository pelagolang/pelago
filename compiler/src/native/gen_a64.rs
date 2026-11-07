//! The AArch64 code generator driver: it lays out the runtime and the data, then hands each function to
//! the machine IR (`a64_select.rs`, `ralloc.rs`, `a64_emit.rs`; see `docs/design/mir.md`) and writes the
//! executable. The rules for division, shifts, overflow and NaN are in `docs/design/backend.md` and are
//! the same as on x86-64.

use super::a64::*;
use super::elf::{self, Blob, Symbol};
use super::gen::Options;
use super::rt_a64::{self, Runtime};
use super::Os;
use crate::ir::{Func, IrTy, Module};

const INT_REGS: [Reg; 8] = [X0, X1, X2, X3, X4, X5, X6, X7];
const MAX_FLOAT_REGS: usize = 8;

/// Compiles the module to an AArch64 Linux executable whose entry runs the function named `entry`.
pub fn compile(m: &Module, entry: &str, opts: &Options) -> Result<Vec<u8>, String> {
    if super::uses_process(m) {
        return Err(super::PROCESS_REFUSED.to_string());
    }
    let main_fn = m
        .funcs
        .iter()
        .position(|f| f.name == entry)
        .ok_or_else(|| format!("no function `{entry}`"))?;
    let mut g = Gen::new(m);
    let main_label = g.funcs[main_fn];
    let rt = rt_a64::emit_for(
        &mut g.a,
        &mut g.ro,
        &mut g.data,
        main_label,
        opts.leak_check,
        opts.target,
        super::Uses::of(m),
    );
    g.rt = Some(rt);
    let (offs, bss) = super::place_module_data(&m.data, &mut g.data, opts.target == Os::Linux);
    g.data_off = offs;
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
        g.a.align(16);
        let start = g.a.here();
        g.function(i, mfuncs[i].take().expect("every function is prepared"));
        syms.push(Symbol {
            name: f.name.clone(),
            offset: start as u64,
            size: (g.a.here() - start) as u64,
        });
    }
    match opts.target {
        Os::Linux => elf::write_a64(&g.a, &g.ro, &g.data, bss, &syms),
        Os::Windows => super::pe::write_a64(&g.a, &g.ro, &g.data, 0),
        Os::MacOs => super::macho::write_a64(&g.a, &g.ro, &g.data, 0),
    }
}

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

struct Gen<'m> {
    m: &'m Module,
    a: Asm,
    ro: Blob,
    data: Blob,
    funcs: Vec<Label>,
    data_off: Vec<u32>,
    rt: Option<Runtime>,
}

impl<'m> Gen<'m> {
    fn new(m: &'m Module) -> Gen<'m> {
        let mut a = Asm::new();
        let funcs = m.funcs.iter().map(|_| a.new_label()).collect();
        Gen {
            m,
            a,
            ro: Blob::default(),
            data: Blob::default(),
            funcs,
            data_off: Vec::new(),
            rt: None,
        }
    }

    /// Selects and allocates `f`; also gives the registers a call of it destroys.
    fn prepare(
        &mut self,
        f: &Func,
        sets: &[Option<super::mir::RegSet>],
    ) -> Result<(super::mir::MFunc, super::mir::RegSet), String> {
        let Some(mut mf) = super::a64_select::select(f) else {
            return Err(format!(
                "`{}` uses a value the AArch64 selector does not handle",
                f.name
            ));
        };
        mf.cold_syms = (0..self.m.funcs.len() as u32)
            .filter(|i| {
                self.m.funcs[*i as usize]
                    .name
                    .ends_with(crate::ir::opt::COLD_SUFFIX)
            })
            .map(|i| super::a64_mir::SYM_FUNC + i)
            .collect();
        let target = super::a64_mir::A64::new();
        super::clobber::narrow_calls(
            &mut mf,
            &target,
            super::a64_mir::op::CALL,
            super::a64_mir::SYM_FUNC,
            sets,
        );
        if cfg!(debug_assertions) {
            super::mir::verify(&mf, &target)?;
        }
        let dump = std::env::var("PELAGO_MIR_DUMP").is_ok_and(|v| v == "1");
        if dump {
            eprintln!("{}", mf.print(&target));
        }
        super::ralloc::allocate(&mut mf, &target, super::gen::reg_limit());
        if dump {
            eprintln!("after allocation:\n{}", mf.print(&target));
        }
        // The probe of a big frame writes `x16` and `x17`, which are never allocated.
        let w = super::clobber::written(&mf, &target, super::mir::RegSet::default());
        Ok((mf, w))
    }

    fn function(&mut self, fi: usize, mf: super::mir::MFunc) {
        let label = self.funcs[fi];
        let stops = [
            self.a.new_label(),
            self.a.new_label(),
            self.a.new_label(),
            self.a.new_label(),
        ];
        let mut env = super::a64_emit::Env {
            a: &mut self.a,
            ro: &mut self.ro,
            funcs: &self.funcs,
            data_off: &self.data_off,
            stops,
            rt: self.rt.as_ref().expect("the runtime is emitted first"),
        };
        super::a64_emit::emit(&mut env, &mf, label);
    }
}
