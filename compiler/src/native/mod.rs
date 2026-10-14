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

impl Os {
    /// The system this compiler runs on, when it is one we can build for.
    pub fn host() -> Option<Os> {
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            Some(Os::Linux)
        } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            Some(Os::Windows)
        } else {
            None
        }
    }
}

pub mod elf;
pub mod exec;
pub mod gen;
pub mod pe;
pub mod rt;
pub mod x86;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod x86_golden;
