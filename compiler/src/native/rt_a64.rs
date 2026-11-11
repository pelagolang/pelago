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
//! runtime promises already; `x18` is the system's and is never touched. Windows and macOS have no
//! `mremap`, so a large block reserves address space for eight times its length and grows inside it
//! (Windows commits the pages as they are needed); Windows also reserves and commits its stack at once
//! (it is only touched as it is used) with a guard page 256 KiB above its base.
//!
//! State words (see `rt.rs`): 144 the base of our stack (0: the system's), 152 the top of the stack
//! the Windows fault handler runs on (0: none), 160 scratch for `VirtualProtect` and `GetConsoleMode`.

use super::a64::*;
use super::elf::Blob;
use super::rt::{
    IMP_ADD_VEH, IMP_CLOSE_HANDLE, IMP_CREATE_FILE, IMP_EXIT_PROCESS, IMP_GET_CONSOLE_MODE,
    IMP_GET_LAST_ERROR, IMP_GET_STD_HANDLE, IMP_MULTI_BYTE, IMP_READ_FILE, IMP_VIRTUAL_ALLOC,
    IMP_VIRTUAL_FREE, IMP_VIRTUAL_PROTECT, IMP_WRITE_FILE, REGION_BYTES, REGION_MAX, REGION_MIN,
    SPAN_TABLE,
};
use super::{Os, Uses};

pub use super::rt::STATE_SIZE;

const PRINT_BUF: i64 = 64 * 1024;
const CHUNK: i64 = 1 << 20;
const STACK: i64 = 1 << 30;
/// The top of the stack that is never advised to use huge pages (Linux).
const STACK_SMALL: i64 = 2 << 20;
/// The stack on Windows: reserved and committed in one go, 256 MiB.
const STACK_WIN: i64 = 1 << 28;
/// Windows: a `PAGE_GUARD` page this far above the base of our stack. The first touch clears the flag
/// and raises an exception that the system can still deliver on the committed room below, which an
/// inaccessible zone would not allow (the frame of the exception is built at the stack pointer).
const GUARD_WIN: i64 = 256 * 1024;
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

/// The system calls the runtime makes.
#[derive(Clone, Copy)]
enum Sys {
    Ioctl,
    Write,
    Exit,
    Sigaltstack,
    Sigaction,
    Munmap,
    Mremap,
    Mmap,
    Mprotect,
    Madvise,
    Read,
    Open,
    Close,
    ClockGettime,
}

/// The call numbers: the generic Linux ones, and the BSD ones of macOS (`mremap` and `madvise`
/// hints for huge pages are Linux only).
fn number(os: Os, call: Sys) -> i64 {
    let (linux, mac) = match call {
        Sys::Ioctl => (29, 54),
        Sys::Write => (64, 4),
        Sys::Exit => (94, 1),
        Sys::Sigaltstack => (132, 53),
        Sys::Sigaction => (134, 46),
        Sys::Munmap => (215, 73),
        Sys::Mremap => (216, 0),
        Sys::Mmap => (222, 197),
        Sys::Mprotect => (226, 74),
        Sys::Madvise => (233, 75),
        // `openat` on Linux (there is no `open`), `open` on macOS.
        Sys::Read => (63, 3),
        Sys::Open => (56, 5),
        Sys::Close => (57, 6),
        // macOS has no such call (its clocks are in the commpage); `OsKind::Clock` answers -3 there.
        Sys::ClockGettime => (113, 0),
    };
    if os == Os::MacOs {
        mac
    } else {
        linux
    }
}

/// The entry points of the runtime.
pub struct Runtime {
    /// `rtWrite(fd = x0, ptr = x1, len = x2)`: writes everything or gives up on an error.
    pub write_fd: Label,
    /// `rtPrint(ptr = x0, len = x1)`: appends to the output buffer.
    pub print: Label,
    /// `rtStop(ptr = x0, len = x1)`: prints `stopped: <message>` to standard error and exits with status 1.
    pub stop: Label,
    /// `rtAlloc(size = x0) -> x0`: zeroed memory, 16-byte aligned; stops when out of memory.
    pub alloc: Label,
    /// `rtFree(ptr = x0)`: accepts null.
    pub free: Label,
    /// `rtRealloc(ptr = x0, size = x1) -> x0`
    pub realloc: Label,
    /// `rtExit(status = x0)`
    pub exit: Label,
    /// `rtWriteU64(fd = x0, value = x1)`: decimal digits.
    pub write_u64: Label,
    /// `rtMemcpy(dst = x0, src = x1, len = x2)`: the areas do not overlap.
    pub memcpy: Label,
    /// `rtFmod(x = d0, y = d1) -> d0`: C's `fmod`, exactly.
    pub fmod: Label,
    /// `rtCtxInit(base = x0, size = x1, code = x2, env = x3) -> x0`: a context on that stack that starts by
    /// calling `code(env)`; the result is what `rtCtxSwitch` takes.
    pub ctx_init: Label,
    /// `rtCtxSwitch(save = x0, to = x1)`: saves `x19` to `x30` on this stack, stores the stack pointer
    /// at `save`, and continues the context `to`.
    pub ctx_switch: Label,
    /// The operating system memory calls, by `OsKind` order (reserve, commit, release, remap, advise):
    /// arguments in `x0` to `x2`, the result in `x0` (an address or 0, or a negative error).
    pub os: [Label; 14],
}

fn state(a: &mut Asm, r: Reg, off: u32) {
    a.lea_sect(r, Sect::Data, off);
}

/// A system call. Linux: number in `x8`, result in `x0` (-4095 to -1 for an error). macOS: number in
/// `x16`, `svc #0x80`, an error is the carry flag with a positive error number in `x0`, turned here into
/// the Linux form; macOS also overwrites `x1`, so nothing may be kept there across a call.
fn sys(a: &mut Asm, os: Os, call: Sys) {
    let nr = number(os, call);
    if os == Os::MacOs {
        let ok = a.new_label();
        a.mov_ri(X16, nr);
        a.svc_imm(0x80);
        a.b_cond(Cond::Lo, ok); // carry clear
        a.neg(X0, X0);
        a.bind(ok);
    } else {
        a.mov_ri(X8, nr);
        a.svc();
    }
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
    // macOS: MAP_PRIVATE is 2, MAP_ANON 0x1000, MAP_NORESERVE 0x40.
    let flags = match (os, flags) {
        (Os::MacOs, 0x22) => 0x1002,
        (Os::MacOs, 0x4022) => 0x1042,
        _ => flags,
    };
    a.mov_ri(X0, 0);
    a.mov_ri(X2, 3);
    a.mov_ri(X3, flags);
    a.mov_ri(X4, -1);
    a.mov_ri(X5, 0);
    sys(a, os, Sys::Mmap);
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

/// The Linux small-block allocator for `rtAlloc` (payload size in `x0`, `x19` = size + 16, `x19` to
/// `x22` saved by the caller), the same design as the x86-64 one in `rt.rs`: 16-byte classes up to 256
/// bytes, then four per doubling up to `REGION_MAX`, 64 KiB spans carved from one reserved region, no
/// header. Class entries (free list, bump pointer, bump limit, block size) sit at the start of the
/// region; the span table follows at `SPAN_TABLE`. Ends with the epilogue and `ret`; larger sizes jump to
/// `large`. `x20` the class, `x21` its entry, `x22` the block, `x9` the region base.
fn emit_region_alloc(a: &mut Asm, large: Label, oom: Label) {
    let (big, grow, grown, classed, have_region, retry, bump, zero, done, refill) = (
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
    );
    let (small_bs, have_bs, reginit, try_map, shrink, mapped) = (
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
    );
    a.mov_ri(X9, REGION_MAX as i64);
    a.cmp_rr(X0, X9);
    a.b_cond(Cond::Hi, large);
    a.cmp_ri(X0, 256);
    a.b_cond(Cond::Hi, big);
    // Up to 256 bytes: class (n + 15) / 16 - 1, at least 0.
    a.add_ri(X10, X0, 15);
    a.lsr_ri(X10, X10, 4);
    a.mov_ri(X11, 1);
    a.cmp_rr(X10, X11);
    a.csel(X10, X11, X10, Cond::Lo);
    a.sub_ri(X20, X10, 1);
    a.b(classed);
    // Above: with m = n - 1 and e = the position of its top bit, class 4 * e - 16 + the next two bits.
    a.bind(big);
    a.sub_ri(X10, X0, 1);
    a.mov_ri(X11, 8);
    a.mov_ri(X12, 512);
    a.bind(grow);
    a.cmp_rr(X10, X12);
    a.b_cond(Cond::Lo, grown);
    a.add_ri(X11, X11, 1);
    a.add_rr(X12, X12, X12);
    a.b(grow);
    a.bind(grown);
    a.sub_ri(X13, X11, 2);
    a.lsrv(X10, X10, X13);
    a.and_ri(X10, X10, 3);
    a.lsl_ri(X20, X11, 2);
    a.sub_ri(X20, X20, 16);
    a.add_rr(X20, X20, X10);

    a.bind(classed);
    state(a, X10, 0);
    a.ldr(X9, X10, 0, 8); // the region's base, 0 before the first allocation
    a.cbnz(X9, have_region);
    a.bl(reginit);
    a.mov_rr(X9, X0);
    a.bind(have_region);
    a.lsl_ri(X21, X20, 5);
    a.add_rr(X21, X21, X9); // this class's entry
    a.bind(retry);
    a.ldr(X11, X21, 0, 8);
    a.cbz(X11, bump);
    // Reuse: unlink, then clear the whole block.
    a.ldr(X12, X11, 0, 8);
    a.str(X12, X21, 0, 8);
    a.ldr(X13, X21, 24, 8);
    a.mov_rr(X22, X11);
    a.add_rr(X14, X11, X13);
    a.mov_rr(X15, X11);
    a.bind(zero);
    a.stp(XZR, XZR, X15, 0);
    a.add_ri(X15, X15, 16);
    a.cmp_rr(X15, X14);
    a.b_cond(Cond::Lo, zero);
    a.b(done);
    // Fresh: carve from the current span (zero already); a class that has no span yet has block size 0.
    a.bind(bump);
    a.ldr(X22, X21, 8, 8);
    a.ldr(X13, X21, 24, 8);
    a.cbz(X13, refill);
    a.add_rr(X14, X22, X13);
    a.ldr(X15, X21, 16, 8);
    a.cmp_rr(X14, X15);
    a.b_cond(Cond::Hi, refill);
    a.str(X14, X21, 8, 8);
    a.bind(done);
    a.mov_rr(X0, X22);
    state(a, X10, 112);
    a.ldr(X11, X10, 0, 8);
    a.add_ri(X11, X11, 1);
    a.str(X11, X10, 0, 8);
    leave(a);
    a.ret();

    // Take the next 64 KiB span for this class: block size 16 * (k + 1) below class 16, else
    // (5 + (k & 3)) << (6 + ((k - 16) >> 2)). `x9` stays the region base.
    a.bind(refill);
    a.cmp_ri(X20, 16);
    a.b_cond(Cond::Lo, small_bs);
    a.sub_ri(X10, X20, 16);
    a.and_ri(X11, X10, 3);
    a.add_ri(X11, X11, 5);
    a.lsr_ri(X10, X10, 2);
    a.add_ri(X10, X10, 6);
    a.lslv(X13, X11, X10);
    a.b(have_bs);
    a.bind(small_bs);
    a.add_ri(X13, X20, 1);
    a.lsl_ri(X13, X13, 4);
    a.bind(have_bs);
    a.str(X13, X21, 24, 8);
    state(a, X10, 0);
    a.ldr(X11, X10, 16, 8); // the next unused span
    a.ldr(X12, X10, 8, 8);
    a.add_rr(X12, X12, X9); // end of the region
    a.mov_ri(X15, 65536);
    a.add_rr(X14, X11, X15);
    a.cmp_rr(X14, X12);
    a.b_cond(Cond::Hi, oom);
    a.str(X14, X10, 16, 8);
    a.sub_rr(X12, X11, X9);
    a.lsr_ri(X12, X12, 16);
    a.add_rr(X12, X12, X9);
    a.str(X20, X12, SPAN_TABLE, 1); // the span's class
    a.mov_rr(X22, X11);
    a.udiv(X14, X15, X13);
    a.mul(X14, X14, X13);
    a.add_rr(X14, X14, X22);
    a.str(X14, X21, 16, 8);
    a.str(X22, X21, 8, 8);
    a.b(retry);

    // First allocation: reserve the region (halving the request until the system agrees), tell the
    // kernel not to back it with huge pages, and set the state. Returns the base in `x0`.
    a.bind(reginit);
    a.mov_ri(X1, REGION_BYTES);
    a.bind(try_map);
    map_x1(a, Os::Linux, 0x4022);
    failed(a, Os::Linux, X0, shrink);
    a.b(mapped);
    a.bind(shrink);
    a.lsr_ri(X1, X1, 1);
    a.mov_ri(X15, REGION_MIN as i64);
    a.cmp_rr(X1, X15);
    a.b_cond(Cond::Hs, try_map);
    a.b(oom);
    a.bind(mapped);
    a.mov_rr(X6, X0);
    a.mov_ri(X2, 15); // MADV_NOHUGEPAGE
    sys(a, Os::Linux, Sys::Madvise);
    state(a, X10, 0);
    a.str(X6, X10, 0, 8);
    a.str(X1, X10, 8, 8);
    a.lsr_ri(X11, X1, 16);
    a.add_imm(X11, X11, (SPAN_TABLE + 65535) as i64);
    a.and_ri(X11, X11, !65535u64);
    a.add_rr(X11, X11, X6);
    a.str(X11, X10, 16, 8); // the first span follows the tables
    a.mov_rr(X0, X6);
    a.ret();
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
    emit_for(a, ro, data, main, leak_check, os, Uses::ALL)
}

/// Like [`emit`], with only the routines `uses` names (the labels of the others are never bound, so
/// nothing may call them).
pub fn emit_for(
    a: &mut Asm,
    ro: &mut Blob,
    data: &mut Blob,
    main: Label,
    leak_check: bool,
    os: Os,
    uses: Uses,
) -> Runtime {
    // `realloc` allocates and frees; `print` and `realloc` copy.
    let alloc_used = uses.alloc || uses.realloc;
    let free_used = uses.free || uses.realloc;
    let copy_used = uses.memcpy || uses.print || uses.realloc || uses.os;
    let win = os == Os::Windows;
    let mac = os == Os::MacOs;
    // Windows and macOS have no `mremap`: a large block reserves eight times its length of address space
    // (a power of two) and grows inside it; the header word 1 holds the used length with the base-2
    // logarithm of the reservation in its low six bits.
    let rsv = win || mac;
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
        ctx_init: a.new_label(),
        ctx_switch: a.new_label(),
        os: [(); 14].map(|_| a.new_label()),
    };
    let flush = a.new_label();
    let fault = a.new_label();
    let tramp = a.new_label();
    let oom = a.new_label();
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
    a.mov_rr(FP, XZR);
    // A big stack of our own, taken lazily, so deep recursion works like it does in the
    // interpreter; if the mapping fails, the stack the system gave us stays.
    let keep = a.new_label();
    let stack = if win { STACK_WIN } else { STACK };
    mmap(a, os, stack, 0x4022); // Linux: private, anonymous, no reservation
    failed(a, os, X0, keep);
    a.mov_rr(X19, X0);
    if os == Os::Linux {
        // Big frames (a 40 MB array in a local) fault in 2 MiB at a time when the kernel agrees. The
        // top 2 MiB stay on small pages, so a small program does not fault in a huge page at start.
        a.mov_ri(X1, STACK - STACK_SMALL);
        a.mov_ri(X2, 14); // MADV_HUGEPAGE
        sys(a, os, Sys::Madvise);
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
    if uses.print {
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
            a.sub_ri(SP, SP, 80);
            a.mov_ri(X0, 1);
            a.mov_ri(X1, if mac { 0x4048_7413 } else { 0x5401 }); // TIOCGETA, TCGETS
            a.mov_from_sp(X2);
            sys(a, os, Sys::Ioctl);
            a.add_ri(SP, SP, 80);
            a.cbnz(X0, piped);
        }
        state(a, X9, 128);
        a.str(XZR, X9, 0, 8);
        a.bind(piped);
    }

    // Guard against running off the stack, and report stray addresses as stops.
    let noguard = a.new_label();
    state(a, X9, 144);
    a.ldr(X0, X9, 0, 8);
    a.cbz(X0, noguard);
    if win {
        a.add_imm(X0, X0, GUARD_WIN);
        a.mov_ri(X1, 4096);
        a.mov_ri(X2, 0x104); // read and write, guard page
        state(a, X3, 160);
        win_call(a, IMP_VIRTUAL_PROTECT);
    } else {
        a.mov_ri(X1, GUARD);
        a.mov_ri(X2, 0);
        sys(a, os, Sys::Mprotect);
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
    } else if mac {
        mmap(a, os, ALT_STACK, 0x22);
        failed(a, os, X0, nosig);
        a.sub_ri(SP, SP, 64);
        a.str(X0, SP, 0, 8); // stack_t { sp, size, flags }
        a.mov_ri(X9, ALT_STACK);
        a.str(X9, SP, 8, 8);
        a.str(XZR, SP, 16, 8);
        a.mov_from_sp(X0);
        a.mov_ri(X1, 0);
        sys(a, os, Sys::Sigaltstack);
        // struct __sigaction { handler, trampoline, mask: u32, flags: u32 }. The kernel calls the
        // trampoline with the handler and the signal's details; it calls the handler on the alternate
        // stack, with siginfo.
        a.lea_label(X9, fault);
        a.str(X9, SP, 32, 8);
        a.lea_label(X9, tramp);
        a.str(X9, SP, 40, 8);
        a.mov_ri(X9, 0x41i64 << 32); // mask 0; SA_ONSTACK | SA_SIGINFO
        a.str(X9, SP, 48, 8);
        for sig in [11, 10] {
            // SIGSEGV, SIGBUS
            a.mov_ri(X0, sig);
            a.add_ri(X1, SP, 32);
            a.mov_ri(X2, 0);
            sys(a, os, Sys::Sigaction);
        }
        a.add_ri(SP, SP, 64);
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
        sys(a, os, Sys::Sigaltstack);
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
            sys(a, os, Sys::Sigaction);
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

    // ---- rtExit -----------------------------------------------------------------------------------
    a.bind(rt.exit);
    a.mov_rr(X19, X0);
    if uses.print {
        a.bl(flush);
    }
    a.mov_rr(X0, X19);
    if win {
        win_call(a, IMP_EXIT_PROCESS);
    } else {
        sys(a, os, Sys::Exit);
    }
    a.brk();

    if uses.print {
        // ---- rtFlush ----------------------------------------------------------------------------------
        // Writes what the output buffer holds. Ends in `rtWrite` or a `ret`.
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

        // ---- rtPrint ----------------------------------------------------------------------------------
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
    }

    // ---- rtWrite ----------------------------------------------------------------------------------
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
        a.mov_rr(X10, X1); // macOS overwrites x1
        sys(a, os, Sys::Write);
        a.mov_rr(X1, X10);
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

    if leak_check || uses.write_u64 {
        // ---- rtWriteU64 ------------------------------------------------------------------------------
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
    }

    if copy_used {
        // ---- rtMemcpy ---------------------------------------------------------------------------------
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
    }

    // ---- rtStop -----------------------------------------------------------------------------------
    a.bind(rt.stop);
    a.mov_rr(X19, X0);
    a.mov_rr(X20, X1);
    if uses.print {
        a.bl(flush); // what the program printed comes first
    }
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
    // `x0` is the signal, `x1` the siginfo.
    {
        let (overflow, invalid, check) = (a.new_label(), a.new_label(), a.new_label());
        a.bind(fault);
        if win {
            // `x0` the exception pointers: the record's code, and for an access violation the
            // address at +0x28. Anything else is passed on (CONTINUE_SEARCH, 0).
            let (pass, tripped) = (a.new_label(), a.new_label());
            a.ldr(X9, X0, 0, 8);
            a.ldr(X10, X9, 0, 4);
            a.mov_ri(X11, 0x8000_0001); // guard page violation
            a.cmp_rr(X10, X11);
            a.b_cond(Cond::Eq, tripped);
            a.mov_ri(X11, 0xC000_00FD); // stack overflow
            a.cmp_rr(X10, X11);
            a.b_cond(Cond::Eq, overflow);
            a.mov_ri(X11, 0xC000_0005); // access violation
            a.cmp_rr(X10, X11);
            a.b_cond(Cond::Eq, check);
            a.bind(pass);
            a.mov_ri(X0, 0);
            a.ret();
            // Our stack's guard page: an overflow when the address is in the stack, else not ours.
            a.bind(tripped);
            a.ldr(X9, X9, 0x28, 8);
            state(a, X10, 144);
            a.ldr(X11, X10, 0, 8);
            a.cbz(X11, pass);
            a.sub_rr(X9, X9, X11);
            a.mov_ri(X12, STACK_WIN);
            a.cmp_rr(X9, X12);
            a.b_cond(Cond::Lo, overflow);
            a.b(pass);
            a.bind(check);
            a.ldr(X9, X9, 0x28, 8);
        } else {
            // siginfo: the faulting address is at +16 (Linux) or +24 (macOS).
            a.ldr(X9, X1, if mac { 24 } else { 16 }, 8);
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

    // ---- the signal trampoline (macOS) -------------------------------------------------------------
    // The kernel calls it with the handler in `x0`, then the style of the call, the signal, the siginfo
    // and the context; the handler gets the last three. It never returns (it ends in `stop`).
    if mac {
        a.bind(tramp);
        a.mov_rr(X9, X0);
        a.mov_rr(X0, X2);
        a.mov_rr(X1, X3);
        a.mov_rr(X2, X4);
        a.blr(X9);
        a.brk();
    }

    if alloc_used {
        // ---- out of memory -----------------------------------------------------------------------------
        a.bind(oom);
        a.lea_sect(X0, Sect::Rodata, msg_oom);
        a.mov_ri(X1, 13);
        a.bl(rt.stop);

        // ---- rtAlloc ----------------------------------------------------------------------------------
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
        if os == Os::Linux {
            emit_region_alloc(a, large, oom);
        } else {
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
        }
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
        if rsv {
            a.and_ri(X12, X12, !4095);
        }
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
        if rsv {
            // x20 the logarithm of the reservation: the length rounded up to a power of two, times eight.
            a.sub_ri(X13, X21, 1);
            a.clz(X13, X13);
            a.mov_ri(X9, 64);
            a.sub_rr(X13, X9, X13);
            a.add_ri(X20, X13, 3);
            a.cmp_ri(X20, 60);
            a.b_cond(Cond::Hi, oom);
            a.mov_ri(X9, 1);
            a.lslv(X12, X9, X20);
            if win {
                // Reserve it all, commit the part in use.
                a.mov_ri(X0, 0);
                a.mov_rr(X1, X12);
                a.mov_ri(X2, 0x2000);
                a.mov_ri(X3, 1);
                win_call(a, IMP_VIRTUAL_ALLOC);
                a.cbz(X0, oom);
                a.mov_rr(X22, X0);
                a.mov_rr(X1, X21);
                a.mov_ri(X2, 0x1000);
                a.mov_ri(X3, 4);
                win_call(a, IMP_VIRTUAL_ALLOC);
                a.cbz(X0, oom);
            } else {
                // Untouched pages cost nothing, so the whole reservation is mapped read-write.
                a.mov_rr(X1, X12);
                map_x1(a, os, 0x4022);
                failed(a, os, X0, oom);
                a.mov_rr(X22, X0);
            }
            a.mov_ri(X9, LARGE_CLASS);
            a.str(X9, X22, 0, 8);
            a.orr_rr(X9, X21, X20);
            a.str(X9, X22, 8, 8);
        } else {
            a.mov_rr(X1, X21);
            map_x1(a, os, 0x22);
            failed(a, os, X0, oom);
            a.mov_rr(X22, X0);
            advise_huge(a, os, X22, X21);
            a.mov_ri(X9, LARGE_CLASS);
            a.str(X9, X22, 0, 8);
            a.str(X21, X22, 8, 8);
        }
        a.bind(out);
        a.add_ri(X0, X22, 16);
        state(a, X10, 112);
        a.ldr(X9, X10, 0, 8);
        a.add_ri(X9, X9, 1);
        a.str(X9, X10, 0, 8);
        leave(a);
        a.ret();
    }

    if uses.os {
        emit_os(a, &rt, os, flush, uses.print, alloc_used && free_used);
    }

    if uses.ctx {
        // ---- rtCtxSwitch / rtCtxInit --------------------------------------------------------------
        // A saved context is a stack pointer with 96 bytes above it: x19 to x28, x29, x30 (the return address).
        a.bind(rt.ctx_switch);
        a.stp_pre(X19, X20, SP, -96);
        a.stp(X21, X22, SP, 16);
        a.stp(X23, X24, SP, 32);
        a.stp(X25, X26, SP, 48);
        a.stp(X27, X28, SP, 64);
        a.stp(FP, LR, SP, 80);
        a.mov_from_sp(X9);
        a.str(X9, X0, 0, 8);
        a.mov_to_sp(X1);
        a.ldp(FP, LR, SP, 80);
        a.ldp(X27, X28, SP, 64);
        a.ldp(X25, X26, SP, 48);
        a.ldp(X23, X24, SP, 32);
        a.ldp(X21, X22, SP, 16);
        a.ldp_post(X19, X20, SP, 96);
        a.ret();

        let ctramp = a.new_label();
        a.bind(rt.ctx_init);
        a.add_rr(X9, X0, X1);
        a.mov_ri(X10, -16);
        a.and_rr(X9, X9, X10);
        a.sub_ri(X9, X9, 96);
        for k in 0..6 {
            a.stp(XZR, XZR, X9, 16 * k);
        }
        a.str(X3, X9, 0, 8); // x19: the environment
        a.str(X2, X9, 8, 8); // x20: the code
        a.lea_label(X10, ctramp);
        a.str(X10, X9, 88, 8); // x30: where the first switch returns
        a.mov_rr(X0, X9);
        a.ret();
        a.bind(ctramp);
        a.mov_rr(X0, X19);
        a.blr(X20);
        a.brk();
    }

    if free_used {
        // ---- rtFree -----------------------------------------------------------------------------------
        a.bind(rt.free);
        {
            let (fdone, flarge, unmap) = (a.new_label(), a.new_label(), a.new_label());
            a.cbz(X0, fdone);
            if os == Os::Linux {
                // A block inside the region: its span's class says which free list takes it back.
                let outside = a.new_label();
                state(a, X12, 0);
                a.ldr(X13, X12, 0, 8);
                a.sub_rr(X14, X0, X13);
                a.ldr(X15, X12, 8, 8);
                a.cmp_rr(X14, X15);
                a.b_cond(Cond::Hs, outside);
                a.lsr_ri(X14, X14, 16);
                a.add_rr(X14, X14, X13);
                a.ldr(X14, X14, SPAN_TABLE, 1);
                a.lsl_ri(X14, X14, 5);
                a.add_rr(X14, X14, X13);
                a.ldr(X15, X14, 0, 8);
                a.str(X15, X0, 0, 8);
                a.str(X0, X14, 0, 8);
                state(a, X12, 112);
                a.ldr(X13, X12, 0, 8);
                a.sub_ri(X13, X13, 1);
                a.str(X13, X12, 0, 8);
                a.ret();
                a.bind(outside);
                a.sub_ri(X10, X0, 16);
                state(a, X12, 112);
                a.ldr(X13, X12, 0, 8);
                a.sub_ri(X13, X13, 1);
                a.str(X13, X12, 0, 8);
                a.b(flarge);
            } else {
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
            }
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
                if mac {
                    // The length is the reservation: 1 << (the logarithm in the low six bits).
                    a.and_ri(X3, X1, 63);
                    a.mov_ri(X1, 1);
                    a.lslv(X1, X1, X3);
                }
                a.mov_rr(X0, X10);
                sys(a, os, Sys::Munmap);
            }
            a.ret();
        }
    }

    if uses.realloc {
        // ---- rtRealloc --------------------------------------------------------------------------------
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
            if os == Os::Linux {
                // A block inside the region is moved by the generic path.
                state(a, X12, 0);
                a.ldr(X13, X12, 0, 8);
                a.sub_rr(X14, X19, X13);
                a.ldr(X15, X12, 8, 8);
                a.cmp_rr(X14, X15);
                a.b_cond(Cond::Lo, generic);
            } else {
                a.ldr(X11, X10, 0, 8);
                a.cmp_ri(X11, LARGE_CLASS as u32);
                a.b_cond(Cond::Ne, generic);
            }
            if rsv {
                // Growth inside the reservation: the pages are already there (macOS) or are committed now
                // (Windows), and the new tail is still zero. Shrinking and outgrowing the reservation copy.
                a.add_imm(X12, X20, 16 + 4095);
                a.and_ri(X12, X12, !4095);
                a.ldr(X13, X10, 8, 8);
                a.and_ri(X14, X13, !4095); // used length
                a.cmp_rr(X12, X14);
                a.b_cond(Cond::Eq, rdone);
                a.b_cond(Cond::Lo, generic);
                a.and_ri(X15, X13, 63);
                a.mov_ri(X9, 1);
                a.lslv(X9, X9, X15); // reservation
                a.cmp_rr(X12, X9);
                a.b_cond(Cond::Hi, generic);
                a.orr_rr(X21, X12, X15); // the new header word
                a.mov_rr(X22, X10);
                if win {
                    a.add_rr(X0, X10, X14);
                    a.sub_rr(X1, X12, X14);
                    a.mov_ri(X2, 0x1000);
                    a.mov_ri(X3, 4);
                    win_call(a, IMP_VIRTUAL_ALLOC);
                    a.cbz(X0, oom);
                }
                a.str(X21, X22, 8, 8);
                a.b(rdone);
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
                sys(a, os, Sys::Mremap);
                failed(a, os, X0, oom);
                a.str(X21, X0, 8, 8);
                a.mov_rr(X22, X0);
                advise_huge(a, os, X22, X21);
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
            if os == Os::Linux {
                // What the old block holds: its class's block size, or its mapping less the header.
                state(a, X10, 0);
                a.ldr(X13, X10, 0, 8);
                a.sub_rr(X14, X19, X13);
                a.ldr(X15, X10, 8, 8);
                a.cmp_rr(X14, X15);
                a.b_cond(Cond::Hs, rclass);
                a.lsr_ri(X14, X14, 16);
                a.add_rr(X14, X14, X13);
                a.ldr(X14, X14, SPAN_TABLE, 1);
                a.lsl_ri(X14, X14, 5);
                a.add_rr(X14, X14, X13);
                a.ldr(X12, X14, 24, 8);
                a.b(rcopy);
                a.bind(rclass);
                a.ldr(X12, X19, -8, 8);
                a.sub_ri(X12, X12, 16);
                a.bind(rcopy);
            } else {
                a.ldr(X11, X19, -16, 8);
                a.cmp_ri(X11, LARGE_CLASS as u32);
                a.b_cond(Cond::Ne, rclass);
                a.ldr(X12, X19, -8, 8);
                if rsv {
                    a.and_ri(X12, X12, !4095);
                }
                a.b(rcopy);
                a.bind(rclass);
                a.mov_ri(X12, 32);
                a.lslv(X12, X12, X11);
                a.bind(rcopy);
                a.sub_ri(X12, X12, 16);
            }
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
    }

    if uses.fmod {
        // ---- rtFmod -----------------------------------------------------------------------------------
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
    }
    rt
}

/// Windows: `WriteFile` or `ReadFile` on descriptor `x0` (0, 1, 2: the standard handles; others are handles
/// from `open`), buffer `x1`, count `x2`, the frame already entered. The count transferred, or -3; a read
/// from a pipe whose writer is gone (error 109) is the end of the input, 0.
fn win_std_io(a: &mut Asm, import: usize, read: bool) {
    let (raw, have, fail, out) = (a.new_label(), a.new_label(), a.new_label(), a.new_label());
    a.sub_ri(SP, SP, 16);
    a.mov_rr(X20, X1);
    a.mov_rr(X21, X2);
    a.cmp_ri(X0, 3);
    a.b_cond(Cond::Ge, raw);
    a.mov_ri(X9, -10);
    a.sub_rr(X0, X9, X0);
    win_call(a, IMP_GET_STD_HANDLE);
    a.b(have);
    a.bind(raw);
    a.bind(have);
    a.mov_rr(X1, X20);
    a.mov_rr(X2, X21);
    a.mov_from_sp(X3);
    a.mov_ri(X4, 0); // no overlapped I/O
    win_call(a, import);
    a.uxt(X0, X0, 32); // a BOOL
    a.cbz(X0, fail);
    a.ldr(X0, SP, 0, 4);
    a.b(out);
    a.bind(fail);
    if read {
        let eof = a.new_label();
        win_call(a, IMP_GET_LAST_ERROR);
        a.cmp_ri(X0, 109);
        a.b_cond(Cond::Eq, eof);
        a.mov_ri(X0, -3);
        a.b(out);
        a.bind(eof);
        a.mov_ri(X0, 0);
    } else {
        a.mov_ri(X0, -3);
    }
    a.bind(out);
    a.add_ri(SP, SP, 16);
    leave(a);
    a.ret();
}

/// Windows `open(path = x0, mode = x1)` with the frame entered: the zero-terminated UTF-8 path becomes UTF-16
/// in a block of its own, then `CreateFileW` (permissive sharing). The handle, or -4 (not found), -5
/// (denied) or -3. `x19` the path, `x20` the mode, `x21` its length, `x22` the block.
fn win_open(a: &mut Asm, rt: &Runtime) {
    let (sl, sd) = (a.new_label(), a.new_label());
    a.mov_rr(X19, X0);
    a.mov_rr(X20, X1);
    a.mov_ri(X21, 0);
    a.bind(sl);
    a.add_rr(X10, X19, X21);
    a.ldr(X9, X10, 0, 1);
    a.cbz(X9, sd);
    a.add_ri(X21, X21, 1);
    a.b(sl);
    a.bind(sd);
    a.lsl_ri(X0, X21, 1);
    a.add_ri(X0, X0, 2);
    a.bl(rt.alloc);
    a.mov_rr(X22, X0);
    a.mov_ri(X0, 65001); // CP_UTF8
    a.mov_ri(X1, 0);
    a.mov_rr(X2, X19);
    a.mov_ri(X3, -1);
    a.mov_rr(X4, X22);
    a.add_ri(X5, X21, 1);
    win_call(a, IMP_MULTI_BYTE);
    let (converted, created, failed, noent, acces, out) = (
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
    );
    a.uxt(X0, X0, 32);
    a.cbnz(X0, converted);
    a.mov_ri(X19, -1);
    a.b(created);
    a.bind(converted);
    let (rd, wr, go) = (a.new_label(), a.new_label(), a.new_label());
    a.cbz(X20, rd);
    a.cmp_ri(X20, 1);
    a.b_cond(Cond::Eq, wr);
    a.mov_ri(X1, 4); // FILE_APPEND_DATA
    a.mov_ri(X4, 4); // OPEN_ALWAYS
    a.b(go);
    a.bind(wr);
    a.mov_ri(X1, 0x4000_0000); // GENERIC_WRITE
    a.mov_ri(X4, 2); // CREATE_ALWAYS
    a.b(go);
    a.bind(rd);
    a.mov_ri(X1, 0x8000_0000); // GENERIC_READ
    a.mov_ri(X4, 3); // OPEN_EXISTING
    a.bind(go);
    a.mov_rr(X0, X22);
    a.mov_ri(X2, 7); // share read, write, delete
    a.mov_ri(X3, 0);
    a.mov_ri(X5, 0x80); // FILE_ATTRIBUTE_NORMAL
    a.mov_ri(X6, 0);
    win_call(a, IMP_CREATE_FILE);
    a.mov_rr(X19, X0);
    a.bind(created);
    win_call(a, IMP_GET_LAST_ERROR);
    a.mov_rr(X20, X0);
    a.mov_rr(X0, X22);
    a.bl(rt.free);
    a.adds_ri(XZR, X19, 1); // the handle is -1 on failure
    a.b_cond(Cond::Eq, failed);
    a.mov_rr(X0, X19);
    a.b(out);
    a.bind(failed);
    a.cmp_ri(X20, 2);
    a.b_cond(Cond::Eq, noent);
    a.cmp_ri(X20, 3);
    a.b_cond(Cond::Eq, noent);
    a.cmp_ri(X20, 5);
    a.b_cond(Cond::Eq, acces);
    a.mov_ri(X0, -3);
    a.b(out);
    a.bind(noent);
    a.mov_ri(X0, -4);
    a.b(out);
    a.bind(acces);
    a.mov_ri(X0, -5);
    a.bind(out);
    leave(a);
    a.ret();
}

/// `madvise(addr, len, MADV_HUGEPAGE)` for a big mapping; `addr` and `len` registers are kept.
fn advise_huge(a: &mut Asm, os: Os, addr: Reg, len: Reg) {
    let skip = a.new_label();
    a.mov_ri(X9, HUGE_MIN);
    a.cmp_rr(len, X9);
    a.b_cond(Cond::Lo, skip);
    a.mov_rr(X0, addr);
    a.mov_rr(X1, len);
    a.mov_ri(X2, 14);
    sys(a, os, Sys::Madvise);
    a.bind(skip);
}

/// The OS memory calls (docs/design/os-memory.md), in a function of their own: `emit_for` is already a huge
/// frame in debug builds.
#[inline(never)]
fn emit_os(a: &mut Asm, rt: &Runtime, os: Os, flush: Label, print: bool, heap: bool) {
    let win = os == Os::Windows;
    // ---- the OS memory calls (docs/design/os-memory.md) ----------------------------------------------
    // Sizes and addresses must be whole pages (else -2). A system error becomes -1 (no memory), -2
    // (invalid argument) or -3. Windows routines keep a frame (the imports lose the link register).
    let (bad, nomem, other, os_error) =
        (a.new_label(), a.new_label(), a.new_label(), a.new_label());
    let mac = os == Os::MacOs;
    let finish = |a: &mut Asm| {
        if win {
            leave(a);
        }
        a.ret();
    };
    let size_ok = |a: &mut Asm, r: Reg, bad: Label| {
        a.cmp_ri(r, 0);
        a.b_cond(Cond::Le, bad);
        assert!(a.tst_ri(r, 4095));
        a.b_cond(Cond::Ne, bad);
    };
    let aligned = |a: &mut Asm, r: Reg, bad: Label| {
        assert!(a.tst_ri(r, 4095));
        a.b_cond(Cond::Ne, bad);
    };
    let start = |a: &mut Asm, l: Label| {
        a.bind(l);
        if win {
            enter(a);
        }
    };
    // reserve(size)
    start(a, rt.os[0]);
    size_ok(a, X0, bad);
    if win {
        a.mov_rr(X1, X0);
        a.mov_ri(X0, 0);
        a.mov_ri(X2, 0x2000); // reserve
        a.mov_ri(X3, 1); // no access
        win_call(a, IMP_VIRTUAL_ALLOC);
        a.cbz(X0, nomem);
        finish(a);
    } else {
        a.mov_rr(X1, X0);
        a.mov_ri(X0, 0);
        a.mov_ri(X2, 0); // no access
        a.mov_ri(X3, if mac { 0x1042 } else { 0x4022 });
        a.mov_ri(X4, -1);
        a.mov_ri(X5, 0);
        sys(a, os, Sys::Mmap);
        a.b(os_error);
    }
    // commit(addr, size)
    start(a, rt.os[1]);
    aligned(a, X0, bad);
    size_ok(a, X1, bad);
    if win {
        a.mov_ri(X2, 0x1000); // commit
        a.mov_ri(X3, 4); // read and write
        win_call(a, IMP_VIRTUAL_ALLOC);
        a.cbz(X0, other);
        a.mov_ri(X0, 0);
        finish(a);
    } else {
        a.mov_ri(X2, 3);
        sys(a, os, Sys::Mprotect);
        a.b(os_error);
    }
    // release(addr, size)
    start(a, rt.os[2]);
    aligned(a, X0, bad);
    size_ok(a, X1, bad);
    if win {
        a.mov_ri(X1, 0);
        a.mov_ri(X2, 0x8000); // release
        win_call(a, IMP_VIRTUAL_FREE);
        a.cbz(X0, other);
        a.mov_ri(X0, 0);
        finish(a);
    } else {
        sys(a, os, Sys::Munmap);
        a.b(os_error);
    }
    // remap(addr, old, new)
    start(a, rt.os[3]);
    aligned(a, X0, bad);
    size_ok(a, X1, bad);
    size_ok(a, X2, bad);
    if win || mac {
        // No kernel remap: a new block, the smaller size copied, the old one released.
        // x19 the old block, x20 its size, x21 the new size, x22 the new block.
        if !win {
            enter(a);
        }
        let (fail, done) = (a.new_label(), a.new_label());
        a.mov_rr(X19, X0);
        a.mov_rr(X20, X1);
        a.mov_rr(X21, X2);
        a.mov_rr(X1, X21);
        map_x1(a, os, 0x22);
        failed(a, os, X0, fail);
        a.mov_rr(X22, X0);
        a.mov_rr(X1, X19);
        a.mov_rr(X2, X20);
        a.cmp_rr(X2, X21);
        let keep = a.new_label();
        a.b_cond(Cond::Ls, keep);
        a.mov_rr(X2, X21);
        a.bind(keep);
        a.bl(rt.memcpy);
        a.mov_rr(X0, X19);
        if win {
            a.mov_ri(X1, 0);
            a.mov_ri(X2, 0x8000);
            win_call(a, IMP_VIRTUAL_FREE);
        } else {
            a.mov_rr(X1, X20);
            sys(a, os, Sys::Munmap);
        }
        a.mov_rr(X0, X22);
        a.b(done);
        a.bind(fail);
        a.mov_ri(X0, -1);
        a.bind(done);
        leave(a);
        a.ret();
    } else {
        a.mov_ri(X3, 1); // may move
        sys(a, os, Sys::Mremap);
        a.b(os_error);
    }
    // advise_nohuge(addr, size)
    start(a, rt.os[4]);
    aligned(a, X0, bad);
    size_ok(a, X1, bad);
    if win || mac {
        a.mov_ri(X0, 0);
        finish(a);
    } else {
        a.mov_ri(X2, 15); // MADV_NOHUGEPAGE
        sys(a, os, Sys::Madvise);
        a.b(os_error);
    }
    // ---- the process calls: write(fd, ptr, len), read(fd, ptr, len), exit(status), argv(), open(path, mode),
    // close(fd). Results: a count or descriptor, or a negative error. ---------------------------------------
    // write flushes standard output first, so output keeps its order.
    start(a, rt.os[5]);
    if print {
        if win {
            // `enter` already pushed a frame; keep the arguments in callee-saved registers across the flush.
            a.mov_rr(X19, X0);
            a.mov_rr(X20, X1);
            a.mov_rr(X21, X2);
            a.bl(flush);
            a.mov_rr(X0, X19);
            a.mov_rr(X1, X20);
            a.mov_rr(X2, X21);
        } else {
            a.stp_pre(X0, X1, SP, -32);
            a.stp(X2, LR, SP, 16);
            a.bl(flush);
            a.ldp(X2, LR, SP, 16);
            a.ldp_post(X0, X1, SP, 32);
        }
    }
    if win {
        win_std_io(a, IMP_WRITE_FILE, false);
        start(a, rt.os[6]);
        win_std_io(a, IMP_READ_FILE, true);
    } else {
        // One call, retried when interrupted (-4).
        for (l, call, first) in [(rt.os[5], Sys::Write, true), (rt.os[6], Sys::Read, false)] {
            if !first {
                a.bind(l);
            }
            let retry = a.new_label();
            a.mov_rr(X9, X0);
            a.mov_rr(X10, X1);
            a.mov_rr(X11, X2);
            a.bind(retry);
            a.mov_rr(X0, X9);
            a.mov_rr(X1, X10);
            a.mov_rr(X2, X11);
            sys(a, os, call);
            a.adds_ri(XZR, X0, 4);
            a.b_cond(Cond::Eq, retry);
            a.ret();
        }
    }
    start(a, rt.os[7]);
    a.b(rt.exit);
    // argv: no start block yet on the AArch64 targets; programs that ask for it are refused at build time.
    start(a, rt.os[8]);
    a.mov_ri(X0, 0);
    finish(a);
    // open(path, mode): mode 0 reads, 1 writes (create, empty), else appends (create).
    start(a, rt.os[9]);
    if win && !heap {
        // No allocator in this program (it never opens a file): nothing to convert the path with.
        a.mov_ri(X0, -3);
        finish(a);
    } else if win {
        win_open(a, rt);
    } else {
        let (rd, wr, go) = (a.new_label(), a.new_label(), a.new_label());
        a.mov_rr(X4, X0);
        a.cbz(X1, rd);
        a.cmp_ri(X1, 1);
        a.b_cond(Cond::Eq, wr);
        a.mov_ri(X2, if mac { 0x209 } else { 0x441 }); // write only, create, append
        a.b(go);
        a.bind(wr);
        a.mov_ri(X2, if mac { 0x601 } else { 0x241 }); // write only, create, truncate
        a.b(go);
        a.bind(rd);
        a.mov_ri(X2, 0);
        a.bind(go);
        if mac {
            a.mov_rr(X0, X4);
            a.mov_rr(X1, X2);
            a.mov_ri(X2, 0o666);
        } else {
            a.mov_ri(X0, -100); // AT_FDCWD
            a.mov_rr(X1, X4);
            a.mov_ri(X3, 0o666);
        }
        sys(a, os, Sys::Open);
        let (done, noent, acces) = (a.new_label(), a.new_label(), a.new_label());
        a.mov_ri(X15, -4095);
        a.cmp_rr(X0, X15);
        a.b_cond(Cond::Lo, done);
        a.mov_ri(X15, -2);
        a.cmp_rr(X0, X15);
        a.b_cond(Cond::Eq, noent);
        a.mov_ri(X15, -13);
        a.cmp_rr(X0, X15);
        a.b_cond(Cond::Eq, acces);
        a.mov_ri(X0, -3);
        a.ret();
        a.bind(noent);
        a.mov_ri(X0, -4);
        a.ret();
        a.bind(acces);
        a.mov_ri(X0, -5);
        a.bind(done);
        a.ret();
    }
    // close(fd)
    start(a, rt.os[10]);
    if win {
        win_call(a, IMP_CLOSE_HANDLE);
        let bad_close = a.new_label();
        a.uxt(X0, X0, 32); // a BOOL
        a.cbz(X0, bad_close);
        a.mov_ri(X0, 0);
        finish(a);
        a.bind(bad_close);
        a.mov_ri(X0, -3);
        finish(a);
    } else {
        sys(a, os, Sys::Close);
        a.ret();
    }
    // clock(which, timespec): `clock_gettime` on Linux; -3 (no clock call yet) on Windows and macOS.
    start(a, rt.os[11]);
    if win {
        a.mov_ri(X0, -3);
        finish(a);
    } else if os == Os::MacOs {
        a.mov_ri(X0, -3);
        a.ret();
    } else {
        sys(a, os, Sys::ClockGettime);
        a.ret();
    }
    // fs and dents: no file-system calls on the 64-bit Arm targets yet (-3).
    for i in [12, 13] {
        start(a, rt.os[i]);
        if win {
            a.mov_ri(X0, -3);
            finish(a);
        } else {
            a.mov_ri(X0, -3);
            a.ret();
        }
    }
    // The exits.
    a.bind(bad);
    a.mov_ri(X0, -2);
    finish(a);
    a.bind(nomem);
    a.mov_ri(X0, -1);
    finish(a);
    a.bind(other);
    a.mov_ri(X0, -3);
    finish(a);
    a.bind(os_error);
    // Linux and macOS: `x0` is the result, or -errno in the last 4095 values.
    let ok = a.new_label();
    a.mov_ri(X15, -4095);
    a.cmp_rr(X0, X15);
    a.b_cond(Cond::Lo, ok);
    a.mov_ri(X15, -12);
    a.cmp_rr(X0, X15);
    a.b_cond(Cond::Eq, nomem);
    a.mov_ri(X15, -22);
    a.cmp_rr(X0, X15);
    a.b_cond(Cond::Eq, bad);
    a.b(other);
    a.bind(ok);
    a.ret();
}
