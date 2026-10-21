//! The runtime the generated code links against, written as machine code: process start, output,
//! the stop routine, and the allocator (`docs/design/backend.md`, section 3).
//!
//! Internal calling convention (System V shaped): arguments in `rdi rsi rdx rcx r8 r9`, result in
//! `rax`. Every routine here preserves `rbx`, `rbp` and `r12` to `r15`.
//!
//! The runtime's state lives at the start of `.data` ([`STATE_SIZE`] bytes):
//!
//! | offset | what |
//! |---|---|
//! | 0 to 95 | heads of the twelve free lists (block sizes 32 B to 64 KiB) |
//! | 96, 104 | next free byte and end of the current chunk |
//! | 112 | live blocks (for the leak check) |
//! | 120 | the one freed large block kept for reuse (0 when none) |
//! | 128 | the output buffer ([`PRINT_BUF`] bytes, mapped at start; 0 when the mapping failed) |
//! | 136 | bytes waiting in the output buffer |
//! | 144 | the base (lowest address) of our own stack, 0 if the system stack is used |
//! | 152 | Windows: the lowest committed address of our stack (it grows downwards on demand) |
//! | 160 | scratch for `VirtualProtect` (Windows) |

use super::elf::Blob;
use super::x86::*;
use super::{Os, Uses};

pub const STATE_SIZE: usize = 176;
/// Size of the buffer that `print` fills; it is written out when full, at exit and at a stop.
const PRINT_BUF: i64 = 64 * 1024;
const CHUNK: i64 = 1 << 20;
const STACK: i64 = 1 << 30;
/// The stack on Windows is reserved up front and committed on demand.
const STACK_WIN: i64 = 1 << 28;
/// What is committed at start, and how much more each guard page hit commits.
const COMMIT_WIN: i64 = 512 * 1024;

/// The kernel32 functions the Windows image imports, in the order of its import table.
pub const IMPORTS: [&str; 8] = [
    "GetStdHandle",
    "WriteFile",
    "ExitProcess",
    "VirtualAlloc",
    "VirtualFree",
    "AddVectoredExceptionHandler",
    "VirtualProtect",
    "GetConsoleMode",
];
pub(super) const IMP_GET_STD_HANDLE: usize = 0;
pub(super) const IMP_WRITE_FILE: usize = 1;
pub(super) const IMP_EXIT_PROCESS: usize = 2;
pub(super) const IMP_VIRTUAL_ALLOC: usize = 3;
pub(super) const IMP_VIRTUAL_FREE: usize = 4;
pub(super) const IMP_ADD_VEH: usize = 5;
pub(super) const IMP_VIRTUAL_PROTECT: usize = 6;
pub(super) const IMP_GET_CONSOLE_MODE: usize = 7;

/// Linux: the lowest 1 MiB of our stack is mapped inaccessible, so running into it is a fault.
const GUARD_LINUX: i64 = 1 << 20;
/// Windows: a `PAGE_GUARD` page this far above the lowest committed address raises an exception while
/// there is still committed room below it to run the handler; the handler commits more.
const GUARD_WIN: i64 = 64 * 1024;
/// Faults this far below the base count as stack overflow too (a frame can step over a guard).
const SLACK: i32 = 1 << 20;
/// Size of the stack the Linux signal handler runs on.
const ALT_STACK: i64 = 32 * 1024;

/// Calls an imported Windows function with the arguments already in `rcx rdx r8 r9`: aligns the
/// stack, makes the shadow space, lets `setup` store further arguments (the fifth is at `[rsp + 32]`),
/// and restores the stack afterwards. Only `rbp` is used for that, and it is restored.
fn win_call(a: &mut Asm, import: usize, setup: impl FnOnce(&mut Asm)) {
    a.push(RBP);
    a.mov_rr(RBP, RSP, 8);
    a.alu_ri(Alu::And, RSP, -16, 8);
    a.alu_ri(Alu::Sub, RSP, 48, 8);
    setup(a);
    a.call_mem_sect(Sect::Idata, super::pe::iat_slot(import));
    a.mov_rr(RSP, RBP, 8);
    a.pop(RBP);
}
/// The largest block served from the free lists: header plus payload.
const SMALL_MAX: i32 = 64 * 1024;
const LARGE_CLASS: i32 = 255;
/// Windows: a large block of at least this many bytes reserves eight times its size up front and
/// commits pages as it grows, so `rt_realloc` extends it in place (there is no `mremap`).
const RESERVE_MIN: i32 = 1 << 20;

/// The entry points of the runtime.
pub struct Runtime {
    /// `rt_write(fd = rdi, ptr = rsi, len = rdx)`: writes everything or gives up on an error.
    pub write_fd: Label,
    /// `rt_print(ptr = rsi, len = rdx)`: appends to the output buffer; clobbers the argument registers.
    pub print: Label,
    /// `rt_stop(ptr = rdi, len = rsi)`: prints `stopped: <message>` to standard error and exits with status 1.
    pub stop: Label,
    /// `rt_alloc(size = rdi) -> rax`: zeroed memory, 16-byte aligned; never fails (stops when out of memory).
    pub alloc: Label,
    /// `rt_free(ptr = rdi)`: accepts null.
    pub free: Label,
    /// `rt_realloc(ptr = rdi, size = rsi) -> rax`: a block of the new size holding the old contents (null
    /// allocates); the rest is zero. A large block is remapped, so growing a big buffer copies nothing.
    pub realloc: Label,
    /// `rt_exit(status = rdi)`
    pub exit: Label,
    /// `rt_write_u64(fd = rdi, value = rsi)`: decimal digits.
    pub write_u64: Label,
}

fn state(a: &mut Asm, r: Reg, off: u32) {
    a.lea_sect(r, Sect::Data, off);
}

/// Emits `_start` (at offset 0 of the code) and the routines. `main` is called with no arguments;
/// its result is ignored. With `leak_check`, the live-block count is checked after `main` returns:
/// a leak prints `leaked N allocation(s)` to standard error and exits with status 3.
pub fn emit(
    a: &mut Asm,
    ro: &mut Blob,
    data: &mut Blob,
    main: Label,
    leak_check: bool,
    target: Os,
) -> Runtime {
    emit_for(a, ro, data, main, leak_check, target, Uses::ALL)
}

/// Like [`emit`], with only the routines `uses` names (the labels of the others are never bound, so
/// nothing may call them).
pub fn emit_for(
    a: &mut Asm,
    ro: &mut Blob,
    data: &mut Blob,
    main: Label,
    leak_check: bool,
    target: Os,
    uses: Uses,
) -> Runtime {
    let win = target == Os::Windows;
    // `realloc` allocates and frees.
    let alloc_used = uses.alloc || uses.realloc;
    let free_used = uses.free || uses.realloc;
    let state_at = data.add_zero(STATE_SIZE, 16);
    assert_eq!(state_at, 0, "the runtime state must come first in .data");
    let rt = Runtime {
        write_fd: a.new_label(),
        print: a.new_label(),
        stop: a.new_label(),
        alloc: a.new_label(),
        free: a.new_label(),
        realloc: a.new_label(),
        exit: a.new_label(),
        write_u64: a.new_label(),
    };
    let flush = a.new_label();
    let fault = a.new_label();
    let msg_stopped = ro.add(b"stopped: ", 1);
    let msg_overflow = ro.add(b"stack overflow", 1);
    let msg_invalid = ro.add(b"invalid memory access", 1);
    let msg_nl = ro.add(b"\n", 1);
    let msg_oom = if alloc_used {
        ro.add(b"out of memory", 1)
    } else {
        0
    };
    let (msg_leak1, msg_leak2) = if leak_check {
        (ro.add(b"leaked ", 1), ro.add(b" allocation(s)\n", 1))
    } else {
        (0, 0)
    };

    // ---- _start ------------------------------------------------------------------------------------
    a.alu_rr(Alu::Xor, RBP, RBP, 4);
    // A big stack of our own, taken lazily, so deep recursion works like it does in the
    // interpreter; if the mapping fails, the stack the system gave us stays.
    let keep = a.new_label();
    if win {
        // Reserve the whole stack, commit its top. If either fails, the system stack stays (and no
        // guard is set up: the base stays 0).
        a.alu_rr(Alu::Xor, RCX, RCX, 4);
        a.mov_ri(RDX, STACK_WIN);
        a.mov_ri(R8, 0x2000); // reserve
        a.mov_ri(R9, 4); // read and write
        win_call(a, IMP_VIRTUAL_ALLOC, |_| {});
        a.test_rr(RAX, RAX, 8);
        a.jcc(Cc::E, keep);
        a.mov_rr(RBX, RAX, 8);
        a.lea(RCX, mem(RBX, (STACK_WIN - COMMIT_WIN) as i32));
        a.mov_ri(RDX, COMMIT_WIN);
        a.mov_ri(R8, 0x1000); // commit
        a.mov_ri(R9, 4);
        win_call(a, IMP_VIRTUAL_ALLOC, |_| {});
        a.test_rr(RAX, RAX, 8);
        a.jcc(Cc::E, keep);
        state(a, RCX, 144);
        a.store(mem(RCX, 0), RBX, 8);
        state(a, RCX, 152);
        a.store(mem(RCX, 0), RAX, 8);
        a.lea(RSP, mem(RBX, STACK_WIN as i32));
    } else {
        a.mov_ri(RAX, 9);
        a.alu_rr(Alu::Xor, RDI, RDI, 4);
        a.mov_ri(RSI, STACK);
        a.mov_ri(RDX, 3);
        a.mov_ri(R10, 0x4022); // private, anonymous, no reservation
        a.mov_ri(R8, -1);
        a.alu_rr(Alu::Xor, R9, R9, 4);
        a.syscall();
        a.alu_ri(Alu::Cmp, RAX, -4095, 8);
        a.jcc(Cc::Ae, keep);
        // Big frames (a 40 MB array in a local) fault in 2 MiB at a time when the kernel agrees.
        a.push(RAX);
        a.mov_rr(RDI, RAX, 8);
        a.mov_ri(RSI, STACK);
        a.mov_ri(RDX, 14); // MADV_HUGEPAGE
        a.mov_ri(RAX, 28);
        a.syscall();
        a.pop(RAX);
        state(a, RCX, 144);
        a.store(mem(RCX, 0), RAX, 8);
        a.lea(RSP, mem(RAX, STACK as i32));
    }
    a.bind(keep);
    a.alu_ri(Alu::And, RSP, -16, 8);
    if uses.print {
        // The output buffer; without one, `print` writes straight through.
        if win {
            a.alu_rr(Alu::Xor, RCX, RCX, 4);
            a.mov_ri(RDX, PRINT_BUF);
            a.mov_ri(R8, 0x3000);
            a.mov_ri(R9, 4);
            win_call(a, IMP_VIRTUAL_ALLOC, |_| {});
        } else {
            a.mov_ri(RAX, 9);
            a.alu_rr(Alu::Xor, RDI, RDI, 4);
            a.mov_ri(RSI, PRINT_BUF);
            a.mov_ri(RDX, 3);
            a.mov_ri(R10, 0x22); // private, anonymous
            a.mov_ri(R8, -1);
            a.alu_rr(Alu::Xor, R9, R9, 4);
            a.syscall();
            let ok = a.new_label();
            a.alu_ri(Alu::Cmp, RAX, -4095, 8);
            a.jcc(Cc::Be, ok);
            a.alu_rr(Alu::Xor, RAX, RAX, 4);
            a.bind(ok);
        }
        state(a, RSI, 128);
        a.store(mem(RSI, 0), RAX, 8);
        // A person watching a terminal sees each print at once: no buffer there.
        let piped = a.new_label();
        if win {
            a.mov_ri(RCX, -11);
            win_call(a, IMP_GET_STD_HANDLE, |_| {});
            a.mov_rr(RCX, RAX, 8);
            state(a, RDX, 160);
            win_call(a, IMP_GET_CONSOLE_MODE, |_| {});
            a.test_rr(RAX, RAX, 4);
            a.jcc(Cc::E, piped);
        } else {
            a.alu_ri(Alu::Sub, RSP, 64, 8);
            a.mov_ri(RAX, 16); // ioctl(1, TCGETS, &termios)
            a.mov_ri(RDI, 1);
            a.mov_ri(RSI, 0x5401);
            a.mov_rr(RDX, RSP, 8);
            a.syscall();
            a.alu_ri(Alu::Add, RSP, 64, 8);
            a.test_rr(RAX, RAX, 8);
            a.jcc(Cc::Ne, piped);
        }
        state(a, RSI, 128);
        a.store_imm(mem(RSI, 0), 0, 8);
        a.bind(piped);
    }
    // Guard against running off the stack, and report stray addresses as stops.
    let noguard = a.new_label();
    if win {
        state(a, RAX, 152);
        a.load(RCX, mem(RAX, 0), 8);
        a.test_rr(RCX, RCX, 8);
        a.jcc(Cc::E, noguard);
        a.alu_ri(Alu::Add, RCX, GUARD_WIN as i32, 8);
        a.mov_ri(RDX, 4096);
        a.mov_ri(R8, 0x104); // read and write, guard page
        state(a, R9, 160);
        win_call(a, IMP_VIRTUAL_PROTECT, |_| {});
        a.bind(noguard);
        a.mov_ri(RCX, 1);
        a.lea_label(RDX, fault);
        win_call(a, IMP_ADD_VEH, |_| {});
    } else {
        state(a, RSI, 144);
        a.load(RDI, mem(RSI, 0), 8);
        a.test_rr(RDI, RDI, 8);
        a.jcc(Cc::E, noguard);
        a.mov_ri(RSI, GUARD_LINUX);
        a.alu_rr(Alu::Xor, RDX, RDX, 4);
        a.mov_ri(RAX, 10); // mprotect(base, guard, PROT_NONE)
        a.syscall();
        a.bind(noguard);
        // A stack of its own for the handler: the faulting one may be the one that ran out.
        let nosig = a.new_label();
        a.mov_ri(RAX, 9);
        a.alu_rr(Alu::Xor, RDI, RDI, 4);
        a.mov_ri(RSI, ALT_STACK);
        a.mov_ri(RDX, 3);
        a.mov_ri(R10, 0x22);
        a.mov_ri(R8, -1);
        a.alu_rr(Alu::Xor, R9, R9, 4);
        a.syscall();
        a.alu_ri(Alu::Cmp, RAX, -4095, 8);
        a.jcc(Cc::Ae, nosig);
        a.alu_ri(Alu::Sub, RSP, 64, 8);
        a.store(mem(RSP, 0), RAX, 8); // stack_t { sp, flags, size }
        a.store_imm(mem(RSP, 8), 0, 8);
        a.store_imm(mem(RSP, 16), ALT_STACK as i32, 8);
        a.mov_ri(RAX, 131); // sigaltstack
        a.mov_rr(RDI, RSP, 8);
        a.alu_rr(Alu::Xor, RSI, RSI, 4);
        a.syscall();
        // struct sigaction { handler, flags, restorer, mask }: on the alternate stack, with siginfo.
        a.lea_label(RAX, fault);
        a.store(mem(RSP, 32), RAX, 8);
        // The kernel insists on a restorer; the handler never returns, so any address will do.
        a.store_imm(mem(RSP, 40), 0x0C00_0004, 8);
        a.store(mem(RSP, 48), RAX, 8);
        a.store_imm(mem(RSP, 56), 0, 8);
        for sig in [11, 7] {
            a.mov_ri(RAX, 13); // rt_sigaction(sig, &act, 0, 8)
            a.mov_ri(RDI, sig);
            a.lea(RSI, mem(RSP, 32));
            a.alu_rr(Alu::Xor, RDX, RDX, 4);
            a.mov_ri(R10, 8);
            a.syscall();
        }
        a.alu_ri(Alu::Add, RSP, 64, 8);
        a.bind(nosig);
    }
    a.call(main);
    let clean = a.new_label();
    if leak_check {
        state(a, RSI, 112);
        a.load(RAX, mem(RSI, 0), 8);
        a.test_rr(RAX, RAX, 8);
        a.jcc(Cc::E, clean);
        a.mov_rr(RBX, RAX, 8);
        a.mov_ri(RDI, 2);
        a.lea_sect(RSI, Sect::Rodata, msg_leak1);
        a.mov_ri(RDX, 7);
        a.call(rt.write_fd);
        a.mov_ri(RDI, 2);
        a.mov_rr(RSI, RBX, 8);
        a.call(rt.write_u64);
        a.mov_ri(RDI, 2);
        a.lea_sect(RSI, Sect::Rodata, msg_leak2);
        a.mov_ri(RDX, 15);
        a.call(rt.write_fd);
        a.mov_ri(RDI, 3);
        a.call(rt.exit);
    }
    a.bind(clean);
    a.alu_rr(Alu::Xor, RDI, RDI, 4);
    a.call(rt.exit);

    // ---- rt_exit -----------------------------------------------------------------------------------
    a.bind(rt.exit);
    if uses.print {
        a.push(RDI);
        a.call(flush);
        a.pop(RDI);
    }
    if win {
        a.mov_rr(RCX, RDI, 4);
        win_call(a, IMP_EXIT_PROCESS, |_| {});
    } else {
        a.mov_ri(RAX, 231);
        a.syscall();
    }
    a.ud2();

    if uses.print {
        // ---- rt_flush ----------------------------------------------------------------------------------
        // Writes what the output buffer holds. Falls through nowhere: it ends in `rt_write` or a `ret`.
        a.bind(flush);
        {
            let none = a.new_label();
            state(a, R8, 128);
            a.load(RSI, mem(R8, 0), 8);
            a.load(RDX, mem(R8, 8), 8);
            a.test_rr(RSI, RSI, 8);
            a.jcc(Cc::E, none);
            a.test_rr(RDX, RDX, 8);
            a.jcc(Cc::E, none);
            a.store_imm(mem(R8, 8), 0, 8);
            a.mov_ri(RDI, 1);
            a.jmp(rt.write_fd);
            a.bind(none);
            a.ret();
        }

        // ---- rt_print ----------------------------------------------------------------------------------
        // Buffered standard output: copy into the buffer; when it does not fit, write the buffer out
        // first; text that is bigger than the whole buffer is written directly.
        a.bind(rt.print);
        {
            let (direct, copy, big) = (a.new_label(), a.new_label(), a.new_label());
            state(a, R8, 128);
            a.load(RAX, mem(R8, 0), 8);
            a.test_rr(RAX, RAX, 8);
            a.jcc(Cc::E, direct);
            a.load(RCX, mem(R8, 8), 8);
            a.mov_ri(R9, PRINT_BUF);
            a.alu_rr(Alu::Sub, R9, RCX, 8);
            a.alu_rr(Alu::Cmp, RDX, R9, 8);
            a.jcc(Cc::Be, copy);
            // Full: write out what is there, then start again.
            a.push(RSI);
            a.push(RDX);
            a.mov_ri(RDI, 1);
            a.mov_rr(RSI, RAX, 8);
            a.mov_rr(RDX, RCX, 8);
            a.call(rt.write_fd);
            a.pop(RDX);
            a.pop(RSI);
            state(a, R8, 128);
            a.store_imm(mem(R8, 8), 0, 8);
            a.load(RAX, mem(R8, 0), 8);
            a.alu_rr(Alu::Xor, RCX, RCX, 4);
            a.mov_ri(R9, PRINT_BUF);
            a.alu_rr(Alu::Cmp, RDX, R9, 8);
            a.jcc(Cc::A, big);
            a.bind(copy);
            a.lea(
                RDI,
                Mem {
                    base: RAX,
                    index: Some((RCX, 1)),
                    disp: 0,
                },
            );
            a.alu_rr(Alu::Add, RCX, RDX, 8);
            a.store(mem(R8, 8), RCX, 8);
            a.mov_rr(RCX, RDX, 8);
            a.rep_movsb();
            a.ret();
            a.bind(direct);
            a.bind(big);
            a.mov_ri(RDI, 1);
            a.jmp(rt.write_fd);
        }
    }

    // ---- rt_write ----------------------------------------------------------------------------------
    a.bind(rt.write_fd);
    if win {
        // GetStdHandle once, then WriteFile until everything is written. `rbx`, `r12`, `r13` hold the
        // handle, the pointer and the count across the calls; the caller's values are kept on the stack.
        a.push(RBX);
        a.push(R12);
        a.push(R13);
        a.alu_ri(Alu::Sub, RSP, 16, 8); // room for the count `WriteFile` reports
        a.mov_rr(R12, RSI, 8);
        a.mov_rr(R13, RDX, 8);
        let (std_out, have) = (a.new_label(), a.new_label());
        a.alu_ri(Alu::Cmp, RDI, 1, 8);
        a.jcc(Cc::E, std_out);
        a.mov_ri(RCX, -12); // standard error
        a.jmp(have);
        a.bind(std_out);
        a.mov_ri(RCX, -11); // standard output
        a.bind(have);
        win_call(a, IMP_GET_STD_HANDLE, |_| {});
        a.mov_rr(RBX, RAX, 8);
        let (wloop, wdone) = (a.new_label(), a.new_label());
        a.bind(wloop);
        a.test_rr(R13, R13, 8);
        a.jcc(Cc::E, wdone);
        a.mov_rr(RCX, RBX, 8);
        a.mov_rr(RDX, R12, 8);
        a.mov_rr(R8, R13, 4);
        a.lea(R9, mem(RSP, 0));
        win_call(a, IMP_WRITE_FILE, |a| {
            a.store_imm(mem(RSP, 32), 0, 8); // no overlapped I/O
        });
        a.test_rr(RAX, RAX, 4);
        a.jcc(Cc::E, wdone);
        a.load(RAX, mem(RSP, 0), 4);
        a.test_rr(RAX, RAX, 4);
        a.jcc(Cc::E, wdone);
        a.alu_rr(Alu::Add, R12, RAX, 8);
        a.alu_rr(Alu::Sub, R13, RAX, 8);
        a.jmp(wloop);
        a.bind(wdone);
        a.alu_ri(Alu::Add, RSP, 16, 8);
        a.pop(R13);
        a.pop(R12);
        a.pop(RBX);
        a.ret();
    } else {
        let (wloop, wdone) = (a.new_label(), a.new_label());
        a.bind(wloop);
        a.test_rr(RDX, RDX, 8);
        a.jcc(Cc::E, wdone);
        a.mov_ri(RAX, 1);
        a.syscall();
        a.alu_ri(Alu::Cmp, RAX, -4, 8); // interrupted: try again
        a.jcc(Cc::E, wloop);
        a.test_rr(RAX, RAX, 8);
        a.jcc(Cc::Le, wdone);
        a.alu_rr(Alu::Add, RSI, RAX, 8);
        a.alu_rr(Alu::Sub, RDX, RAX, 8);
        a.jmp(wloop);
        a.bind(wdone);
        a.ret();
    }

    if leak_check || uses.write_u64 {
        // ---- rt_write_u64 ------------------------------------------------------------------------------
        a.bind(rt.write_u64);
        a.push(RBX);
        a.alu_ri(Alu::Sub, RSP, 32, 8);
        a.lea(RBX, mem(RSP, 32));
        a.mov_rr(RAX, RSI, 8);
        a.mov_ri(RCX, 10);
        let digit = a.new_label();
        a.bind(digit);
        a.alu_rr(Alu::Xor, RDX, RDX, 4);
        a.div(RCX, 8);
        a.alu_ri(Alu::Add, RDX, 48, 1);
        a.alu_ri(Alu::Sub, RBX, 1, 8);
        a.store(mem(RBX, 0), RDX, 1);
        a.test_rr(RAX, RAX, 8);
        a.jcc(Cc::Ne, digit);
        a.mov_rr(RSI, RBX, 8);
        a.lea(RDX, mem(RSP, 32));
        a.alu_rr(Alu::Sub, RDX, RBX, 8);
        a.call(rt.write_fd);
        a.alu_ri(Alu::Add, RSP, 32, 8);
        a.pop(RBX);
        a.ret();
    }

    // ---- rt_stop -----------------------------------------------------------------------------------
    a.bind(rt.stop);
    a.mov_rr(R12, RDI, 8);
    a.mov_rr(R13, RSI, 8);
    if uses.print {
        a.call(flush); // what the program printed comes first
    }
    a.mov_ri(RDI, 2);
    a.lea_sect(RSI, Sect::Rodata, msg_stopped);
    a.mov_ri(RDX, 9);
    a.call(rt.write_fd);
    a.mov_ri(RDI, 2);
    a.mov_rr(RSI, R12, 8);
    a.mov_rr(RDX, R13, 8);
    a.call(rt.write_fd);
    a.mov_ri(RDI, 2);
    a.lea_sect(RSI, Sect::Rodata, msg_nl);
    a.mov_ri(RDX, 1);
    a.call(rt.write_fd);
    a.mov_ri(RDI, 1);
    a.call(rt.exit);

    // ---- the fault handler -------------------------------------------------------------------------
    // Linux: `rdi` signal, `rsi` siginfo (the address at +16). Windows: `rcx` the exception pointers;
    // anything but a guard page hit, a stack overflow or an access violation is passed on.
    {
        let (check, overflow, invalid) = (a.new_label(), a.new_label(), a.new_label());
        let grow = a.new_label();
        a.bind(fault);
        if win {
            let pass = a.new_label();
            a.load(RAX, mem(RCX, 0), 8);
            a.load(RDX, mem(RAX, 0), 4);
            a.alu_ri(Alu::Cmp, RDX, 0x8000_0001u32 as i32, 4); // guard page violation
            a.jcc(Cc::E, grow);
            a.alu_ri(Alu::Cmp, RDX, 0xC000_00FDu32 as i32, 4); // stack overflow
            a.jcc(Cc::E, overflow);
            a.alu_ri(Alu::Cmp, RDX, 0xC000_0005u32 as i32, 4); // access violation
            a.jcc(Cc::E, check);
            a.bind(pass);
            a.alu_rr(Alu::Xor, RAX, RAX, 4);
            a.ret();
            // Our stack's guard page: commit the next chunk below, put the guard further down, and
            // go on. When the reservation is used up (or the commit fails), that is an overflow.
            a.bind(grow);
            state(a, R8, 152);
            a.load(RCX, mem(R8, 0), 8);
            a.test_rr(RCX, RCX, 8);
            a.jcc(Cc::E, pass); // not ours
            state(a, R9, 144);
            a.load(RAX, mem(R9, 0), 8);
            a.lea(RDX, mem(RCX, -(COMMIT_WIN as i32)));
            a.alu_rr(Alu::Cmp, RDX, RAX, 8);
            a.jcc(Cc::B, overflow);
            a.mov_rr(RCX, RDX, 8);
            a.mov_ri(RDX, COMMIT_WIN);
            a.mov_ri(R8, 0x1000);
            a.mov_ri(R9, 4);
            win_call(a, IMP_VIRTUAL_ALLOC, |_| {});
            a.test_rr(RAX, RAX, 8);
            a.jcc(Cc::E, overflow);
            state(a, R8, 152);
            a.store(mem(R8, 0), RAX, 8);
            a.lea(RCX, mem(RAX, GUARD_WIN as i32));
            a.mov_ri(RDX, 4096);
            a.mov_ri(R8, 0x104);
            state(a, R9, 160);
            win_call(a, IMP_VIRTUAL_PROTECT, |_| {});
            a.mov_ri(RAX, -1); // EXCEPTION_CONTINUE_EXECUTION
            a.ret();
            a.bind(check);
            a.load(RAX, mem(RAX, 0x28), 8);
        } else {
            a.load(RAX, mem(RSI, 16), 8);
            a.bind(check);
        }
        state(a, R8, 144);
        a.load(RCX, mem(R8, 0), 8);
        a.test_rr(RCX, RCX, 8);
        a.jcc(Cc::E, invalid);
        a.alu_rr(Alu::Sub, RAX, RCX, 8);
        a.alu_ri(Alu::Add, RAX, SLACK, 8);
        let end = if win { STACK_WIN } else { GUARD_LINUX };
        a.mov_ri(RDX, SLACK as i64 + end);
        a.alu_rr(Alu::Cmp, RAX, RDX, 8);
        a.jcc(Cc::B, overflow);
        a.bind(invalid);
        a.lea_sect(RDI, Sect::Rodata, msg_invalid);
        a.mov_ri(RSI, 21);
        a.call(rt.stop);
        a.bind(overflow);
        a.lea_sect(RDI, Sect::Rodata, msg_overflow);
        a.mov_ri(RSI, 14);
        a.call(rt.stop);
    }

    let oom = a.new_label();
    if alloc_used {
        // ---- out of memory -----------------------------------------------------------------------------
        a.bind(oom);
        a.lea_sect(RDI, Sect::Rodata, msg_oom);
        a.mov_ri(RSI, 13);
        a.call(rt.stop);

        // ---- rt_alloc ----------------------------------------------------------------------------------
        a.bind(rt.alloc);
        a.push(RBX);
        a.push(R12);
        a.lea(RBX, mem(RDI, 16)); // header plus payload
        let (large, have, size_loop, fresh, retry, carve, out) = (
            a.new_label(),
            a.new_label(),
            a.new_label(),
            a.new_label(),
            a.new_label(),
            a.new_label(),
            a.new_label(),
        );
        a.alu_ri(Alu::Cmp, RBX, SMALL_MAX, 8);
        a.jcc(Cc::A, large);
        a.alu_rr(Alu::Xor, R12, R12, 4); // class
        a.mov_ri(RAX, 32); // block size of that class
        a.bind(size_loop);
        a.alu_rr(Alu::Cmp, RAX, RBX, 8);
        a.jcc(Cc::Ae, have);
        a.alu_rr(Alu::Add, RAX, RAX, 8);
        a.alu_ri(Alu::Add, R12, 1, 8);
        a.jmp(size_loop);
        a.bind(have);
        state(a, RSI, 0);
        a.mov_rr(RCX, R12, 8);
        a.shift_imm(Shift::Shl, RCX, 3, 8);
        a.alu_rr(Alu::Add, RSI, RCX, 8);
        a.load(RDX, mem(RSI, 0), 8); // head of this class's free list
        a.test_rr(RDX, RDX, 8);
        a.jcc(Cc::E, fresh);
        // Reuse: unlink, then clear the payload.
        a.load(RCX, mem(RDX, 16), 8);
        a.store(mem(RSI, 0), RCX, 8);
        a.lea(RDI, mem(RDX, 16));
        a.lea(RCX, mem(RAX, -16));
        a.mov_rr(RBX, RDX, 8);
        a.alu_rr(Alu::Xor, RAX, RAX, 4);
        a.rep_stosb();
        a.mov_rr(RDX, RBX, 8);
        a.jmp(out);
        // Fresh: carve from the current chunk (already zero), asking for a new chunk when it is used up.
        a.bind(fresh);
        state(a, RSI, 96);
        a.bind(retry);
        a.load(RDX, mem(RSI, 0), 8);
        a.mov_rr(RCX, RDX, 8);
        a.alu_rr(Alu::Add, RCX, RAX, 8);
        a.alu_rm(Alu::Cmp, RCX, mem(RSI, 8), 8);
        a.jcc(Cc::Be, carve);
        a.push(RAX);
        a.push(RSI);
        emit_mmap(a, CHUNK, win);
        a.mov_rr(RDX, RAX, 8);
        a.pop(RSI);
        a.pop(RAX);
        map_failed(a, RDX, oom, win);
        a.store(mem(RSI, 0), RDX, 8);
        a.lea(RCX, mem(RDX, 0));
        a.mov_ri(RDI, CHUNK);
        a.alu_rr(Alu::Add, RCX, RDI, 8);
        a.store(mem(RSI, 8), RCX, 8);
        a.jmp(retry);
        a.bind(carve);
        a.store(mem(RSI, 0), RCX, 8);
        a.store(mem(RDX, 0), R12, 8); // header: the class
        a.jmp(out);
        // Large: its own mapping, rounded to whole pages; the header keeps the length.
        a.bind(large);
        a.lea(RSI, mem(RBX, 4095));
        a.alu_ri(Alu::And, RSI, -4096, 8);
        if !win {
            // From 2 MiB up the mapping is a whole number of 2 MiB pages, so the kernel can back it
            // with huge pages from its first byte (it aligns such mappings itself).
            let small = a.new_label();
            a.alu_ri(Alu::Cmp, RSI, HUGE_MIN, 8);
            a.jcc(Cc::B, small);
            a.alu_ri(Alu::Add, RSI, (2 << 20) - 1, 8);
            a.alu_ri(Alu::And, RSI, -(2 << 20), 8);
            a.bind(small);
        }
        a.mov_rr(R12, RSI, 8);
        // The block freed last, if it is big enough and not more than twice as big, is reused (cleared)
        // instead of a new mapping with its page faults.
        let mapnew = a.new_label();
        state(a, RDX, 120);
        a.load(RCX, mem(RDX, 0), 8);
        a.test_rr(RCX, RCX, 8);
        a.jcc(Cc::E, mapnew);
        a.load(RAX, mem(RCX, 8), 8);
        a.alu_rr(Alu::Cmp, RAX, RSI, 8);
        a.jcc(Cc::B, mapnew);
        a.lea(RDI, mem(RSI, 0));
        a.alu_rr(Alu::Add, RDI, RDI, 8);
        a.alu_rr(Alu::Cmp, RAX, RDI, 8);
        a.jcc(Cc::A, mapnew);
        a.store_imm(mem(RDX, 0), 0, 8);
        a.mov_rr(RBX, RCX, 8);
        a.lea(RDI, mem(RCX, 16));
        a.lea(RCX, mem(RAX, -16));
        a.alu_rr(Alu::Xor, RAX, RAX, 4);
        a.rep_stosb();
        a.mov_rr(RDX, RBX, 8);
        a.jmp(out);
        a.bind(mapnew);
        if win {
            // Reserve eight times the length, commit the length; the header word 0 keeps the reserved
            // bytes above the class byte, word 1 the committed length.
            let plain = a.new_label();
            a.alu_ri(Alu::Cmp, R12, RESERVE_MIN, 8);
            a.jcc(Cc::B, plain);
            a.alu_rr(Alu::Xor, RCX, RCX, 4);
            a.mov_rr(RDX, R12, 8);
            a.shift_imm(Shift::Shl, RDX, 3, 8);
            a.mov_ri(R8, 0x2000); // reserve
            a.mov_ri(R9, 4);
            win_call(a, IMP_VIRTUAL_ALLOC, |_| {});
            a.test_rr(RAX, RAX, 8);
            a.jcc(Cc::E, plain); // no room to reserve: an ordinary mapping
            a.mov_rr(RBX, RAX, 8);
            a.mov_rr(RCX, RBX, 8);
            a.mov_rr(RDX, R12, 8);
            a.mov_ri(R8, 0x1000); // commit
            a.mov_ri(R9, 4);
            win_call(a, IMP_VIRTUAL_ALLOC, |_| {});
            a.test_rr(RAX, RAX, 8);
            a.jcc(Cc::E, oom);
            a.mov_rr(RDX, RBX, 8);
            a.mov_rr(RAX, R12, 8);
            a.shift_imm(Shift::Shl, RAX, 11, 8);
            a.alu_ri(Alu::Or, RAX, LARGE_CLASS, 8);
            a.store(mem(RDX, 0), RAX, 8);
            a.store(mem(RDX, 8), R12, 8);
            a.jmp(out);
            a.bind(plain);
        }
        a.mov_rr(RSI, R12, 8);
        emit_mmap_reg(a, RSI, win);
        a.mov_rr(RDX, RAX, 8);
        map_failed(a, RDX, oom, win);
        if !win {
            advise_huge(a, RDX, R12);
        }
        a.store_imm(mem(RDX, 0), LARGE_CLASS, 8);
        a.store(mem(RDX, 8), R12, 8);
        a.bind(out);
        a.lea(RAX, mem(RDX, 16));
        state(a, RSI, 112);
        a.alu_mi(Alu::Add, mem(RSI, 0), 1, 8);
        a.pop(R12);
        a.pop(RBX);
        a.ret();
    }

    if free_used {
        // ---- rt_free -----------------------------------------------------------------------------------
        a.bind(rt.free);
        let (fdone, flarge) = (a.new_label(), a.new_label());
        a.test_rr(RDI, RDI, 8);
        a.jcc(Cc::E, fdone);
        a.lea(RDX, mem(RDI, -16));
        a.load(RAX, mem(RDX, 0), 8);
        if win {
            a.extend_rr(RAX, RAX, 1, false, false); // the class byte
        }
        state(a, RSI, 112);
        a.alu_mi(Alu::Sub, mem(RSI, 0), 1, 8);
        a.alu_ri(Alu::Cmp, RAX, LARGE_CLASS, 8);
        a.jcc(Cc::E, flarge);
        a.shift_imm(Shift::Shl, RAX, 3, 8);
        state(a, RSI, 0);
        a.alu_rr(Alu::Add, RSI, RAX, 8);
        a.load(RCX, mem(RSI, 0), 8);
        a.store(mem(RDX, 16), RCX, 8);
        a.store(mem(RSI, 0), RDX, 8);
        a.bind(fdone);
        a.ret();
        a.bind(flarge);
        // Keep one large block for the next large allocation; unmap when the slot is taken.
        let unmap = a.new_label();
        state(a, RSI, 120);
        a.load(RCX, mem(RSI, 0), 8);
        a.test_rr(RCX, RCX, 8);
        a.jcc(Cc::Ne, unmap);
        a.store(mem(RSI, 0), RDX, 8);
        a.ret();
        a.bind(unmap);
        if win {
            a.mov_rr(RCX, RDX, 8);
            a.alu_rr(Alu::Xor, RDX, RDX, 4);
            a.mov_ri(R8, 0x8000); // release
            win_call(a, IMP_VIRTUAL_FREE, |_| {});
        } else {
            a.load(RSI, mem(RDX, 8), 8);
            a.mov_rr(RDI, RDX, 8);
            a.mov_ri(RAX, 11);
            a.syscall();
        }
        a.ret();
    }

    if uses.realloc {
        // ---- rt_realloc --------------------------------------------------------------------------------
        // A large block moves with `mremap` (Linux): the kernel moves the pages, nothing is copied and the
        // new tail is zero. Anything else gets a new block, the smaller of the two sizes copied, and the
        // old block freed. Windows has no remap: a large block reserves eight times its size and commits
        // pages as it grows, which is the same in-place growth.
        a.bind(rt.realloc);
        let (generic, rdone) = (a.new_label(), a.new_label());
        a.push(RBX);
        a.push(R12);
        a.push(R13);
        a.mov_rr(RBX, RDI, 8);
        a.mov_rr(R12, RSI, 8);
        a.test_rr(RBX, RBX, 8);
        a.jcc(Cc::E, generic);
        a.lea(RDX, mem(RBX, -16));
        a.load(RAX, mem(RDX, 0), 8);
        if win {
            a.extend_rr(RAX, RAX, 1, false, false);
        }
        a.alu_ri(Alu::Cmp, RAX, LARGE_CLASS, 8);
        a.jcc(Cc::Ne, generic);
        if win {
            // Grow in place inside the reservation: commit the new pages, which arrive zeroed.
            a.lea(RCX, mem(R12, 16 + 4095));
            a.alu_ri(Alu::And, RCX, -4096, 8);
            a.load(RSI, mem(RDX, 8), 8); // committed length
            a.alu_rr(Alu::Cmp, RCX, RSI, 8);
            a.jcc(Cc::E, rdone);
            a.jcc(Cc::B, generic); // shrinking copies
            a.load(RAX, mem(RDX, 0), 8);
            a.shift_imm(Shift::Shr, RAX, 8, 8); // reserved bytes, 0 for an ordinary mapping
            a.alu_rr(Alu::Cmp, RCX, RAX, 8);
            a.jcc(Cc::A, generic);
            a.mov_rr(R13, RCX, 8);
            a.lea(RCX, mem(RBX, -16));
            a.alu_rr(Alu::Add, RCX, RSI, 8);
            a.mov_rr(RDX, R13, 8);
            a.alu_rr(Alu::Sub, RDX, RSI, 8);
            a.mov_ri(R8, 0x1000); // commit
            a.mov_ri(R9, 4);
            win_call(a, IMP_VIRTUAL_ALLOC, |_| {});
            a.test_rr(RAX, RAX, 8);
            a.jcc(Cc::E, oom);
            a.store(mem(RBX, -8), R13, 8);
            a.jmp(rdone);
        } else {
            // New mapping length: header plus payload, whole pages.
            a.lea(RCX, mem(R12, 16 + 4095));
            a.alu_ri(Alu::And, RCX, -4096, 8);
            a.load(RSI, mem(RDX, 8), 8); // old length
            a.alu_rr(Alu::Cmp, RCX, RSI, 8);
            a.jcc(Cc::E, rdone); // same pages: the block stays
            a.mov_rr(R13, RCX, 8);
            a.mov_rr(RDI, RDX, 8);
            a.mov_rr(RDX, RCX, 8);
            a.mov_ri(R10, 1); // may move
            a.mov_ri(RAX, 25);
            a.syscall();
            a.alu_ri(Alu::Cmp, RAX, -4095, 8);
            a.jcc(Cc::Ae, oom);
            a.store(mem(RAX, 8), R13, 8);
            a.mov_rr(RDX, RAX, 8);
            advise_huge(a, RDX, R13);
            a.lea(RBX, mem(RDX, 16));
            a.jmp(rdone);
        }
        // Generic: allocate, copy what fits, free the old block.
        a.bind(generic);
        a.mov_rr(RDI, R12, 8);
        a.call(rt.alloc);
        a.mov_rr(R13, RAX, 8);
        let rfree = a.new_label();
        a.test_rr(RBX, RBX, 8);
        a.jcc(Cc::E, rfree);
        // Bytes the old block can give: the rest of its class, or its mapping.
        a.load(RAX, mem(RBX, -16), 8);
        if win {
            a.extend_rr(RAX, RAX, 1, false, false);
        }
        let (rclass, rcopy) = (a.new_label(), a.new_label());
        a.alu_ri(Alu::Cmp, RAX, LARGE_CLASS, 8);
        a.jcc(Cc::Ne, rclass);
        a.load(RCX, mem(RBX, -8), 8);
        a.jmp(rcopy);
        a.bind(rclass);
        a.mov_rr(RCX, RAX, 8);
        a.mov_ri(RAX, 32);
        a.shift_cl(Shift::Shl, RAX, 8);
        a.mov_rr(RCX, RAX, 8);
        a.bind(rcopy);
        a.alu_ri(Alu::Sub, RCX, 16, 8);
        a.alu_rr(Alu::Cmp, RCX, R12, 8);
        let keep = a.new_label();
        a.jcc(Cc::Be, keep);
        a.mov_rr(RCX, R12, 8);
        a.bind(keep);
        a.mov_rr(RDI, R13, 8);
        a.mov_rr(RSI, RBX, 8);
        a.rep_movsb();
        a.mov_rr(RDI, RBX, 8);
        a.call(rt.free);
        a.bind(rfree);
        a.mov_rr(RBX, R13, 8);
        a.bind(rdone);
        a.mov_rr(RAX, RBX, 8);
        a.pop(R13);
        a.pop(R12);
        a.pop(RBX);
        a.ret();
    }
    rt
}

/// Takes `len` bytes of fresh zeroed memory (`mmap` on Linux, `VirtualAlloc` on Windows): the address
/// in `rax`, or an error value that [`map_failed`] recognises.
fn emit_mmap(a: &mut Asm, len: i64, win: bool) {
    a.mov_ri(RSI, len);
    emit_mmap_reg(a, RSI, win);
}

/// The same with the length already in `rsi`.
fn emit_mmap_reg(a: &mut Asm, len: Reg, win: bool) {
    debug_assert_eq!(len, RSI);
    if win {
        a.alu_rr(Alu::Xor, RCX, RCX, 4);
        a.mov_rr(RDX, RSI, 8);
        a.mov_ri(R8, 0x3000); // commit and reserve
        a.mov_ri(R9, 4); // read and write
        win_call(a, IMP_VIRTUAL_ALLOC, |_| {});
        return;
    }
    a.mov_ri(RAX, 9);
    a.alu_rr(Alu::Xor, RDI, RDI, 4);
    a.mov_ri(RDX, 3);
    a.mov_ri(R10, 0x22);
    a.mov_ri(R8, -1);
    a.alu_rr(Alu::Xor, R9, R9, 4);
    a.syscall();
}

/// Mappings at least this big ask for huge pages: a 40 MB buffer then takes a few page faults instead
/// of ten thousand. Only a hint (the kernel may say no); Linux only.
const HUGE_MIN: i32 = 2 << 20;

/// `madvise(addr, len, MADV_HUGEPAGE)` for a big mapping; `addr` and `len` registers are kept.
fn advise_huge(a: &mut Asm, addr: Reg, len: Reg) {
    let skip = a.new_label();
    a.alu_ri(Alu::Cmp, len, HUGE_MIN, 8);
    a.jcc(Cc::B, skip);
    a.push(addr);
    a.mov_rr(RDI, addr, 8);
    a.mov_rr(RSI, len, 8);
    a.mov_ri(RDX, 14);
    a.mov_ri(RAX, 28);
    a.syscall();
    a.pop(addr);
    a.bind(skip);
}

/// Jumps to `oom` when the mapping call that left its result in `r` failed.
fn map_failed(a: &mut Asm, r: Reg, oom: Label, win: bool) {
    if win {
        a.test_rr(r, r, 8);
        a.jcc(Cc::E, oom);
    } else {
        a.alu_ri(Alu::Cmp, r, -4095, 8);
        a.jcc(Cc::Ae, oom);
    }
}
