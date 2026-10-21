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
        } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            Some(Os::Windows)
        } else {
            None
        }
    }
}

pub mod a64;
pub mod elf;
pub mod exec;
pub mod gen;
pub mod gen_a64;
pub mod pe;
pub mod regs;
pub mod regs_a64;
pub mod rt;
pub mod rt_a64;
pub mod x86;

#[cfg(test)]
mod a64_golden;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod x86_golden;
