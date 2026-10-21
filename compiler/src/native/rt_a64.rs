//! The runtime for AArch64 (card A-10), written as machine code like `rt.rs` is for x86-64:
//! process start, output, the stop routine, the allocator and the routines the generator calls
//! (`memcpy`, `fmod`). The behaviour is the same as the x86-64 runtime's; see `rt.rs` for the
//! state layout (`STATE_SIZE` bytes at the start of `.data`) and `docs/design/backend.md`.
//!
//! Internal calling convention: arguments in `x0` to `x7`, result in `x0` (`d0` for floats). A
//! routine may change `x0` to `x17` and the flags; it keeps `x19` to `x28`, `x29` and the stack
//! pointer. The system call numbers are the generic Linux ones (`write` 64, `exit_group` 94,
//! `mmap` 222, ...), not the x86-64 ones.
//!
//! On Windows the same routines call `kernel32` through the import table (`win_call`). Those calls
//! may change `x0` to `x17`, `v0` to `v7` and the flags and keep `x19` and up, which is what this
//! runtime promises already; `x18` is the system's and is never touched. Windows has no `mremap`, so
//! a large block always grows by copying, and its stack is reserved and committed at once (it is
//! only touched as it is used) with an inaccessible zone at the bottom.
//!
//! State words (see `rt.rs`): 144 the base of our stack (0: the system's), 152 the top of the stack
//! the Windows fault handler runs on (0: none), 160 scratch for `VirtualProtect` and `GetConsoleMode`.

use super::a64::*;
use super::elf::Blob;
use super::rt::{
    IMP_ADD_VEH, IMP_EXIT_PROCESS, IMP_GET_CONSOLE_MODE, IMP_GET_STD_HANDLE, IMP_VIRTUAL_ALLOC,
    IMP_VIRTUAL_FREE, IMP_VIRTUAL_PROTECT, IMP_WRITE_FILE,
};
use super::Os;

pub use super::rt::STATE_SIZE;

const PRINT_BUF: i64 = 64 * 1024;
const CHUNK: i64 = 1 << 20;
const STACK: i64 = 1 << 30;
/// The stack on Windows: reserved and committed in one go, 256 MiB.
const STACK_WIN: i64 = 1 << 28;
/// The stack the Windows fault handler runs on.
const HANDLER_STACK_WIN: i64 = 64 * 1024;
/// The lowest 1 MiB of our stack is mapped inaccessible, so running into it is a fault.
const GUARD: i64 = 1 << 20;
/// Faults this far below the base count as stack overflow too (a frame can step over a guard).
const SLACK: i64 = 1 << 20;
const ALT_STACK: i64 = 32 * 1024;
const SMALL_MAX: i64 = 64 * 1024;
const LARGE_CLASS: i64 = 255;
/// Mappings at least this big ask for huge pages.
const HUGE_MIN: i64 = 4 << 20;

const SYS_IOCTL: i64 = 29;
const SYS_WRITE: i64 = 64;
const SYS_EXIT_GROUP: i64 = 94;
const SYS_SIGALTSTACK: i64 = 132;
const SYS_RT_SIGACTION: i64 = 134;
const SYS_MUNMAP: i64 = 215;
const SYS_MREMAP: i64 = 216;
const SYS_MMAP: i64 = 222;
const SYS_MPROTECT: i64 = 226;
const SYS_MADVISE: i64 = 233;

/// The entry points of the runtime.
pub struct Runtime {
    /// `rt_write(fd = x0, ptr = x1, len = x2)`: writes everything or gives up on an error.
    pub write_fd: Label,
    /// `rt_print(ptr = x0, len = x1)`: appends to the output buffer.
    pub print: Label,
    /// `rt_stop(ptr = x0, len = x1)`: prints `stopped: <message>` to standard error and exits with status 1.
    pub stop: Label,
    /// `rt_alloc(size = x0) -> x0`: zeroed memory, 16-byte aligned; stops when out of memory.
    pub alloc: Label,
    /// `rt_free(ptr = x0)`: accepts null.
    pub free: Label,
    /// `rt_realloc(ptr = x0, size = x1) -> x0`
    pub realloc: Label,
    /// `rt_exit(status = x0)`
    pub exit: Label,
    /// `rt_write_u64(fd = x0, value = x1)`: decimal digits.
    pub write_u64: Label,
    /// `rt_memcpy(dst = x0, src = x1, len = x2)`: the areas do not overlap.
    pub memcpy: Label,
    /// `rt_fmod(x = d0, y = d1) -> d0`: C's `fmod`, exactly.
    pub fmod: Label,
}

fn state(a: &mut Asm, r: Reg, off: u32) {
    a.lea_sect(r, Sect::Data, off);
}

fn sys(a: &mut Asm, nr: i64) {
    a.mov_ri(X8, nr);
    a.svc();
}

/// Calls the imported Windows function `import` (an index into `rt::IMPORTS`) with the arguments in
/// `x0` to `x7`. The link register is lost: the caller keeps its own.
fn win_call(a: &mut Asm, import: usize) {
    a.lea_sect(X16, Sect::Idata, super::pe::iat_slot(import));
    a.ldr(X16, X16, 0, 8);
    a.blr(X16);
}

/// Fresh zeroed read-write memory (`mmap(0, len, PROT_READ | PROT_WRITE, flags, -1, 0)` on Linux,
/// `VirtualAlloc` on Windows) of the length in `x1`; the address, or an error value that [`failed`]
/// recognises, in `x0`.
fn map_x1(a: &mut Asm, os: Os, flags: i64) {
    if os == Os::Windows {
        a.mov_ri(X0, 0);
        a.mov_ri(X2, 0x3000); // commit and reserve
        a.mov_ri(X3, 4); // read and write
        win_call(a, IMP_VIRTUAL_ALLOC);
        return;
    }
    a.mov_ri(X0, 0);
    a.mov_ri(X2, 3);
    a.mov_ri(X3, flags);
    a.mov_ri(X4, -1);
    a.mov_ri(X5, 0);
    sys(a, SYS_MMAP);
}

fn mmap(a: &mut Asm, os: Os, len: i64, flags: i64) {
    a.mov_ri(X1, len);
    map_x1(a, os, flags);
}

/// Jumps to `to` when `r` holds the failure of a mapping call: an error value of a system call
/// (-4095 to -1), or null on Windows.
fn failed(a: &mut Asm, os: Os, r: Reg, to: Label) {
    if os == Os::Windows {
        a.cbz(r, to);
        return;
    }
    a.mov_ri(X15, -4095);
    a.cmp_rr(r, X15);
    a.b_cond(Cond::Hs, to);
}

/// Zeroes `[from, to)`, both multiples of 16 (clobbers `from`).
fn zero_range(a: &mut Asm, from: Reg, to: Reg) {
    let (top, done) = (a.new_label(), a.new_label());
    a.bind(top);
    a.cmp_rr(from, to);
    a.b_cond(Cond::Hs, done);
    a.stp(XZR, XZR, from, 0);
    a.add_ri(from, from, 16);
    a.b(top);
    a.bind(done);
}

/// Pushes a frame that keeps `x19` to `x22` (and the link register) and pops it again.
fn enter(a: &mut Asm) {
    a.stp_pre(FP, LR, SP, -48);
    a.mov_from_sp(FP);
    a.stp(X19, X20, SP, 16);
    a.stp(X21, X22, SP, 32);
}

fn leave(a: &mut Asm) {
    a.ldp(X19, X20, SP, 16);
    a.ldp(X21, X22, SP, 32);
    a.ldp_post(FP, LR, SP, 48);
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
    os: Os,
) -> Runtime {
    let win = os == Os::Windows;
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
        memcpy: a.new_label(),
        fmod: a.new_label(),
    };
    let flush = a.new_label();
    let fault = a.new_label();
    let oom = a.new_label();
    let msg_stopped = ro.add(b"stopped: ", 1);
    let msg_overflow = ro.add(b"stack overflow", 1);
    let msg_invalid = ro.add(b"invalid memory access", 1);
    let msg_nl = ro.add(b"\n", 1);
    let msg_oom = ro.add(b"out of memory", 1);
    let msg_leak1 = ro.add(b"leaked ", 1);
    let msg_leak2 = ro.add(b" allocation(s)\n", 1);

    // ---- _start ------------------------------------------------------------------------------------
    a.mov_rr(FP, XZR);
    // A big stack of our own, taken lazily, so deep recursion works like it does in the
    // interpreter; if the mapping fails, the stack the system gave us stays.
    let keep = a.new_label();
    let stack = if win { STACK_WIN } else { STACK };
    mmap(a, os, stack, 0x4022); // Linux: private, anonymous, no reservation
    failed(a, os, X0, keep);
    a.mov_rr(X19, X0);
    if !win {
        // Big frames (a 40 MB array in a local) fault in 2 MiB at a time when the kernel agrees.
        a.mov_ri(X1, STACK);
        a.mov_ri(X2, 14); // MADV_HUGEPAGE
        sys(a, SYS_MADVISE);
    }
    state(a, X9, 144);
    a.str(X19, X9, 0, 8);
    a.mov_ri(X9, stack);
    a.add_rr(X9, X19, X9);
    a.mov_to_sp(X9);
    a.bind(keep);
    a.mov_from_sp(X9);
    a.and_ri(X9, X9, !15);
    a.mov_to_sp(X9);
    // The output buffer; without one, `print` writes straight through.
    mmap(a, os, PRINT_BUF, 0x22);
    let ok = a.new_label();
    failed(a, os, X0, ok);
    a.mov_ri(X0, 0);
    a.bind(ok);
    state(a, X9, 128);
    a.str(X0, X9, 0, 8);
    // A person watching a terminal sees each print at once: no buffer there.
    let piped = a.new_label();
    if win {
        a.mov_ri(X0, -11);
        win_call(a, IMP_GET_STD_HANDLE);
        state(a, X1, 160);
        win_call(a, IMP_GET_CONSOLE_MODE);
        a.uxt(X0, X0, 32); // a BOOL
        a.cbz(X0, piped);
    } else {
        a.sub_ri(SP, SP, 64);
        a.mov_ri(X0, 1);
        a.mov_ri(X1, 0x5401); // TCGETS
        a.mov_from_sp(X2);
        sys(a, SYS_IOCTL);
        a.add_ri(SP, SP, 64);
        a.cbnz(X0, piped);
    }
    state(a, X9, 128);
    a.str(XZR, X9, 0, 8);
    a.bind(piped);
    // Guard against running off the stack, and report stray addresses as stops.
    let noguard = a.new_label();
    state(a, X9, 144);
    a.ldr(X0, X9, 0, 8);
    a.cbz(X0, noguard);
    a.mov_ri(X1, GUARD);
    if win {
        a.mov_ri(X2, 1); // PAGE_NOACCESS
        state(a, X3, 160);
        win_call(a, IMP_VIRTUAL_PROTECT);
    } else {
        a.mov_ri(X2, 0);
        sys(a, SYS_MPROTECT);
    }
    a.bind(noguard);
    // A stack of its own for the handler: the faulting one may be the one that ran out.
    let nosig = a.new_label();
    if win {
        mmap(a, os, HANDLER_STACK_WIN, 0);
        let none = a.new_label();
        failed(a, os, X0, none);
        a.mov_ri(X9, HANDLER_STACK_WIN);
        a.add_rr(X0, X0, X9);
        state(a, X9, 152);
        a.str(X0, X9, 0, 8);
        a.bind(none);
        a.mov_ri(X0, 1); // call this handler first
        a.lea_label(X1, fault);
        win_call(a, IMP_ADD_VEH);
    } else {
        mmap(a, os, ALT_STACK, 0x22);
        failed(a, os, X0, nosig);
        a.sub_ri(SP, SP, 64);
        a.str(X0, SP, 0, 8); // stack_t { sp, flags, size }
        a.str(XZR, SP, 8, 8);
        a.mov_ri(X9, ALT_STACK);
        a.str(X9, SP, 16, 8);
        a.mov_from_sp(X0);
        a.mov_ri(X1, 0);
        sys(a, SYS_SIGALTSTACK);
        // struct sigaction { handler, flags, (restorer,) mask }: on the alternate stack, with siginfo.
        // The words after the flags stay zero, which suits the layout with and without a restorer.
        a.lea_label(X9, fault);
        a.str(X9, SP, 32, 8);
        a.mov_ri(X9, 0x0800_0004); // SA_ONSTACK | SA_SIGINFO
        a.str(X9, SP, 40, 8);
        a.str(XZR, SP, 48, 8);
        a.str(XZR, SP, 56, 8);
        for sig in [11, 7] {
            a.mov_ri(X0, sig);
            a.add_ri(X1, SP, 32);
            a.mov_ri(X2, 0);
            a.mov_ri(X3, 8);
            sys(a, SYS_RT_SIGACTION);
        }
        a.add_ri(SP, SP, 64);
    }
    a.bind(nosig);
    a.bl(main);
    let clean = a.new_label();
    if leak_check {
        state(a, X9, 112);
        a.ldr(X19, X9, 0, 8);
        a.cbz(X19, clean);
        a.mov_ri(X0, 2);
        a.lea_sect(X1, Sect::Rodata, msg_leak1);
        a.mov_ri(X2, 7);
        a.bl(rt.write_fd);
        a.mov_ri(X0, 2);
        a.mov_rr(X1, X19);
        a.bl(rt.write_u64);
        a.mov_ri(X0, 2);
        a.lea_sect(X1, Sect::Rodata, msg_leak2);
        a.mov_ri(X2, 15);
        a.bl(rt.write_fd);
        a.mov_ri(X0, 3);
        a.bl(rt.exit);
    }
    a.bind(clean);
    a.mov_ri(X0, 0);
    a.bl(rt.exit);

    // ---- rt_exit -----------------------------------------------------------------------------------
    a.bind(rt.exit);
    a.mov_rr(X19, X0);
    a.bl(flush);
    a.mov_rr(X0, X19);
    if win {
        win_call(a, IMP_EXIT_PROCESS);
    } else {
        sys(a, SYS_EXIT_GROUP);
    }
    a.brk();

    // ---- rt_flush ----------------------------------------------------------------------------------
    // Writes what the output buffer holds. Ends in `rt_write` or a `ret`.
    a.bind(flush);
    {
        let none = a.new_label();
        state(a, X9, 128);
        a.ldr(X1, X9, 0, 8);
        a.ldr(X2, X9, 8, 8);
        a.cbz(X1, none);
        a.cbz(X2, none);
        a.str(XZR, X9, 8, 8);
        a.mov_ri(X0, 1);
        a.b(rt.write_fd);
        a.bind(none);
        a.ret();
    }

    // ---- rt_print ----------------------------------------------------------------------------------
    // Buffered standard output: copy into the buffer; when it does not fit, write the buffer out
    // first; text that is bigger than the whole buffer is written directly.
    a.bind(rt.print);
    {
        let (direct, copy, big) = (a.new_label(), a.new_label(), a.new_label());
        state(a, X9, 128);
        a.ldr(X10, X9, 0, 8);
        a.cbz(X10, direct);
        a.ldr(X11, X9, 8, 8);
        a.mov_ri(X12, PRINT_BUF);
        a.sub_rr(X12, X12, X11);
        a.cmp_rr(X1, X12);
        a.b_cond(Cond::Ls, copy);
        // Full: write out what is there, then start again.
        a.stp_pre(X0, X1, SP, -32);
        a.str(LR, SP, 16, 8);
        a.mov_ri(X0, 1);
        a.mov_rr(X1, X10);
        a.mov_rr(X2, X11);
        a.bl(rt.write_fd);
        a.ldr(LR, SP, 16, 8);
        a.ldp_post(X0, X1, SP, 32);
        state(a, X9, 128);
        a.str(XZR, X9, 8, 8);
        a.ldr(X10, X9, 0, 8);
        a.mov_ri(X11, 0);
        a.mov_ri(X12, PRINT_BUF);
        a.cmp_rr(X1, X12);
        a.b_cond(Cond::Hi, big);
        a.bind(copy);
        a.add_rr(X13, X10, X11);
        a.add_rr(X14, X11, X1);
        a.str(X14, X9, 8, 8);
        a.mov_rr(X2, X1);
        a.mov_rr(X1, X0);
        a.mov_rr(X0, X13);
        a.b(rt.memcpy);
        a.bind(direct);
        a.bind(big);
        a.mov_rr(X2, X1);
        a.mov_rr(X1, X0);
        a.mov_ri(X0, 1);
        a.b(rt.write_fd);
    }

    // ---- rt_write ----------------------------------------------------------------------------------
    a.bind(rt.write_fd);
    if win {
        // GetStdHandle once, then WriteFile until everything is written. x19 the handle, x20 the
        // pointer, x21 the count; the count `WriteFile` reports goes to the word at `sp`.
        let (wloop, wdone, std_out, have) =
            (a.new_label(), a.new_label(), a.new_label(), a.new_label());
        enter(a);
        a.sub_ri(SP, SP, 16);
        a.mov_rr(X20, X1);
        a.mov_rr(X21, X2);
        a.cmp_ri(X0, 1);
        a.b_cond(Cond::Eq, std_out);
        a.mov_ri(X0, -12); // standard error
        a.b(have);
        a.bind(std_out);
        a.mov_ri(X0, -11); // standard output
        a.bind(have);
        win_call(a, IMP_GET_STD_HANDLE);
        a.mov_rr(X19, X0);
        a.bind(wloop);
        a.cbz(X21, wdone);
        a.mov_rr(X0, X19);
        a.mov_rr(X1, X20);
        a.mov_rr(X2, X21);
        a.mov_from_sp(X3);
        a.mov_ri(X4, 0); // no overlapped I/O
        win_call(a, IMP_WRITE_FILE);
        a.uxt(X0, X0, 32); // a BOOL
        a.cbz(X0, wdone);
        a.ldr(X9, SP, 0, 4);
        a.cbz(X9, wdone);
        a.add_rr(X20, X20, X9);
        a.sub_rr(X21, X21, X9);
        a.b(wloop);
        a.bind(wdone);
        a.add_ri(SP, SP, 16);
        leave(a);
        a.ret();
    } else {
        let (wloop, wdone) = (a.new_label(), a.new_label());
        a.mov_rr(X9, X0);
        a.bind(wloop);
        a.cbz(X2, wdone);
        a.mov_rr(X0, X9);
        sys(a, SYS_WRITE);
        a.adds_ri(XZR, X0, 4); // interrupted (-4): try again
        a.b_cond(Cond::Eq, wloop);
        a.cmp_ri(X0, 0);
        a.b_cond(Cond::Le, wdone);
        a.add_rr(X1, X1, X0);
        a.sub_rr(X2, X2, X0);
        a.b(wloop);
        a.bind(wdone);
        a.ret();
    }

    // ---- rt_write_u64 ------------------------------------------------------------------------------
    a.bind(rt.write_u64);
    {
        a.sub_ri(SP, SP, 48);
        a.str(LR, SP, 32, 8);
        a.mov_rr(X9, X1);
        a.add_ri(X10, SP, 32);
        a.mov_ri(X11, 10);
        let digit = a.new_label();
        a.bind(digit);
        a.udiv(X12, X9, X11);
        a.msub(X13, X12, X11, X9);
        a.add_ri(X13, X13, 48);
        a.sub_ri(X10, X10, 1);
        a.str(X13, X10, 0, 1);
        a.mov_rr(X9, X12);
        a.cbnz(X9, digit);
        a.mov_rr(X1, X10);
        a.add_ri(X2, SP, 32);
        a.sub_rr(X2, X2, X10);
        a.bl(rt.write_fd);
        a.ldr(LR, SP, 32, 8);
        a.add_ri(SP, SP, 48);
        a.ret();
    }

    // ---- rt_memcpy ---------------------------------------------------------------------------------
    a.bind(rt.memcpy);
    {
        let (blocks, pairs, bytes, done, tail) = (
            a.new_label(),
            a.new_label(),
            a.new_label(),
            a.new_label(),
            a.new_label(),
        );
        a.bind(blocks);
        a.cmp_ri(X2, 32);
        a.b_cond(Cond::Lo, pairs);
        a.ldp(X3, X4, X1, 0);
        a.ldp(X5, X6, X1, 16);
        a.stp(X3, X4, X0, 0);
        a.stp(X5, X6, X0, 16);
        a.add_ri(X0, X0, 32);
        a.add_ri(X1, X1, 32);
        a.sub_ri(X2, X2, 32);
        a.b(blocks);
        a.bind(pairs);
        a.cmp_ri(X2, 16);
        a.b_cond(Cond::Lo, tail);
        a.ldp(X3, X4, X1, 0);
        a.stp(X3, X4, X0, 0);
        a.add_ri(X0, X0, 16);
        a.add_ri(X1, X1, 16);
        a.sub_ri(X2, X2, 16);
        a.bind(tail);
        a.mov_ri(X5, 0);
        a.bind(bytes);
        a.cmp_rr(X5, X2);
        a.b_cond(Cond::Hs, done);
        a.ldr_idx(X3, X1, X5, 1, false);
        a.str_idx(X3, X0, X5, 1, false);
        a.add_ri(X5, X5, 1);
        a.b(bytes);
        a.bind(done);
        a.ret();
    }

    // ---- rt_stop -----------------------------------------------------------------------------------
    a.bind(rt.stop);
    a.mov_rr(X19, X0);
    a.mov_rr(X20, X1);
    a.bl(flush); // what the program printed comes first
    a.mov_ri(X0, 2);
    a.lea_sect(X1, Sect::Rodata, msg_stopped);
    a.mov_ri(X2, 9);
    a.bl(rt.write_fd);
    a.mov_ri(X0, 2);
    a.mov_rr(X1, X19);
    a.mov_rr(X2, X20);
    a.bl(rt.write_fd);
    a.mov_ri(X0, 2);
    a.lea_sect(X1, Sect::Rodata, msg_nl);
    a.mov_ri(X2, 1);
    a.bl(rt.write_fd);
    a.mov_ri(X0, 1);
    a.bl(rt.exit);

    // ---- the fault handler -------------------------------------------------------------------------
    // `x0` is the signal, `x1` the siginfo (the faulting address at +16).
    {
        let (overflow, invalid, check) = (a.new_label(), a.new_label(), a.new_label());
        a.bind(fault);
        if win {
            // `x0` the exception pointers: the record's code, and for an access violation the
            // address at +0x28. Anything else is passed on (CONTINUE_SEARCH, 0).
            let pass = a.new_label();
            a.ldr(X9, X0, 0, 8);
            a.ldr(X10, X9, 0, 4);
            a.mov_ri(X11, 0xC000_00FD); // stack overflow
            a.cmp_rr(X10, X11);
            a.b_cond(Cond::Eq, overflow);
            a.mov_ri(X11, 0xC000_0005); // access violation
            a.cmp_rr(X10, X11);
            a.b_cond(Cond::Eq, check);
            a.bind(pass);
            a.mov_ri(X0, 0);
            a.ret();
            a.bind(check);
            a.ldr(X9, X9, 0x28, 8);
        } else {
            a.ldr(X9, X1, 16, 8);
        }
        state(a, X10, 144);
        a.ldr(X11, X10, 0, 8);
        a.cbz(X11, invalid);
        a.sub_rr(X9, X9, X11);
        a.add_ri(X9, X9, SLACK as u32);
        a.mov_ri(X12, SLACK + GUARD);
        a.cmp_rr(X9, X12);
        a.b_cond(Cond::Lo, overflow);
        a.bind(invalid);
        if win {
            // The fault may have come from the end of our stack: leave it.
            let keep = a.new_label();
            state(a, X10, 152);
            a.ldr(X11, X10, 0, 8);
            a.cbz(X11, keep);
            a.mov_to_sp(X11);
            a.bind(keep);
        }
        a.lea_sect(X0, Sect::Rodata, msg_invalid);
        a.mov_ri(X1, 21);
        a.bl(rt.stop);
        a.bind(overflow);
        if win {
            let keep = a.new_label();
            state(a, X10, 152);
            a.ldr(X11, X10, 0, 8);
            a.cbz(X11, keep);
            a.mov_to_sp(X11);
            a.bind(keep);
        }
        a.lea_sect(X0, Sect::Rodata, msg_overflow);
        a.mov_ri(X1, 14);
        a.bl(rt.stop);
    }

    // ---- out of memory -----------------------------------------------------------------------------
    a.bind(oom);
    a.lea_sect(X0, Sect::Rodata, msg_oom);
    a.mov_ri(X1, 13);
    a.bl(rt.stop);

    // ---- rt_alloc ----------------------------------------------------------------------------------
    // x19 header plus payload, x20 class, x21 block size, x22 the block.
    a.bind(rt.alloc);
    enter(a);
    a.add_ri(X19, X0, 16);
    let (large, have, size_loop, fresh, retry, carve, out) = (
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
    );
    a.mov_ri(X9, SMALL_MAX);
    a.cmp_rr(X19, X9);
    a.b_cond(Cond::Hi, large);
    a.mov_ri(X20, 0);
    a.mov_ri(X21, 32);
    a.bind(size_loop);
    a.cmp_rr(X21, X19);
    a.b_cond(Cond::Hs, have);
    a.add_rr(X21, X21, X21);
    a.add_ri(X20, X20, 1);
    a.b(size_loop);
    a.bind(have);
    state(a, X10, 0);
    a.add_rr_sh(X10, X10, X20, Shift::Lsl, 3);
    a.ldr(X11, X10, 0, 8); // head of this class's free list
    a.cbz(X11, fresh);
    // Reuse: unlink, then clear the payload.
    a.ldr(X12, X11, 16, 8);
    a.str(X12, X10, 0, 8);
    a.mov_rr(X22, X11);
    a.add_ri(X13, X11, 16);
    a.add_rr(X14, X11, X21);
    zero_range(a, X13, X14);
    a.b(out);
    // Fresh: carve from the current chunk (already zero), asking for a new chunk when it is used up.
    a.bind(fresh);
    a.bind(retry);
    state(a, X10, 96);
    a.ldr(X22, X10, 0, 8);
    a.add_rr(X12, X22, X21);
    a.ldr(X13, X10, 8, 8);
    a.cmp_rr(X12, X13);
    a.b_cond(Cond::Ls, carve);
    mmap(a, os, CHUNK, 0x22);
    failed(a, os, X0, oom);
    state(a, X10, 96);
    a.str(X0, X10, 0, 8);
    a.mov_ri(X9, CHUNK);
    a.add_rr(X9, X0, X9);
    a.str(X9, X10, 8, 8);
    a.b(retry);
    a.bind(carve);
    a.str(X12, X10, 0, 8);
    a.str(X20, X22, 0, 8); // header: the class
    a.b(out);
    // Large: its own mapping, rounded to whole pages; the header keeps the length.
    a.bind(large);
    a.add_ri(X21, X19, 4095);
    a.and_ri(X21, X21, !4095);
    // The block freed last, if it is big enough and not more than twice as big, is reused (cleared)
    // instead of a new mapping with its page faults.
    let mapnew = a.new_label();
    state(a, X10, 120);
    a.ldr(X11, X10, 0, 8);
    a.cbz(X11, mapnew);
    a.ldr(X12, X11, 8, 8);
    a.cmp_rr(X12, X21);
    a.b_cond(Cond::Lo, mapnew);
    a.add_rr(X13, X21, X21);
    a.cmp_rr(X12, X13);
    a.b_cond(Cond::Hi, mapnew);
    a.str(XZR, X10, 0, 8);
    a.mov_rr(X22, X11);
    a.add_ri(X13, X11, 16);
    a.add_rr(X14, X11, X12);
    zero_range(a, X13, X14);
    a.b(out);
    a.bind(mapnew);
    a.mov_rr(X1, X21);
    map_x1(a, os, 0x22);
    failed(a, os, X0, oom);
    a.mov_rr(X22, X0);
    if !win {
        advise_huge(a, X22, X21);
    }
    a.mov_ri(X9, LARGE_CLASS);
    a.str(X9, X22, 0, 8);
    a.str(X21, X22, 8, 8);
    a.bind(out);
    a.add_ri(X0, X22, 16);
    state(a, X10, 112);
    a.ldr(X9, X10, 0, 8);
    a.add_ri(X9, X9, 1);
    a.str(X9, X10, 0, 8);
    leave(a);
    a.ret();

    // ---- rt_free -----------------------------------------------------------------------------------
    a.bind(rt.free);
    {
        let (fdone, flarge, unmap) = (a.new_label(), a.new_label(), a.new_label());
        a.cbz(X0, fdone);
        a.sub_ri(X10, X0, 16);
        a.ldr(X11, X10, 0, 8);
        state(a, X12, 112);
        a.ldr(X13, X12, 0, 8);
        a.sub_ri(X13, X13, 1);
        a.str(X13, X12, 0, 8);
        a.cmp_ri(X11, LARGE_CLASS as u32);
        a.b_cond(Cond::Eq, flarge);
        state(a, X12, 0);
        a.add_rr_sh(X12, X12, X11, Shift::Lsl, 3);
        a.ldr(X13, X12, 0, 8);
        a.str(X13, X10, 16, 8);
        a.str(X10, X12, 0, 8);
        a.bind(fdone);
        a.ret();
        // Keep one large block for the next large allocation; unmap when the slot is taken.
        a.bind(flarge);
        state(a, X12, 120);
        a.ldr(X13, X12, 0, 8);
        a.cbnz(X13, unmap);
        a.str(X10, X12, 0, 8);
        a.ret();
        a.bind(unmap);
        if win {
            // VirtualFree(block, 0, MEM_RELEASE); the call loses the link register.
            a.stp_pre(FP, LR, SP, -16);
            a.mov_rr(X0, X10);
            a.mov_ri(X1, 0);
            a.mov_ri(X2, 0x8000);
            win_call(a, IMP_VIRTUAL_FREE);
            a.ldp_post(FP, LR, SP, 16);
        } else {
            a.ldr(X1, X10, 8, 8);
            a.mov_rr(X0, X10);
            sys(a, SYS_MUNMAP);
        }
        a.ret();
    }

    // ---- rt_realloc --------------------------------------------------------------------------------
    // A large block moves with `mremap`: the kernel moves the pages, nothing is copied and the new
    // tail is zero. Anything else gets a new block, the smaller of the two sizes copied, and the old
    // block freed. x19 old block, x20 new size, x21 new block or length, x22 the remapped block.
    a.bind(rt.realloc);
    {
        let (generic, rdone, rclass, rcopy, rskip) = (
            a.new_label(),
            a.new_label(),
            a.new_label(),
            a.new_label(),
            a.new_label(),
        );
        enter(a);
        a.mov_rr(X19, X0);
        a.mov_rr(X20, X1);
        a.cbz(X19, generic);
        a.sub_ri(X10, X19, 16);
        a.ldr(X11, X10, 0, 8);
        a.cmp_ri(X11, LARGE_CLASS as u32);
        a.b_cond(Cond::Ne, generic);
        if win {
            // No `mremap` here: a large block moves by copying.
            a.b(generic);
        } else {
            // New mapping length: header plus payload, whole pages.
            a.add_imm(X12, X20, 16 + 4095);
            a.and_ri(X12, X12, !4095);
            a.ldr(X13, X10, 8, 8); // old length
            a.cmp_rr(X12, X13);
            a.b_cond(Cond::Eq, rdone); // same pages: the block stays
            a.mov_rr(X21, X12);
            a.mov_rr(X0, X10);
            a.mov_rr(X1, X13);
            a.mov_rr(X2, X12);
            a.mov_ri(X3, 1); // may move
            sys(a, SYS_MREMAP);
            failed(a, os, X0, oom);
            a.str(X21, X0, 8, 8);
            a.mov_rr(X22, X0);
            advise_huge(a, X22, X21);
            a.add_ri(X19, X22, 16);
            a.b(rdone);
        }
        // Generic: allocate, copy what fits, free the old block.
        a.bind(generic);
        a.mov_rr(X0, X20);
        a.bl(rt.alloc);
        a.mov_rr(X21, X0);
        a.cbz(X19, rskip);
        // Bytes the old block can give: the rest of its class, or its mapping.
        a.ldr(X11, X19, -16, 8);
        a.cmp_ri(X11, LARGE_CLASS as u32);
        a.b_cond(Cond::Ne, rclass);
        a.ldr(X12, X19, -8, 8);
        a.b(rcopy);
        a.bind(rclass);
        a.mov_ri(X12, 32);
        a.lslv(X12, X12, X11);
        a.bind(rcopy);
        a.sub_ri(X12, X12, 16);
        a.cmp_rr(X12, X20);
        a.csel(X2, X12, X20, Cond::Ls);
        a.mov_rr(X0, X21);
        a.mov_rr(X1, X19);
        a.bl(rt.memcpy);
        a.mov_rr(X0, X19);
        a.bl(rt.free);
        a.bind(rskip);
        a.mov_rr(X19, X21);
        a.bind(rdone);
        a.mov_rr(X0, X19);
        leave(a);
        a.ret();
    }

    // ---- rt_fmod -----------------------------------------------------------------------------------
    // C's `fmod` for doubles, bit by bit (the algorithm of musl): the result is exact. x9 the bits
    // of x, x10 the bits of y, x11 and x12 their exponents, x13 the sign of x.
    a.bind(rt.fmod);
    {
        let (nanres, cont, retx, xnorm, xdone, ynorm, ydone) = (
            a.new_label(),
            a.new_label(),
            a.new_label(),
            a.new_label(),
            a.new_label(),
            a.new_label(),
            a.new_label(),
        );
        let (top, after, skip, skip2, denorm, fin, zero) = (
            a.new_label(),
            a.new_label(),
            a.new_label(),
            a.new_label(),
            a.new_label(),
            a.new_label(),
            a.new_label(),
        );
        a.fmov_to_gp(true, X9, V(0));
        a.fmov_to_gp(true, X10, V(1));
        a.ubfx(X11, X9, 52, 11);
        a.ubfx(X12, X10, 52, 11);
        a.lsr_ri(X13, X9, 63);
        // y is zero or NaN, or x is infinite or NaN: the result is NaN.
        a.lsl_ri(X14, X10, 1);
        a.cbz(X14, nanres);
        a.mov_ri(X15, 0xFFE0_0000_0000_0000u64 as i64);
        a.cmp_rr(X14, X15);
        a.b_cond(Cond::Hi, nanres);
        a.cmp_ri(X11, 0x7FF);
        a.b_cond(Cond::Eq, nanres);
        // |x| < |y|: x itself; |x| == |y|: a zero with the sign of x.
        a.lsl_ri(X15, X9, 1);
        a.cmp_rr(X15, X14);
        a.b_cond(Cond::Hi, cont);
        a.b_cond(Cond::Ne, retx);
        a.b(zero);
        a.bind(retx);
        a.ret();
        a.bind(cont);
        // Make both operands a 53-bit integer with the exponent kept apart.
        a.cbnz(X11, xnorm);
        a.lsl_ri(X14, X9, 12);
        a.clz(X14, X14);
        a.neg(X11, X14);
        a.add_ri(X14, X14, 1);
        a.lslv(X9, X9, X14);
        a.b(xdone);
        a.bind(xnorm);
        a.and_ri(X9, X9, 0x000F_FFFF_FFFF_FFFF);
        a.orr_ri(X9, X9, 1 << 52);
        a.bind(xdone);
        a.cbnz(X12, ynorm);
        a.lsl_ri(X14, X10, 12);
        a.clz(X14, X14);
        a.neg(X12, X14);
        a.add_ri(X14, X14, 1);
        a.lslv(X10, X10, X14);
        a.b(ydone);
        a.bind(ynorm);
        a.and_ri(X10, X10, 0x000F_FFFF_FFFF_FFFF);
        a.orr_ri(X10, X10, 1 << 52);
        a.bind(ydone);
        // x mod y, one bit of the exponent difference at a time.
        a.bind(top);
        a.cmp_rr(X11, X12);
        a.b_cond(Cond::Le, after);
        a.cmp_rr(X9, X10);
        a.b_cond(Cond::Lo, skip);
        a.b_cond(Cond::Eq, zero);
        a.sub_rr(X9, X9, X10);
        a.bind(skip);
        a.lsl_ri(X9, X9, 1);
        a.sub_ri(X11, X11, 1);
        a.b(top);
        a.bind(after);
        a.cmp_rr(X9, X10);
        a.b_cond(Cond::Lo, skip2);
        a.b_cond(Cond::Eq, zero);
        a.sub_rr(X9, X9, X10);
        a.bind(skip2);
        // Shift the leading one back to bit 52.
        a.clz(X14, X9);
        a.sub_ri(X14, X14, 11);
        a.lslv(X9, X9, X14);
        a.sub_rr(X11, X11, X14);
        // Scale the result: a normal number, or a subnormal one.
        a.cmp_ri(X11, 0);
        a.b_cond(Cond::Le, denorm);
        a.and_ri(X9, X9, 0x000F_FFFF_FFFF_FFFF);
        a.lsl_ri(X14, X11, 52);
        a.orr_rr(X9, X9, X14);
        a.b(fin);
        a.bind(denorm);
        a.neg(X14, X11);
        a.add_ri(X14, X14, 1);
        a.lsrv(X9, X9, X14);
        a.bind(fin);
        a.lsl_ri(X13, X13, 63);
        a.orr_rr(X9, X9, X13);
        a.fmov_from_gp(true, V(0), X9);
        a.ret();
        a.bind(zero);
        a.lsl_ri(X13, X13, 63);
        a.fmov_from_gp(true, V(0), X13);
        a.ret();
        a.bind(nanres);
        a.fmul(true, V(0), V(0), V(1));
        a.fdiv(true, V(0), V(0), V(0));
        a.ret();
    }
    rt
}

/// `madvise(addr, len, MADV_HUGEPAGE)` for a big mapping; `addr` and `len` registers are kept.
fn advise_huge(a: &mut Asm, addr: Reg, len: Reg) {
    let skip = a.new_label();
    a.mov_ri(X9, HUGE_MIN);
    a.cmp_rr(len, X9);
    a.b_cond(Cond::Lo, skip);
    a.mov_rr(X0, addr);
    a.mov_rr(X1, len);
    a.mov_ri(X2, 14);
    sys(a, SYS_MADVISE);
    a.bind(skip);
}
