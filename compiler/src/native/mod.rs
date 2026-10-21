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
    };

    /// The routines the module's instructions can call (a little too many is fine, too few is not).
    pub fn of(m: &crate::ir::Module) -> Uses {
        use crate::ir::{BinOp, Op};
        let mut u = Uses::default();
        for f in &m.funcs {
            for i in &f.insts {
                match &i.op {
                    Op::Print(..) => u.print = true,
                    Op::Alloc(_) => u.alloc = true,
                    Op::Free(_) => u.free = true,
                    Op::Realloc(..) => u.realloc = true,
                    Op::MemCopy(..) => u.memcpy = true,
                    Op::Bin(BinOp::Rem, a, _) if f.ty(*a).is_float() => u.fmod = true,
                    _ => {}
                }
            }
        }
        u
    }
}

pub mod a64;
pub mod elf;
pub mod exec;
pub mod gen;
pub mod gen_a64;
pub mod macho;
pub mod mir;
pub mod pe;
pub mod ralloc;
pub mod regs;
pub mod regs_a64;
pub mod rt;
pub mod rt_a64;
pub mod x86;

#[cfg(test)]
mod a64_golden;
#[cfg(test)]
mod ralloc_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod x86_golden;
