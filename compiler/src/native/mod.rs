//! The native backend (card A-09, design in `docs/design/backend.md`): x86-64 machine code and
//! an ELF executable, straight from the IR, with no assembler, linker or libc.
//!
//! * [`x86`] encodes instructions.
//! * [`elf`] lays out and writes a static Linux executable.
//! * [`rt`] is the small runtime the generated code needs (start, write, stop, allocator).

/// The operating system the executable is built for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Os {
    Linux,
    Windows,
    /// Apple Silicon only (AArch64, `--arch=aarch64`).
    MacOs,
}

/// The processor the executable is built for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    /// The processor this compiler runs on, when it is one we can build for.
    pub fn host() -> Option<Arch> {
        if cfg!(target_arch = "x86_64") {
            Some(Arch::X86_64)
        } else if cfg!(target_arch = "aarch64") {
            Some(Arch::Aarch64)
        } else {
            None
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }
}

impl Os {
    /// The system this compiler runs on, when it is one we can build for.
    pub fn host() -> Option<Os> {
        if cfg!(all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )) {
            Some(Os::Linux)
        } else if cfg!(all(
            target_os = "windows",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )) {
            Some(Os::Windows)
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            Some(Os::MacOs)
        } else {
            None
        }
    }
}

/// The runtime routines a program can call. A routine nothing calls is left out of the executable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Uses {
    /// Buffered standard output (`print`); without it the start code sets up no buffer.
    pub print: bool,
    pub alloc: bool,
    pub free: bool,
    /// Decimal digits to a file (only the leak report, or a test program).
    pub write_u64: bool,
    pub realloc: bool,
    /// AArch64 only: copies that are not a few constant bytes.
    pub memcpy: bool,
    /// AArch64 only: float remainder.
    pub fmod: bool,
    /// Context switching (`CtxInit`, `CtxSwitch`).
    pub ctx: bool,
    /// The OS memory calls (`Op::Os`).
    pub os: bool,
}

/// What a program that talks to its process (arguments, descriptors, exit) cannot yet be built for.
pub const PROCESS_REFUSED: &str = "the operating system layer is not built for this target";

/// What a program that reads its arguments cannot yet be built for on Windows.
pub const ARGV_REFUSED: &str =
    "`main(args)` and `std.env` are built for x86-64 (Linux and Windows) only so far (AArch64 has no start block yet)";

/// Whether the module uses `OsKind::Argv`.
pub fn uses_argv(m: &crate::ir::Module) -> bool {
    use crate::ir::{Op, OsKind};
    m.funcs
        .iter()
        .flat_map(|f| &f.insts)
        .any(|i| matches!(&i.op, Op::Os(OsKind::Argv, ..)))
}

/// Whether the module uses `OsKind::Write`, `Read`, `Exit` or `Argv`.
pub fn uses_process(m: &crate::ir::Module) -> bool {
    use crate::ir::Op;
    m.funcs
        .iter()
        .flat_map(|f| &f.insts)
        .any(|i| matches!(&i.op, Op::Os(k, ..) if k.is_process()))
}

impl Uses {
    /// Every routine: what the hand-written test programs may call.
    pub const ALL: Uses = Uses {
        print: true,
        alloc: true,
        free: true,
        write_u64: true,
        realloc: true,
        memcpy: true,
        fmod: true,
        ctx: true,
        os: true,
    };

    /// The routines the module's instructions can call (a little too many is fine, too few is not).
    pub fn of(m: &crate::ir::Module) -> Uses {
        use crate::ir::{BinOp, Op};
        let mut u = Uses::default();
        for f in &m.funcs {
            // Only the instructions a block still holds: the optimiser leaves the ones it removed in
            // `insts`, and counting them linked a `free` routine into a program that never frees.
            for i in f
                .blocks
                .iter()
                .flat_map(|b| b.insts.iter())
                .map(|&n| &f.insts[n])
            {
                match &i.op {
                    Op::Print(..) => u.print = true,
                    Op::Alloc(_) => u.alloc = true,
                    Op::Free(_) => u.free = true,
                    Op::Realloc(..) => u.realloc = true,
                    Op::MemCopy(..) => u.memcpy = true,
                    Op::CtxInit(..) | Op::CtxSwitch(..) => u.ctx = true,
                    Op::Os(k, ..) => {
                        u.os = true;
                        // Opening a file builds a block for the path on Windows.
                        if *k == crate::ir::OsKind::Open {
                            u.alloc = true;
                            u.free = true;
                        }
                    }
                    Op::Bin(BinOp::Rem, a, _) if f.ty(*a).is_float() => u.fmod = true,
                    _ => {}
                }
            }
        }
        u
    }
}

pub mod a64;
pub mod a64_emit;
pub mod a64_mir;
pub mod a64_select;
pub mod clobber;
pub mod elf;
pub mod exec;
pub mod gen;
pub mod gen_a64;
#[cfg(target_os = "linux")]
pub mod load;
pub mod macho;
pub mod mir;
pub mod pe;
pub mod ralloc;
pub mod regs;
pub mod rt;
pub mod rt_a64;
pub mod x86;
pub mod x86_emit;
pub mod x86_mir;
pub mod x86_select;

#[cfg(test)]
mod a64_golden;
#[cfg(test)]
mod ralloc_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod x86_golden;

/// Places the module's static data in the `.data` blob and gives the offset of each block. With `bss`, the blocks that are
/// all zero go last and are left out of the blob (the loader gives zeroed memory right after it, ELF `.bss`): the
/// executable is smaller by their size. Gives the offsets and the number of zeroed bytes that follow the blob.
pub(super) fn place_module_data(
    blocks: &[Vec<u8>],
    data: &mut elf::Blob,
    bss: bool,
) -> (Vec<u32>, usize) {
    let mut offs = vec![0u32; blocks.len()];
    let zero = |b: &Vec<u8>| bss && b.iter().all(|&x| x == 0);
    for (i, b) in blocks.iter().enumerate() {
        if !zero(b) {
            offs[i] = data.add(b, 16);
        }
    }
    let start = data.add(&[], 16) as usize;
    let mut end = start;
    for (i, b) in blocks.iter().enumerate() {
        if zero(b) {
            end = end.div_ceil(16) * 16;
            offs[i] = end as u32;
            end += b.len();
        }
    }
    (offs, end - start)
}
