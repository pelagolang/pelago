# `pelago run`: the fast run mode

Status: decided by the thread that built it (2026-10-27); source: the Goose comparison list, item 6.

## Problem

`pelago run` walks the IR in an interpreter. That is the right tool for `--stats`, the step counter and
the leak report, but it is slow: on `bench/programs/collatz` it takes 6.5 s, and the same program built
natively takes 0.09 s *including* compiling it (about 70 times faster). `run --native` already exists,
but it is opt-in, writes an executable to the temporary directory, and starts it as a child process.
Goose's bundled runner shows the target: type `run`, get near-native speed, no separate build step.

## Today

| Command | Pipeline | Speed |
|---|---|---|
| `run` | check, lower, (`--opt`), IR interpreter | slow, full diagnostics (`--stats`, leaks) |
| `run --native` | check, lower, optimise, x86-64/AArch64 code, ELF/PE written to a temp file, child process | native |
| `build` | the same as `run --native`, keeps the file | native |

## Decision

1. **Native is the default for `run`** on hosts the backend supports (Linux x86-64 and AArch64, Windows
   x86-64). It optimises by default, like `build`. `--no-opt` and `--leak-check` keep their meaning.
2. **`--interp` selects the interpreter** (with `--opt` and `--stats` as before). `--stats` implies
   `--interp`, since the step count only exists there. `--native` stays as an accepted no-op, so
   scripts and the docs do not break.
3. **Unsupported hosts** (macOS, other architectures) fall back to the interpreter. The language
   behaves the same either way; only speed differs.
4. **In-process loading on Linux.** Instead of writing a file and starting a child, the compiler maps
   the generated image into its own address space and jumps to its entry point. The image is a static
   executable with no libc whose entry point builds its own stack and ends in `exit_group`, so the
   program simply finishes the process with its own exit status. Steps: map the image at its fixed
   address (`0x400000`) with `MAP_FIXED_NOREPLACE`, copy the segments in, `mprotect` code to
   read+execute and data to read+write, and call the entry. If the address is taken or any call
   fails, fall back to the temporary-file path, so the failure is never visible. Windows keeps the
   temporary file for now (its image needs the import table filled in by the OS loader).
5. **Not a JIT.** There is no second code generator and no tiering: the in-process mode runs the same
   machine code `build` writes, so a program means the same thing everywhere and `build` output can be
   trusted to behave as `run` did.

## Costs and risks

- The program can fault or corrupt memory *inside the compiler process*. This is acceptable for `run`
  because the process ends with the program, and the runtime already installs its own fault handler
  (stack overflow, invalid access) that prints a message and exits.
- The loader needs `unsafe` and two libc calls (`mmap`, `mprotect`) declared by hand with
  `extern "C"`: the standard library already links libc, so there is no new dependency.
- A non-PIE build of the compiler would sit at `0x400000`. The fixed-address map refuses to replace it
  and the fallback runs instead.
- AArch64 relies on `mprotect` to executable making the instruction cache coherent, which Linux
  guarantees on that architecture.
- The interpreter keeps its role: leak report, steps, and a reference for the optimiser tests.

## Plan

1. This note.
2. `run` defaults to native (temporary file), with `--interp` and the fallback.
3. In-process loading on Linux.

Not planned: Windows in-process loading, a persistent compile server, caching compiled images (the
whole pipeline for `hello` is 17 ms, nearly all in lowering the core library; if that matters it is a
separate card).

## Questions for the owner

1. Is making native the default acceptable, given the interpreter then needs `--interp`?
2. A program stopped by a fault in `run` reports as the native runtime does (`stopped: ...`); is that
   the same text and exit status the interpreter gives? (Checked in step 2; differences get listed in
   the PR.)
