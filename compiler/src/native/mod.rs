//! The native backend (card A-09, design in `docs/design/backend.md`): x86-64 machine code and
//! an ELF executable, straight from the IR, with no assembler, linker or libc.
//!
//! * [`x86`] encodes instructions.
//! * [`elf`] lays out and writes a static Linux executable.
//! * [`rt`] is the small runtime the generated code needs (start, write, stop, allocator).

pub mod elf;
pub mod rt;
pub mod x86;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod x86_golden;
