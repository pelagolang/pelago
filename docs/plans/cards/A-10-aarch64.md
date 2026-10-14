Id: A-10
Title: AArch64 (64-bit ARM) backend
Column: Needs decision
Order: after A-09 N6 (track A)
Depends on: A-09 (the IR-to-machine-code design, the runtime, the tests)
Size: L, in stages (R1 to R4)
Touches: new `compiler/src/native/a64.rs` (encoder), `native/gen_a64.rs` (code generator), `rt_a64` (runtime), writers for Linux ELF, Windows PE and macOS Mach-O, CI

# Words first

- **x86-64** (also written x86_64, AMD64, x64) is what A-09 builds. It is the 64-bit Intel/AMD architecture; every current PC and server uses it.
- **32-bit x86** (i386, i686) is a different, older instruction set. **Not planned:** no current system needs it, and the IR (64-bit pointers, 64-bit `int`) is designed for 64-bit targets.
- **AArch64** (ARM64, ARMv8-A and later) is the 64-bit ARM architecture: Apple Silicon Macs, Windows on ARM laptops, Raspberry Pi 4 and 5, AWS Graviton and most phones. ROADMAP Phase 6 names it as the next target, then RISC-V.

# Why it is a good second target

- **Fixed-width instructions** (always 4 bytes): the encoder is simpler than x86's, with no prefixes, mod-rm bytes or variable lengths.
- **Load/store architecture** with 31 general registers: the "every value in a frame slot" generator translates directly, and register assignment has much more room (callee-saved `x19` to `x28`: ten registers instead of x86's five).
- **Testable here:** `qemu-user` (apt) runs Linux ARM64 programs on this x86 container; GitHub offers free ARM runners for public repositories (`ubuntu-24.04-arm`, `windows-11-arm`, and `macos-latest`, which is ARM).

# Plan

| Stage | Work | Done when |
|---|---|---|
| **R1** | `a64.rs`: the encoder for the forms the generator needs (`mov`/`movz`/`movk`, `ldr`/`str` in all widths, `add`/`sub`/`and`/`orr`/`eor`, `mul`, `sdiv`/`udiv`/`msub`, shifts, `cmp`, `cset`, `b`/`b.cond`/`bl`/`blr`/`ret`, `adr`/`adrp`, scalar float `fadd`/`fsub`/`fmul`/`fdiv`/`fcmp`/`scvtf`/`fcvtzs`/`fcvt`, `svc`). Golden bytes checked against `aarch64-linux-gnu-as` (apt `binutils-aarch64-linux-gnu`). | every form matches the assembler |
| **R2** | Linux ELF (`EM_AARCH64`), the runtime over syscalls (`svc #0`: `write` 64, `exit_group` 94, `mmap` 222, `munmap` 215), the simple generator `gen_a64.rs`. Calling convention: AAPCS64 (`x0` to `x7`, `d0` to `d7`, callee-saved `x19` to `x28`). | all tests green under `qemu-aarch64` and on the `ubuntu-24.04-arm` runner |
| **R3** | Registers for the heaviest values (`x19` to `x28`), same scheme as `regs.rs` (the analysis is shared; only the register list differs). | all tests green; benches recorded |
| **R4** | Windows on ARM (PE machine `0xAA64`, `kernel32` imports, Microsoft ARM64 conventions) and macOS (Mach-O). | each its own card; see risks |

# Design choice: how to share code with x86-64

The simple generator is about 700 lines of templates. Two ways to support a second CPU:

1. **A second generator over the same IR** (recommended): `gen.rs` and `gen_a64.rs` share the analysis (`regs.rs`), the frame layout idea and the test harness, and differ in instruction templates. Cheap, readable, and each stays simple. Cost: a bug in IR handling has to be fixed twice.
2. **A machine-level IR in between** (virtual registers, two-address ops) with one instruction selector per CPU. Better for optimisation (card A-11 wants instruction selection with addressing modes and fused compare-and-branch), but a large rewrite of what works.

**Recommendation:** do (1) now, and take (2) when A-11 starts, because A-11's instruction selection is the natural place to introduce it. The tests (three-way differential against the interpreter) protect either choice.

# Risks

- **macOS** will not run an unsigned executable on Apple Silicon: a Mach-O needs at least an ad-hoc code signature, and normal programs go through `libSystem` (raw syscalls are unsupported there and may change). That is why macOS is R4 and its own card; it needs a Mac to test (a CI runner can do it).
- **Windows on ARM** needs the Microsoft ARM64 ABI details (frame records, `x18` reserved) and can only be tested on the `windows-11-arm` runner.
- **Instruction cache**: we only write files, never patch code at run time, so no cache maintenance is needed.

# Questions for the owner

1. AArch64 next after A-09's speed work (recommended), or before it?
2. Option (1) for now, option (2) with A-11 (recommended)?
3. Is macOS wanted at all in the first round, or only Linux and Windows on ARM?
