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
//! | 0, 8, 16 | Linux: the base and size of the allocator's region (0 until the first allocation) and its next unused span (`docs/design/heap.md`) |
//! | 0 to 95 | Windows: heads of the twelve free lists (block sizes 32 B to 64 KiB) |
//! | 96, 104 | Windows: next free byte and end of the current chunk |
//! | 112 | live blocks (for the leak check) |
//! | 120 | the one freed large block kept for reuse (0 when none) |
//! | 128 | the output buffer ([`PRINT_BUF`] bytes, mapped at start; 0 when the mapping failed) |
//! | 136 | bytes waiting in the output buffer |
//! | 144 | the base (lowest address) of our own stack, 0 if the system stack is used |
//! | 152 | Windows: the lowest committed address of our stack (it grows downwards on demand) |
//! | 160 | scratch for `VirtualProtect` (Windows) |
//! | 168 | Linux: the kernel's start block (the stack pointer at `_start`: argument count, arguments, environment) |

use super::elf::Blob;
use super::x86::*;
use super::{Os, Uses};

pub const STATE_SIZE: usize = 176;
/// Size of the buffer that `print` fills; it is written out when full, at exit and at a stop.
const PRINT_BUF: i64 = 64 * 1024;
const CHUNK: i64 = 1 << 20;
const STACK: i64 = 1 << 30;
/// The top of the stack that is never advised to use huge pages (Linux).
const STACK_SMALL: i64 = 2 << 20;
/// The stack on Windows is reserved up front and committed on demand.
const STACK_WIN: i64 = 1 << 28;
/// What is committed at start, and how much more each guard page hit commits.
const COMMIT_WIN: i64 = 512 * 1024;

/// The kernel32 functions the Windows image imports, in the order of its import table.
pub const IMPORTS: [&str; 25] = [
    "GetStdHandle",
    "WriteFile",
    "ExitProcess",
    "VirtualAlloc",
    "VirtualFree",
    "AddVectoredExceptionHandler",
    "VirtualProtect",
    "GetConsoleMode",
    "ReadFile",
    "CreateFileW",
    "CloseHandle",
    "MultiByteToWideChar",
    "GetLastError",
    "GetSystemTimePreciseAsFileTime",
    "QueryPerformanceCounter",
    "QueryPerformanceFrequency",
    "GetCommandLineW",
    "GetEnvironmentStringsW",
    "WideCharToMultiByte",
    "GetFileAttributesExW",
    "DeleteFileW",
    "RemoveDirectoryW",
    "CreateDirectoryW",
    "MoveFileExW",
    // From advapi32.dll (the last import; see `pe.rs`).
    "SystemFunction036",
];
pub(super) const IMP_WALL_TIME: usize = 13;
pub(super) const IMP_PERF_COUNTER: usize = 14;
pub(super) const IMP_PERF_FREQUENCY: usize = 15;
pub(super) const IMP_COMMAND_LINE: usize = 16;
pub(super) const IMP_ENVIRONMENT: usize = 17;
pub(super) const IMP_WIDE_CHAR: usize = 18;
pub(super) const IMP_FILE_ATTRIBUTES: usize = 19;
pub(super) const IMP_DELETE_FILE: usize = 20;
pub(super) const IMP_REMOVE_DIRECTORY: usize = 21;
pub(super) const IMP_CREATE_DIRECTORY: usize = 22;
pub(super) const IMP_MOVE_FILE: usize = 23;
pub(super) const IMP_RTL_GEN_RANDOM: usize = 24;
pub(super) const IMP_GET_STD_HANDLE: usize = 0;
pub(super) const IMP_WRITE_FILE: usize = 1;
pub(super) const IMP_EXIT_PROCESS: usize = 2;
pub(super) const IMP_VIRTUAL_ALLOC: usize = 3;
pub(super) const IMP_VIRTUAL_FREE: usize = 4;
pub(super) const IMP_ADD_VEH: usize = 5;
pub(super) const IMP_VIRTUAL_PROTECT: usize = 6;
pub(super) const IMP_GET_CONSOLE_MODE: usize = 7;
pub(super) const IMP_READ_FILE: usize = 8;
pub(super) const IMP_CREATE_FILE: usize = 9;
pub(super) const IMP_CLOSE_HANDLE: usize = 10;
pub(super) const IMP_MULTI_BYTE: usize = 11;
pub(super) const IMP_GET_LAST_ERROR: usize = 12;

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
/// Like [`win_call`] with room for seven arguments (three beyond the registers).
fn win_call7(a: &mut Asm, import: usize, setup: impl FnOnce(&mut Asm)) {
    a.push(RBP);
    a.mov_rr(RBP, RSP, 8);
    a.alu_ri(Alu::And, RSP, -16, 8);
    a.alu_ri(Alu::Sub, RSP, 64, 8);
    setup(a);
    a.call_mem_sect(Sect::Idata, super::pe::iat_slot(import));
    a.mov_rr(RSP, RBP, 8);
    a.pop(RBP);
}
/// The largest block served from the free lists: header plus payload.
const SMALL_MAX: i32 = 64 * 1024;
const LARGE_CLASS: i32 = 255;
/// Windows: a large block of at least this many bytes reserves eight times its size up front and
/// commits pages as it grows, so `rtRealloc` extends it in place (there is no `mremap`).
const RESERVE_MIN: i32 = 1 << 20;

/// Linux: the largest payload served from the region; bigger blocks get their own mapping.
pub(super) const REGION_MAX: i32 = 8192;
/// Linux: the region is asked for this many bytes of address space (nothing is resident until used)
/// and the request is halved until the system agrees, down to `REGION_MIN`.
pub(super) const REGION_BYTES: i64 = 1 << 36;
pub(super) const REGION_MIN: i32 = 1 << 24;
/// Linux: the region starts with the class table (`CLASSES` entries of 32 bytes: free list, bump
/// pointer, bump limit, block size) and then the span table at this offset (one byte per 64 KiB span).
pub(super) const SPAN_TABLE: i32 = 2048;

/// Emits the Linux small-block allocator for `rtAlloc` (payload size in `rdi`, `rbx` and `r12` saved
/// by the caller): classes 16 to 256 bytes in steps of 16, then four per doubling up to
/// [`REGION_MAX`]; blocks come from 64 KiB spans bump-carved out of one reserved region, with no header.
/// A reused block is cleared; a carved one is zero already. Ends with `ret`; larger sizes jump to `large`.
fn emit_region_alloc(a: &mut Asm, large: Label, oom: Label) {
    let (big, classed, have_region, retry, bump, zero, done, refill) = (
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
    );
    let (grow, grown, small_bs, have_bs, reginit, try_map, mapped) = (
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
        a.new_label(),
    );
    a.alu_ri(Alu::Cmp, RDI, REGION_MAX, 8);
    a.jcc(Cc::A, large);
    a.alu_ri(Alu::Cmp, RDI, 256, 8);
    a.jcc(Cc::A, big);
    // Up to 256 bytes: class (n + 15) / 16 - 1, at least 0.
    a.lea(RAX, mem(RDI, 15));
    a.shift_imm(Shift::Shr, RAX, 4, 8);
    a.mov_ri(RCX, 1);
    a.alu_rr(Alu::Cmp, RAX, RCX, 8);
    a.cmov(Cc::B, RAX, RCX, 8);
    a.lea(R12, mem(RAX, -1));
    a.jmp(classed);
    // Above: with m = n - 1 and e = the position of its top bit, class 4 * e - 16 + the next two bits.
    a.bind(big);
    a.lea(RDX, mem(RDI, -1));
    a.mov_ri(RCX, 8);
    a.mov_ri(RAX, 512);
    a.bind(grow);
    a.alu_rr(Alu::Cmp, RDX, RAX, 8);
    a.jcc(Cc::B, grown);
    a.alu_ri(Alu::Add, RCX, 1, 8);
    a.alu_rr(Alu::Add, RAX, RAX, 8);
    a.jmp(grow);
    a.bind(grown);
    a.mov_rr(R12, RCX, 8);
    a.shift_imm(Shift::Shl, R12, 2, 8);
    a.alu_ri(Alu::Sub, R12, 16, 8);
    a.alu_ri(Alu::Sub, RCX, 2, 8);
    a.shift_cl(Shift::Shr, RDX, 8);
    a.alu_ri(Alu::And, RDX, 3, 8);
    a.alu_rr(Alu::Add, R12, RDX, 8);

    a.bind(classed);
    state(a, R10, 0);
    a.load(R9, mem(R10, 0), 8); // the region's base, 0 before the first allocation
    a.test_rr(R9, R9, 8);
    a.jcc(Cc::Ne, have_region);
    a.call(reginit);
    a.mov_rr(R9, RAX, 8);
    a.bind(have_region);
    a.mov_rr(RSI, R12, 8);
    a.shift_imm(Shift::Shl, RSI, 5, 8);
    a.alu_rr(Alu::Add, RSI, R9, 8); // this class's entry
    a.bind(retry);
    a.load(RDX, mem(RSI, 0), 8);
    a.test_rr(RDX, RDX, 8);
    a.jcc(Cc::E, bump);
    // Reuse: unlink, then clear the whole block.
    a.load(RCX, mem(RDX, 0), 8);
    a.store(mem(RSI, 0), RCX, 8);
    a.load(RCX, mem(RSI, 24), 8);
    a.mov_rr(RAX, RDX, 8);
    a.bind(zero);
    a.store_imm(mem(RDX, 0), 0, 8);
    a.store_imm(mem(RDX, 8), 0, 8);
    a.alu_ri(Alu::Add, RDX, 16, 8);
    a.alu_ri(Alu::Sub, RCX, 16, 8);
    a.jcc(Cc::Ne, zero);
    a.jmp(done);
    // Fresh: carve from the current span (zero already); a class that has no span yet has block size 0.
    a.bind(bump);
    a.load(RAX, mem(RSI, 8), 8);
    a.load(RCX, mem(RSI, 24), 8);
    a.test_rr(RCX, RCX, 8);
    a.jcc(Cc::E, refill);
    a.lea(RDX, mem_idx(RAX, RCX, 1, 0));
    a.alu_rm(Alu::Cmp, RDX, mem(RSI, 16), 8);
    a.jcc(Cc::A, refill);
    a.store(mem(RSI, 8), RDX, 8);
    a.bind(done);
    state(a, RSI, 112);
    a.alu_mi(Alu::Add, mem(RSI, 0), 1, 8);
    a.pop(R12);
    a.pop(RBX);
    a.ret();

    // Take the next 64 KiB span for this class: block size 16 * (k + 1) below class 16, else
    // (5 + (k & 3)) << (6 + ((k - 16) >> 2)).
    a.bind(refill);
    a.alu_ri(Alu::Cmp, R12, 16, 8);
    a.jcc(Cc::B, small_bs);
    a.lea(RDX, mem(R12, -16));
    a.mov_rr(RAX, RDX, 8);
    a.alu_ri(Alu::And, RAX, 3, 8);
    a.alu_ri(Alu::Add, RAX, 5, 8);
    a.shift_imm(Shift::Shr, RDX, 2, 8);
    a.alu_ri(Alu::Add, RDX, 6, 8);
    a.mov_rr(RCX, RDX, 8);
    a.shift_cl(Shift::Shl, RAX, 8);
    a.mov_rr(RCX, RAX, 8);
    a.jmp(have_bs);
    a.bind(small_bs);
    a.lea(RCX, mem(R12, 1));
    a.shift_imm(Shift::Shl, RCX, 4, 8);
    a.bind(have_bs);
    a.store(mem(RSI, 24), RCX, 8);
    state(a, R10, 0);
    a.load(RAX, mem(R10, 16), 8); // the next unused span
    a.load(R8, mem(R10, 8), 8);
    a.alu_rr(Alu::Add, R8, R9, 8); // end of the region
    a.lea(RDX, mem(RAX, 65536));
    a.alu_rr(Alu::Cmp, RDX, R8, 8);
    a.jcc(Cc::A, oom);
    a.store(mem(R10, 16), RDX, 8);
    a.mov_rr(R8, RAX, 8);
    a.alu_rr(Alu::Sub, R8, R9, 8);
    a.shift_imm(Shift::Shr, R8, 16, 8);
    a.alu_rr(Alu::Add, R8, R9, 8);
    a.store(mem(R8, SPAN_TABLE), R12, 1); // the span's class
    a.mov_rr(R10, RAX, 8);
    a.mov_ri(RAX, 65536);
    a.alu_rr(Alu::Xor, RDX, RDX, 4);
    a.div(RCX, 8);
    a.imul_rr(RAX, RCX, 8);
    a.alu_rr(Alu::Add, RAX, R10, 8);
    a.store(mem(RSI, 16), RAX, 8);
    a.store(mem(RSI, 8), R10, 8);
    a.jmp(retry);

    // First allocation: reserve the region (halving the request until the system agrees), tell the
    // kernel not to back it with huge pages (a first touch would then cost 2 MiB), and set the state.
    a.bind(reginit);
    a.mov_ri(RSI, REGION_BYTES);
    a.bind(try_map);
    a.mov_ri(RAX, 9);
    a.alu_rr(Alu::Xor, RDI, RDI, 4);
    a.mov_ri(RDX, 3);
    a.mov_ri(R10, 0x4022); // private, anonymous, no reservation
    a.mov_ri(R8, -1);
    a.alu_rr(Alu::Xor, R9, R9, 4);
    a.syscall();
    a.alu_ri(Alu::Cmp, RAX, -4095, 8);
    a.jcc(Cc::B, mapped);
    a.shift_imm(Shift::Shr, RSI, 1, 8);
    a.alu_ri(Alu::Cmp, RSI, REGION_MIN, 8);
    a.jcc(Cc::Ae, try_map);
    a.jmp(oom);
    a.bind(mapped);
    a.push(RAX);
    a.push(RSI);
    a.mov_rr(RDI, RAX, 8);
    a.mov_ri(RDX, 15); // MADV_NOHUGEPAGE
    a.mov_ri(RAX, 28);
    a.syscall();
    a.pop(RSI);
    a.pop(RAX);
    state(a, R9, 0);
    a.store(mem(R9, 0), RAX, 8);
    a.store(mem(R9, 8), RSI, 8);
    a.mov_rr(RDX, RSI, 8);
    a.shift_imm(Shift::Shr, RDX, 16, 8);
    a.alu_ri(Alu::Add, RDX, SPAN_TABLE + 65535, 8);
    a.alu_ri(Alu::And, RDX, -65536, 8);
    a.alu_rr(Alu::Add, RDX, RAX, 8);
    a.store(mem(R9, 16), RDX, 8); // the first span follows the tables
    a.ret();
}

/// The entry points of the runtime.
pub struct Runtime {
    /// `rtWrite(fd = rdi, ptr = rsi, len = rdx)`: writes everything or gives up on an error.
    pub write_fd: Label,
    /// `rtPrint(ptr = rsi, len = rdx)`: appends to the output buffer; clobbers the argument registers.
    pub print: Label,
    /// `rtStopAt(ptr = rdi, len = rsi, location = rdx)`: `rtStop` that also says where (a location, `Span::at`: the
    /// file number over the line; 0 for nowhere): `file:line: stopped: <message>`.
    pub stop_at: Label,
    /// `rtStop(ptr = rdi, len = rsi)`: prints `stopped: <message>` to standard error and exits with status 1.
    pub stop: Label,
    /// `rtAlloc(size = rdi) -> rax`: zeroed memory, 16-byte aligned; never fails (stops when out of memory).
    pub alloc: Label,
    /// `rtFree(ptr = rdi)`: accepts null.
    pub free: Label,
    /// `rtRealloc(ptr = rdi, size = rsi) -> rax`: a block of the new size holding the old contents (null
    /// allocates); the rest is zero. A large block is remapped, so growing a big buffer copies nothing.
    pub realloc: Label,
    /// `rtExit(status = rdi)`
    pub exit: Label,
    /// `rtWriteU64(fd = rdi, value = rsi)`: decimal digits.
    pub write_u64: Label,
    /// `rtCtxInit(base = rdi, size = rsi, code = rdx, env = rcx) -> rax`: a context on that stack that
    /// starts by calling `code(env)`; the result is what `rtCtxSwitch` takes.
    pub ctx_init: Label,
    /// `rtCtxSwitch(save = rdi, to = rsi)`: saves the callee-saved registers on this stack, stores the
    /// stack pointer at `save`, and continues the context `to`.
    pub ctx_switch: Label,
    /// The operating system memory calls, by `OsKind` order (reserve, commit, release, remap, advise):
    /// arguments in `rdi`, `rsi`, `rdx`, the result in `rax` (an address or 0, or a negative error).
    pub os: [Label; 22],
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
    emit_for(a, ro, data, main, leak_check, target, Uses::ALL, &[])
}

/// Like [`emit`], with only the routines `uses` names (the labels of the others are never bound, so
/// nothing may call them).
#[allow(clippy::too_many_arguments)]
pub fn emit_for(
    a: &mut Asm,
    ro: &mut Blob,
    data: &mut Blob,
    main: Label,
    leak_check: bool,
    target: Os,
    uses: Uses,
    files: &[String],
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
        stop_at: a.new_label(),
        alloc: a.new_label(),
        free: a.new_label(),
        realloc: a.new_label(),
        exit: a.new_label(),
        write_u64: a.new_label(),
        ctx_init: a.new_label(),
        ctx_switch: a.new_label(),
        os: [(); 22].map(|_| a.new_label()),
    };
    let flush = a.new_label();
    let fault = a.new_label();
    let msg_stopped = ro.add(b"stopped: ", 1);
    // The names of the files that a stop can name, one length byte and the name each, in file-number order (an
    // empty name for a file no stop names).
    let files_at = if files.iter().any(|f| !f.is_empty()) {
        let mut table = Vec::new();
        for f in files {
            let n = f.len().min(255);
            table.push(n as u8);
            table.extend_from_slice(&f.as_bytes()[..n]);
        }
        Some(ro.add(&table, 1))
    } else {
        None
    };
    let (msg_colon, msg_colsp) = (ro.add(b":", 1), ro.add(b": ", 1));
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
    if uses.os && !win {
        // The kernel's start block: argument count, arguments, environment (offset 168 of the state).
        state(a, RCX, 168);
        a.store(mem(RCX, 0), RSP, 8);
    }
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
        // Big frames (a 40 MB array in a local) fault in 2 MiB at a time when the kernel agrees. The
        // top 2 MiB stay on small pages: a program that only uses a few KiB of stack would otherwise
        // fault in a whole huge page on its first call.
        a.push(RAX);
        a.mov_rr(RDI, RAX, 8);
        a.mov_ri(RSI, STACK - STACK_SMALL);
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
            a.mov_ri(RAX, 13); // rtSigaction(sig, &act, 0, 8)
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

    // ---- rtExit -----------------------------------------------------------------------------------
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
        // ---- rtFlush ----------------------------------------------------------------------------------
        // Writes what the output buffer holds. Falls through nowhere: it ends in `rtWrite` or a `ret`.
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

        // ---- rtPrint ----------------------------------------------------------------------------------
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

    // ---- rtWrite ----------------------------------------------------------------------------------
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

    if leak_check || uses.write_u64 || files_at.is_some() {
        // ---- rtWriteU64 ------------------------------------------------------------------------------
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

    // ---- rtStop -----------------------------------------------------------------------------------
    a.bind(rt.stop);
    a.alu_rr(Alu::Xor, RDX, RDX, 4);
    a.bind(rt.stop_at);
    a.mov_rr(R12, RDI, 8);
    a.mov_rr(R13, RSI, 8);
    a.mov_rr(R14, RDX, 8);
    if uses.print {
        a.call(flush); // what the program printed comes first
    }
    if let Some(files_at) = files_at {
        // `file:line: ` first, when the stop knows where it happened: the file's name is the `rax`-th entry of
        // the table (a length byte, then the name), the line is the low 20 bits of the location.
        let (skip, scan, found) = (a.new_label(), a.new_label(), a.new_label());
        a.test_rr(R14, R14, 8);
        a.jcc(Cc::E, skip);
        a.mov_rr(RAX, R14, 8);
        a.shift_imm(Shift::Shr, RAX, 20, 8);
        a.lea_sect(RSI, Sect::Rodata, files_at);
        a.bind(scan);
        a.alu_ri(Alu::Sub, RAX, 1, 8);
        a.jcc(Cc::E, found);
        a.load_ext(RCX, mem(RSI, 0), 1, false, true);
        a.lea(RSI, mem_idx(RSI, RCX, 1, 1));
        a.jmp(scan);
        a.bind(found);
        a.load_ext(RDX, mem(RSI, 0), 1, false, true);
        a.alu_ri(Alu::Add, RSI, 1, 8);
        a.mov_ri(RDI, 2);
        a.call(rt.write_fd);
        a.mov_ri(RDI, 2);
        a.lea_sect(RSI, Sect::Rodata, msg_colon);
        a.mov_ri(RDX, 1);
        a.call(rt.write_fd);
        a.mov_ri(RDI, 2);
        a.mov_rr(RSI, R14, 8);
        a.alu_ri(Alu::And, RSI, 0xF_FFFF, 8);
        a.call(rt.write_u64);
        a.mov_ri(RDI, 2);
        a.lea_sect(RSI, Sect::Rodata, msg_colsp);
        a.mov_ri(RDX, 2);
        a.call(rt.write_fd);
        a.bind(skip);
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

        // ---- rtAlloc ----------------------------------------------------------------------------------
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
        if !win {
            emit_region_alloc(a, large, oom);
        }
        if win {
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
        }
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

    if uses.os {
        emit_os(a, &rt, win, flush, uses.print, alloc_used && free_used);
    }

    if uses.ctx {
        // ---- rtCtxSwitch / rtCtxInit --------------------------------------------------------------
        // A saved context is a stack pointer with six registers above it, then the return address:
        // r15, r14, r13, r12, rbp, rbx, ret.
        a.bind(rt.ctx_switch);
        for r in [RBX, RBP, R12, R13, R14, R15] {
            a.push(r);
        }
        a.store(mem(RDI, 0), RSP, 8);
        a.mov_rr(RSP, RSI, 8);
        for r in [R15, R14, R13, R12, RBP, RBX] {
            a.pop(r);
        }
        a.ret();

        let tramp = a.new_label();
        a.bind(rt.ctx_init);
        a.mov_rr(RAX, RDI, 8);
        a.alu_rr(Alu::Add, RAX, RSI, 8);
        a.alu_ri(Alu::And, RAX, -16, 8);
        a.lea_label(R8, tramp);
        a.store(mem(RAX, -8), R8, 8);
        a.store(mem(RAX, -16), RCX, 8); // rbx: the environment
        a.store_imm(mem(RAX, -24), 0, 8); // rbp: 0 ends frame chains
        a.store(mem(RAX, -32), RDX, 8); // r12: the code
        a.alu_ri(Alu::Sub, RAX, 56, 8);
        a.ret();
        // The first switch into a context returns here with the stack 16-byte aligned.
        a.bind(tramp);
        a.mov_rr(RDI, RBX, 8);
        a.call_reg(R12);
        a.ud2();
    }

    if free_used {
        // ---- rtFree -----------------------------------------------------------------------------------
        a.bind(rt.free);
        let (fdone, flarge) = (a.new_label(), a.new_label());
        a.test_rr(RDI, RDI, 8);
        a.jcc(Cc::E, fdone);
        if win {
            a.lea(RDX, mem(RDI, -16));
            a.load(RAX, mem(RDX, 0), 8);
            a.extend_rr(RAX, RAX, 1, false, false); // the class byte
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
        } else {
            // A block inside the region: its span's class says which free list takes it back.
            let outside = a.new_label();
            state(a, RSI, 0);
            a.load(RDX, mem(RSI, 0), 8);
            a.mov_rr(RAX, RDI, 8);
            a.alu_rr(Alu::Sub, RAX, RDX, 8);
            a.alu_rm(Alu::Cmp, RAX, mem(RSI, 8), 8);
            a.jcc(Cc::Ae, outside);
            a.shift_imm(Shift::Shr, RAX, 16, 8);
            a.alu_rr(Alu::Add, RAX, RDX, 8);
            a.load_ext(RAX, mem(RAX, SPAN_TABLE), 1, false, true);
            a.shift_imm(Shift::Shl, RAX, 5, 8);
            a.alu_rr(Alu::Add, RAX, RDX, 8);
            a.load(RCX, mem(RAX, 0), 8);
            a.store(mem(RDI, 0), RCX, 8);
            a.store(mem(RAX, 0), RDI, 8);
            state(a, RSI, 112);
            a.alu_mi(Alu::Sub, mem(RSI, 0), 1, 8);
            a.ret();
            a.bind(outside);
            a.lea(RDX, mem(RDI, -16));
            state(a, RSI, 112);
            a.alu_mi(Alu::Sub, mem(RSI, 0), 1, 8);
            a.jmp(flarge);
        }
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
        // ---- rtRealloc --------------------------------------------------------------------------------
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
        if win {
            a.lea(RDX, mem(RBX, -16));
            a.load(RAX, mem(RDX, 0), 8);
            a.extend_rr(RAX, RAX, 1, false, false);
            a.alu_ri(Alu::Cmp, RAX, LARGE_CLASS, 8);
            a.jcc(Cc::Ne, generic);
        } else {
            // A block inside the region is moved by the generic path.
            state(a, RSI, 0);
            a.load(RAX, mem(RSI, 0), 8);
            a.mov_rr(RCX, RBX, 8);
            a.alu_rr(Alu::Sub, RCX, RAX, 8);
            a.alu_rm(Alu::Cmp, RCX, mem(RSI, 8), 8);
            a.jcc(Cc::B, generic);
            a.lea(RDX, mem(RBX, -16));
        }
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
        if win {
            a.load(RAX, mem(RBX, -16), 8);
            a.extend_rr(RAX, RAX, 1, false, false);
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
        } else {
            // What the old block holds: its class's block size, or its mapping less the header.
            let (rlarge, rcap) = (a.new_label(), a.new_label());
            state(a, RSI, 0);
            a.load(RAX, mem(RSI, 0), 8);
            a.mov_rr(RCX, RBX, 8);
            a.alu_rr(Alu::Sub, RCX, RAX, 8);
            a.alu_rm(Alu::Cmp, RCX, mem(RSI, 8), 8);
            a.jcc(Cc::Ae, rlarge);
            a.shift_imm(Shift::Shr, RCX, 16, 8);
            a.alu_rr(Alu::Add, RCX, RAX, 8);
            a.load_ext(RCX, mem(RCX, SPAN_TABLE), 1, false, true);
            a.shift_imm(Shift::Shl, RCX, 5, 8);
            a.alu_rr(Alu::Add, RCX, RAX, 8);
            a.load(RCX, mem(RCX, 24), 8);
            a.jmp(rcap);
            a.bind(rlarge);
            a.load(RCX, mem(RBX, -8), 8);
            a.alu_ri(Alu::Sub, RCX, 16, 8);
            a.bind(rcap);
        }
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

/// The OS memory calls (docs/design/os-memory.md), in a function of their own: `emit_for` is already a huge
/// frame in debug builds.
#[inline(never)]
fn emit_os(a: &mut Asm, rt: &Runtime, win: bool, flush: Label, print: bool, heap: bool) {
    // ---- the OS memory calls (docs/design/os-memory.md) ----------------------------------------------
    // Sizes and addresses must be whole pages (else -2). A system error becomes -1 (no memory), -2
    // (invalid argument) or -3. Callers treat every register but the callee-saved ones as lost.
    let (bad, nomem, other) = (a.new_label(), a.new_label(), a.new_label());
    // The shared exits: `bad` returns -2, `os_error` maps the negative errno in `rax` (Linux) and
    // `other` returns -3.
    let os_error = a.new_label();
    let page_size = |a: &mut Asm, r: Reg, bad: Label| {
        // A size: positive and a multiple of 4096.
        a.alu_ri(Alu::Cmp, r, 0, 8);
        a.jcc(Cc::Le, bad);
        a.mov_rr(RAX, r, 8);
        a.alu_ri(Alu::And, RAX, 4095, 8);
        a.jcc(Cc::Ne, bad);
    };
    let page_aligned = |a: &mut Asm, r: Reg, bad: Label| {
        a.mov_rr(RAX, r, 8);
        a.alu_ri(Alu::And, RAX, 4095, 8);
        a.jcc(Cc::Ne, bad);
    };
    // reserve(size)
    a.bind(rt.os[0]);
    page_size(a, RDI, bad);
    if win {
        a.mov_rr(RDX, RDI, 8);
        a.alu_rr(Alu::Xor, RCX, RCX, 4);
        a.mov_ri(R8, 0x2000); // reserve
        a.mov_ri(R9, 1); // no access
        win_call(a, IMP_VIRTUAL_ALLOC, |_| {});
        a.test_rr(RAX, RAX, 8);
        a.jcc(Cc::E, nomem);
        a.ret();
    } else {
        a.mov_rr(RSI, RDI, 8);
        a.mov_ri(RAX, 9);
        a.alu_rr(Alu::Xor, RDI, RDI, 4);
        a.alu_rr(Alu::Xor, RDX, RDX, 4); // PROT_NONE
        a.mov_ri(R10, 0x4022); // private, anonymous, no reserve
        a.mov_ri(R8, -1);
        a.alu_rr(Alu::Xor, R9, R9, 4);
        a.syscall();
        a.jmp(os_error);
    }
    // commit(addr, size)
    a.bind(rt.os[1]);
    page_aligned(a, RDI, bad);
    page_size(a, RSI, bad);
    if win {
        a.mov_rr(RCX, RDI, 8);
        a.mov_rr(RDX, RSI, 8);
        a.mov_ri(R8, 0x1000); // commit
        a.mov_ri(R9, 4); // read and write
        win_call(a, IMP_VIRTUAL_ALLOC, |_| {});
        a.test_rr(RAX, RAX, 8);
        a.jcc(Cc::E, other);
        a.alu_rr(Alu::Xor, RAX, RAX, 4);
        a.ret();
    } else {
        a.mov_ri(RDX, 3);
        a.mov_ri(RAX, 10);
        a.syscall();
        a.jmp(os_error);
    }
    // release(addr, size)
    a.bind(rt.os[2]);
    page_aligned(a, RDI, bad);
    page_size(a, RSI, bad);
    if win {
        a.mov_rr(RCX, RDI, 8);
        a.alu_rr(Alu::Xor, RDX, RDX, 4);
        a.mov_ri(R8, 0x8000); // release
        win_call(a, IMP_VIRTUAL_FREE, |_| {});
        a.test_rr(RAX, RAX, 8);
        a.jcc(Cc::E, other);
        a.alu_rr(Alu::Xor, RAX, RAX, 4);
        a.ret();
    } else {
        a.mov_ri(RAX, 11);
        a.syscall();
        a.jmp(os_error);
    }
    // remap(addr, old, new)
    a.bind(rt.os[3]);
    page_aligned(a, RDI, bad);
    page_size(a, RSI, bad);
    page_size(a, RDX, bad);
    if win {
        // No kernel remap: a new block, the smaller size copied, the old one released.
        a.push(RBX);
        a.push(R12);
        a.push(R13);
        a.mov_rr(RBX, RDI, 8);
        a.mov_rr(R12, RSI, 8);
        a.mov_rr(R13, RDX, 8);
        a.alu_rr(Alu::Xor, RCX, RCX, 4);
        a.mov_rr(RDX, R13, 8);
        a.mov_ri(R8, 0x3000); // commit and reserve
        a.mov_ri(R9, 4);
        win_call(a, IMP_VIRTUAL_ALLOC, |_| {});
        let fail = a.new_label();
        a.test_rr(RAX, RAX, 8);
        a.jcc(Cc::E, fail);
        a.mov_rr(RDI, RAX, 8);
        a.mov_rr(RSI, RBX, 8);
        a.mov_rr(RCX, R12, 8);
        a.alu_rr(Alu::Cmp, RCX, R13, 8);
        let keep = a.new_label();
        a.jcc(Cc::Be, keep);
        a.mov_rr(RCX, R13, 8);
        a.bind(keep);
        a.mov_rr(R12, RAX, 8); // the new block
        a.rep_movsb();
        a.mov_rr(RCX, RBX, 8);
        a.alu_rr(Alu::Xor, RDX, RDX, 4);
        a.mov_ri(R8, 0x8000);
        win_call(a, IMP_VIRTUAL_FREE, |_| {});
        a.mov_rr(RAX, R12, 8);
        let done = a.new_label();
        a.jmp(done);
        a.bind(fail);
        a.mov_ri(RAX, -1);
        a.bind(done);
        a.pop(R13);
        a.pop(R12);
        a.pop(RBX);
        a.ret();
    } else {
        a.mov_ri(R10, 1); // may move
        a.mov_ri(RAX, 25);
        a.syscall();
        a.jmp(os_error);
    }
    // advise_nohuge(addr, size)
    a.bind(rt.os[4]);
    page_aligned(a, RDI, bad);
    page_size(a, RSI, bad);
    if win {
        a.alu_rr(Alu::Xor, RAX, RAX, 4);
        a.ret();
    } else {
        a.mov_ri(RDX, 15); // MADV_NOHUGEPAGE
        a.mov_ri(RAX, 28);
        a.syscall();
        a.jmp(os_error);
    }
    // The exits.
    a.bind(bad);
    a.mov_ri(RAX, -2);
    a.ret();
    a.bind(nomem);
    a.mov_ri(RAX, -1);
    a.ret();
    a.bind(other);
    a.mov_ri(RAX, -3);
    a.ret();
    // ---- the process calls (Linux only): write(fd, ptr, len), read(fd, ptr, len), exit(status), argv() ----
    if !win {
        // write: standard output is flushed first, so output keeps its order; one `write`, retried when
        // interrupted. The result is the count or a negative errno.
        a.bind(rt.os[5]);
        if print {
            a.push(RDI);
            a.push(RSI);
            a.push(RDX);
            a.call(flush);
            a.pop(RDX);
            a.pop(RSI);
            a.pop(RDI);
        }
        let retry = a.new_label();
        a.bind(retry);
        a.mov_ri(RAX, 1);
        a.syscall();
        a.alu_ri(Alu::Cmp, RAX, -4, 8);
        a.jcc(Cc::E, retry);
        a.ret();
        a.bind(rt.os[6]);
        let retry = a.new_label();
        a.bind(retry);
        a.mov_ri(RAX, 0);
        a.syscall();
        a.alu_ri(Alu::Cmp, RAX, -4, 8);
        a.jcc(Cc::E, retry);
        a.ret();
        a.bind(rt.os[7]);
        a.jmp(rt.exit);
        a.bind(rt.os[8]);
        state(a, RAX, 168);
        a.load(RAX, mem(RAX, 0), 8);
        a.ret();
        // open(path = rdi, mode = rsi): 0 reads, 1 writes (create, truncate), else appends; permissions 0666
        // before the umask. The descriptor, or -4 (no such file), -5 (denied) or -3 (anything else).
        a.bind(rt.os[9]);
        let (ro, wr, go) = (a.new_label(), a.new_label(), a.new_label());
        a.test_rr(RSI, RSI, 8);
        a.jcc(Cc::E, ro);
        a.alu_ri(Alu::Cmp, RSI, 1, 8);
        a.jcc(Cc::E, wr);
        a.mov_ri(RSI, 0x441); // O_WRONLY | O_CREAT | O_APPEND
        a.jmp(go);
        a.bind(wr);
        a.mov_ri(RSI, 0x241); // O_WRONLY | O_CREAT | O_TRUNC
        a.jmp(go);
        a.bind(ro);
        a.mov_ri(RSI, 0); // O_RDONLY
        a.bind(go);
        a.mov_ri(RDX, 0o666);
        a.mov_ri(RAX, 2);
        a.syscall();
        let (done, noent, acces) = (a.new_label(), a.new_label(), a.new_label());
        a.alu_ri(Alu::Cmp, RAX, -4095, 8);
        a.jcc(Cc::B, done);
        a.alu_ri(Alu::Cmp, RAX, -2, 8);
        a.jcc(Cc::E, noent);
        a.alu_ri(Alu::Cmp, RAX, -13, 8);
        a.jcc(Cc::E, acces);
        a.mov_ri(RAX, -3);
        a.ret();
        a.bind(noent);
        a.mov_ri(RAX, -4);
        a.ret();
        a.bind(acces);
        a.mov_ri(RAX, -5);
        a.bind(done);
        a.ret();
        // close(fd = rdi)
        a.bind(rt.os[10]);
        a.mov_ri(RAX, 3);
        a.syscall();
        a.ret();
        // clock(which = rdi, timespec = rsi): `clock_gettime`, the kernel's `{ seconds, nanoseconds }` is the
        // layout `OsKind::Clock` promises. 0, or a negative errno.
        a.bind(rt.os[11]);
        a.mov_ri(RAX, 228);
        a.syscall();
        a.ret();
        // fs(op = rdi, path = rsi, aux = rdx): the path calls of `OsKind::Fs`, by `op`. 0, a size and kind
        // written at `aux` for `stat`, or -4 (no such file), -5 (denied), -3 (anything else).
        let (acc, unl, rmd, mkd, ren, sta, res) = (
            a.new_label(),
            a.new_label(),
            a.new_label(),
            a.new_label(),
            a.new_label(),
            a.new_label(),
            a.new_label(),
        );
        let (go, noent, acces, done) = (a.new_label(), a.new_label(), a.new_label(), a.new_label());
        a.bind(rt.os[12]);
        a.mov_rr(R8, RDX, 8);
        a.mov_rr(RCX, RDI, 8);
        a.mov_rr(RDI, RSI, 8);
        a.mov_rr(RSI, RDX, 8);
        for (k, l) in [(0, acc), (1, unl), (2, rmd), (3, mkd), (4, ren), (5, sta)] {
            a.alu_ri(Alu::Cmp, RCX, k, 8);
            a.jcc(Cc::E, l);
        }
        a.mov_ri(RAX, -2);
        a.ret();
        a.bind(acc);
        a.mov_ri(RSI, 0); // F_OK
        a.mov_ri(RAX, 21); // access
        a.jmp(go);
        a.bind(unl);
        a.mov_ri(RAX, 87); // unlink
        a.jmp(go);
        a.bind(rmd);
        a.mov_ri(RAX, 84); // rmdir
        a.jmp(go);
        a.bind(mkd);
        a.mov_ri(RSI, 0o777); // before the umask
        a.mov_ri(RAX, 83); // mkdir
        a.jmp(go);
        a.bind(ren);
        a.mov_ri(RAX, 82); // rename
        a.bind(go);
        a.syscall();
        a.jmp(res);
        // stat: the kernel's 144-byte record on the stack; `st_mode` is at 24 and `st_size` at 48.
        a.bind(sta);
        a.alu_ri(Alu::Sub, RSP, 144, 8);
        a.mov_rr(RSI, RSP, 8);
        a.mov_ri(RAX, 4);
        a.syscall();
        let (failed, is_file, is_dir, put) =
            (a.new_label(), a.new_label(), a.new_label(), a.new_label());
        a.test_rr(RAX, RAX, 8);
        a.jcc(Cc::Ne, failed);
        a.load_ext(RDX, mem(RSP, 24), 4, false, false);
        a.alu_ri(Alu::And, RDX, 0xF000, 4);
        a.mov_ri(RCX, 3);
        a.alu_ri(Alu::Cmp, RDX, 0x8000, 4);
        a.jcc(Cc::E, is_file);
        a.alu_ri(Alu::Cmp, RDX, 0x4000, 4);
        a.jcc(Cc::E, is_dir);
        a.jmp(put);
        a.bind(is_file);
        a.mov_ri(RCX, 1);
        a.jmp(put);
        a.bind(is_dir);
        a.mov_ri(RCX, 2);
        a.bind(put);
        a.load(RDX, mem(RSP, 48), 8);
        a.store(mem(R8, 0), RDX, 8);
        a.store(mem(R8, 8), RCX, 8);
        a.alu_ri(Alu::Add, RSP, 144, 8);
        a.alu_rr(Alu::Xor, RAX, RAX, 4);
        a.ret();
        a.bind(failed);
        a.alu_ri(Alu::Add, RSP, 144, 8);
        a.bind(res);
        a.alu_ri(Alu::Cmp, RAX, -4095, 8);
        a.jcc(Cc::B, done);
        a.alu_ri(Alu::Cmp, RAX, -2, 8);
        a.jcc(Cc::E, noent);
        a.alu_ri(Alu::Cmp, RAX, -13, 8);
        a.jcc(Cc::E, acces);
        a.mov_ri(RAX, -3);
        a.ret();
        a.bind(noent);
        a.mov_ri(RAX, -4);
        a.ret();
        a.bind(acces);
        a.mov_ri(RAX, -5);
        a.bind(done);
        a.ret();
        // dents(fd = rdi, buffer = rsi, count = rdx): `getdents64`; the bytes written, 0 at the end, or the
        // errors of `fs`.
        a.bind(rt.os[13]);
        a.mov_ri(RAX, 217);
        a.syscall();
        a.jmp(res);
        // sleep(seconds = rdi, nanoseconds = rsi): `nanosleep` on a timespec built on the stack. An early
        // return (a signal) is not an error: the caller looks at the clock again.
        a.bind(rt.os[15]);
        a.alu_ri(Alu::Sub, RSP, 16, 8);
        a.store(mem(RSP, 0), RDI, 8);
        a.store(mem(RSP, 8), RSI, 8);
        a.mov_rr(RDI, RSP, 8);
        a.alu_rr(Alu::Xor, RSI, RSI, 4);
        a.mov_ri(RAX, 35);
        a.syscall();
        a.alu_ri(Alu::Add, RSP, 16, 8);
        a.alu_rr(Alu::Xor, RAX, RAX, 4);
        a.ret();
        // random(buffer = rdi, count = rsi): `getrandom` with no flags. A count of 1 to 256 is never cut short
        // by a signal once the generator is ready, so anything but the whole count is an error.
        let (rbad, rother) = (a.new_label(), a.new_label());
        a.bind(rt.os[14]);
        a.alu_ri(Alu::Cmp, RSI, 1, 8);
        a.jcc(Cc::L, rbad);
        a.alu_ri(Alu::Cmp, RSI, 256, 8);
        a.jcc(Cc::G, rbad);
        a.mov_rr(R8, RSI, 8);
        a.alu_rr(Alu::Xor, RDX, RDX, 4);
        a.mov_ri(RAX, 318);
        a.syscall();
        a.alu_rr(Alu::Cmp, RAX, R8, 8);
        a.jcc(Cc::Ne, rother);
        a.alu_rr(Alu::Xor, RAX, RAX, 4);
        a.ret();
        a.bind(rbad);
        a.mov_ri(RAX, -2);
        a.ret();
        a.bind(rother);
        a.mov_ri(RAX, -3);
        a.ret();
    }
    // The atomics (the same on every x86-64 target: no system call). Each gives the old value in `rax` and is
    // sequentially consistent, as the `lock` prefix and `xchg` are.
    // cas(address = rdi, expected = rsi, new = rdx)
    a.bind(rt.os[16]);
    a.mov_rr(RAX, RSI, 8);
    a.lock_cmpxchg(mem(RDI, 0), RDX);
    a.ret();
    // add(address = rdi, delta = rsi)
    a.bind(rt.os[17]);
    a.lock_xadd(mem(RDI, 0), RSI);
    a.mov_rr(RAX, RSI, 8);
    a.ret();
    // swap(address = rdi, new = rsi)
    a.bind(rt.os[18]);
    a.xchg_mem(mem(RDI, 0), RSI);
    a.mov_rr(RAX, RSI, 8);
    a.ret();
    if !win {
        // thread_start(code = rdi, arg = rsi, stack_top = rdx): `clone` sharing the address space, files, handlers
        // and thread group. The top of the stack holds, from `stack_top - 8` down, the thread's word (the kernel
        // writes its id there at once and clears it, waking a futex wait, when the thread has ended: the stack is
        // free to go after that), a pad, the code and the argument; the child pops the argument and the code and
        // calls `code(arg)`, then ends only its own thread (`exit`, not `exit_group`).
        let (parent, failed) = (a.new_label(), a.new_label());
        a.bind(rt.os[19]);
        a.alu_ri(Alu::And, RDX, -16, 8);
        a.store(mem(RDX, -24), RDI, 8);
        a.store(mem(RDX, -32), RSI, 8);
        a.lea(RSI, mem(RDX, -32));
        a.lea(R10, mem(RDX, -8));
        a.mov_rr(RDX, R10, 8);
        a.mov_ri(RDI, 0x350F00); // VM, FS, FILES, SIGHAND, THREAD, SYSVSEM, PARENT_SETTID, CHILD_CLEARTID
        a.alu_rr(Alu::Xor, R8, R8, 4);
        a.mov_ri(RAX, 56);
        a.syscall();
        a.test_rr(RAX, RAX, 8);
        a.jcc(Cc::Ne, parent);
        a.pop(RDI);
        a.pop(RAX);
        a.call_reg(RAX);
        a.alu_rr(Alu::Xor, RDI, RDI, 4);
        a.mov_ri(RAX, 60);
        a.syscall();
        a.bind(parent);
        a.alu_ri(Alu::Cmp, RAX, 0, 8);
        a.jcc(Cc::L, failed);
        a.ret();
        a.bind(failed);
        a.mov_ri(RAX, -3);
        a.ret();
        // futex_wait(address = rdi, expected = rsi, nanoseconds = rdx): `futex` with `FUTEX_WAIT` and a
        // relative timeout (none when negative). Every outcome but a failure to call is 0.
        let (forever, go) = (a.new_label(), a.new_label());
        a.bind(rt.os[20]);
        a.alu_ri(Alu::Sub, RSP, 16, 8);
        a.alu_ri(Alu::Cmp, RDX, 0, 8);
        a.jcc(Cc::L, forever);
        a.mov_rr(RAX, RDX, 8);
        a.mov_ri(RCX, 1_000_000_000);
        a.alu_rr(Alu::Xor, RDX, RDX, 4);
        a.div(RCX, 8);
        a.store(mem(RSP, 0), RAX, 8);
        a.store(mem(RSP, 8), RDX, 8);
        a.mov_rr(R10, RSP, 8);
        a.jmp(go);
        a.bind(forever);
        a.alu_rr(Alu::Xor, R10, R10, 4);
        a.bind(go);
        a.mov_rr(RDX, RSI, 8);
        a.mov_ri(RSI, 0); // FUTEX_WAIT (not private: the kernel wakes a thread's word that way)
        a.alu_rr(Alu::Xor, R8, R8, 4);
        a.alu_rr(Alu::Xor, R9, R9, 4);
        a.mov_ri(RAX, 202);
        a.syscall();
        a.alu_ri(Alu::Add, RSP, 16, 8);
        a.alu_rr(Alu::Xor, RAX, RAX, 4);
        a.ret();
        // futex_wake(address = rdi, count = rsi): `FUTEX_WAKE`; gives how many woke.
        a.bind(rt.os[21]);
        a.mov_rr(RDX, RSI, 8);
        a.mov_ri(RSI, 1); // FUTEX_WAKE
        a.mov_ri(RAX, 202);
        a.syscall();
        a.ret();
    } else {
        for i in [19, 20, 21] {
            a.bind(rt.os[i]);
            a.mov_ri(RAX, -3);
            a.ret();
        }
    }
    if win {
        // The handle for descriptor `rdi` in `rax`: 0, 1 and 2 are the standard handles, anything else is a
        // handle from `open`.
        let std_handle = |a: &mut Asm| {
            let (raw, have) = (a.new_label(), a.new_label());
            a.alu_ri(Alu::Cmp, RDI, 3, 8);
            a.jcc(Cc::Ge, raw);
            a.mov_ri(RCX, -10);
            a.alu_rr(Alu::Sub, RCX, RDI, 8);
            win_call(a, IMP_GET_STD_HANDLE, |_| {});
            a.jmp(have);
            a.bind(raw);
            a.mov_rr(RAX, RDI, 8);
            a.bind(have);
        };
        // write(fd = rdi, ptr = rsi, count = rdx): standard output is flushed first; one `WriteFile`; the
        // count written or -3.
        a.bind(rt.os[5]);
        if print {
            a.push(RDI);
            a.push(RSI);
            a.push(RDX);
            a.call(flush);
            a.pop(RDX);
            a.pop(RSI);
            a.pop(RDI);
        }
        for (slot, import) in [(5, IMP_WRITE_FILE), (6, IMP_READ_FILE)] {
            if slot == 6 {
                a.bind(rt.os[6]);
            }
            a.push(RBX);
            a.push(R12);
            a.push(R13);
            a.alu_ri(Alu::Sub, RSP, 16, 8);
            a.mov_rr(R12, RSI, 8);
            a.mov_rr(R13, RDX, 8);
            std_handle(a);
            a.mov_rr(RCX, RAX, 8);
            a.mov_rr(RDX, R12, 8);
            a.mov_rr(R8, R13, 4);
            a.lea(R9, mem(RSP, 0));
            win_call(a, import, |a| {
                a.store_imm(mem(RSP, 32), 0, 8); // no overlapped I/O
            });
            let (fail, out) = (a.new_label(), a.new_label());
            a.test_rr(RAX, RAX, 4);
            a.jcc(Cc::E, fail);
            a.load(RAX, mem(RSP, 0), 4);
            a.jmp(out);
            a.bind(fail);
            if slot == 6 {
                // A pipe whose writer has gone (ERROR_BROKEN_PIPE, 109) is the end of the input.
                let (eof, other_err) = (a.new_label(), a.new_label());
                win_call(a, IMP_GET_LAST_ERROR, |_| {});
                a.alu_ri(Alu::Cmp, RAX, 109, 4);
                a.jcc(Cc::E, eof);
                a.bind(other_err);
                a.mov_ri(RAX, -3);
                a.jmp(out);
                a.bind(eof);
                a.alu_rr(Alu::Xor, RAX, RAX, 4);
            } else {
                a.mov_ri(RAX, -3);
            }
            a.bind(out);
            a.alu_ri(Alu::Add, RSP, 16, 8);
            a.pop(R13);
            a.pop(R12);
            a.pop(RBX);
            a.ret();
        }
        a.bind(rt.os[7]);
        a.jmp(rt.exit);
        // argv: the start-up block, made on the first call and kept at offset 168 of the state. Windows gives the
        // command line as one UTF-16 text, so the block has count -1, the command line in UTF-8 as its one text
        // (`core`'s `rtWinSplit` splits it), a zero, then the environment's `NAME=value` texts (without the
        // `=C:` ones that hold the current directories) and a zero. One `VirtualAlloc` holds it all: the header,
        // room for the environment's pointers (at most one per two bytes of it), then the texts.
        let (made, scan, env_go, env_skip, env_next, env_done) = (
            a.new_label(),
            a.new_label(),
            a.new_label(),
            a.new_label(),
            a.new_label(),
            a.new_label(),
        );
        a.bind(rt.os[8]);
        state(a, RAX, 168);
        a.load(RAX, mem(RAX, 0), 8);
        a.test_rr(RAX, RAX, 8);
        a.jcc(Cc::Ne, made);
        a.push(RBX);
        a.push(R12);
        a.push(R13);
        a.push(R14);
        a.push(R15);
        win_call(a, IMP_COMMAND_LINE, |_| {});
        a.mov_rr(R12, RAX, 8);
        win_call(a, IMP_ENVIRONMENT, |_| {});
        a.mov_rr(R13, RAX, 8);
        // r14: the environment's length in UTF-16 units, up to and with the second zero of the two that end it.
        a.alu_rr(Alu::Xor, R14, R14, 4);
        a.bind(scan);
        a.load_ext(RAX, mem_idx(R13, R14, 2, 0), 2, false, true);
        a.alu_ri(Alu::Add, R14, 1, 8);
        a.test_rr(RAX, RAX, 4);
        a.jcc(Cc::Ne, scan);
        a.load_ext(RAX, mem_idx(R13, R14, 2, 0), 2, false, true);
        a.test_rr(RAX, RAX, 4);
        a.jcc(Cc::Ne, scan);
        a.alu_ri(Alu::Add, R14, 1, 8);
        // rbx and r15: the UTF-8 sizes of the command line (with its zero) and of the environment.
        let size_of = |a: &mut Asm, text: Reg, units: Option<Reg>| {
            a.mov_ri(RCX, 65001); // CP_UTF8
            a.alu_rr(Alu::Xor, RDX, RDX, 4);
            a.mov_rr(R8, text, 8);
            match units {
                Some(u) => a.mov_rr(R9, u, 8),
                None => a.mov_ri(R9, -1),
            }
            win_call7(a, IMP_WIDE_CHAR, |a| {
                for at in [32, 40, 48, 56] {
                    a.store_imm(mem(RSP, at), 0, 8);
                }
            });
        };
        size_of(a, R12, None);
        a.mov_rr(RBX, RAX, 8);
        size_of(a, R13, Some(R14));
        a.mov_rr(R15, RAX, 8);
        // rdi: where the texts start; rsi: the block. Both survive the Windows calls.
        a.mov_rr(RDI, R15, 8);
        a.shift_imm(Shift::Shr, RDI, 1, 8);
        a.lea(RDI, mem(RDI, 2));
        a.shift_imm(Shift::Shl, RDI, 3, 8);
        a.alu_ri(Alu::Add, RDI, 24, 8);
        a.alu_rr(Alu::Xor, RCX, RCX, 4);
        a.lea(RDX, mem_idx(RDI, RBX, 1, 0));
        a.alu_rr(Alu::Add, RDX, R15, 8);
        a.mov_ri(R8, 0x3000); // commit and reserve
        a.mov_ri(R9, 4); // read and write
        win_call(a, IMP_VIRTUAL_ALLOC, |_| {});
        a.mov_rr(RSI, RAX, 8);
        a.store_imm(mem(RSI, 0), -1, 8);
        a.lea(RAX, mem_idx(RSI, RDI, 1, 0));
        a.store(mem(RSI, 8), RAX, 8);
        a.store_imm(mem(RSI, 16), 0, 8);
        // The two conversions, into the texts area.
        let convert = |a: &mut Asm, text: Reg, units: Option<Reg>, skip: Option<Reg>, room: Reg| {
            a.mov_ri(RCX, 65001);
            a.alu_rr(Alu::Xor, RDX, RDX, 4);
            a.mov_rr(R8, text, 8);
            match units {
                Some(u) => a.mov_rr(R9, u, 8),
                None => a.mov_ri(R9, -1),
            }
            win_call7(a, IMP_WIDE_CHAR, |a| {
                a.lea(RAX, mem_idx(RSI, RDI, 1, 0));
                if let Some(k) = skip {
                    a.alu_rr(Alu::Add, RAX, k, 8);
                }
                a.store(mem(RSP, 32), RAX, 8);
                a.store(mem(RSP, 40), room, 8);
                a.store_imm(mem(RSP, 48), 0, 8);
                a.store_imm(mem(RSP, 56), 0, 8);
            });
        };
        convert(a, R12, None, None, RBX);
        convert(a, R13, Some(R14), Some(RBX), R15);
        // The environment's pointers: rcx walks the texts, rdx is the next slot.
        a.lea(RCX, mem_idx(RSI, RDI, 1, 0));
        a.alu_rr(Alu::Add, RCX, RBX, 8);
        a.lea(RDX, mem(RSI, 24));
        a.bind(env_go);
        a.load_ext(RAX, mem(RCX, 0), 1, false, true);
        a.test_rr(RAX, RAX, 4);
        a.jcc(Cc::E, env_done);
        a.alu_ri(Alu::Cmp, RAX, 61, 4); // `=`
        a.jcc(Cc::E, env_skip);
        a.store(mem(RDX, 0), RCX, 8);
        a.alu_ri(Alu::Add, RDX, 8, 8);
        a.bind(env_skip);
        a.bind(env_next);
        a.load_ext(RAX, mem(RCX, 0), 1, false, true);
        a.alu_ri(Alu::Add, RCX, 1, 8);
        a.test_rr(RAX, RAX, 4);
        a.jcc(Cc::Ne, env_next);
        a.jmp(env_go);
        a.bind(env_done);
        a.store_imm(mem(RDX, 0), 0, 8);
        state(a, RAX, 168);
        a.store(mem(RAX, 0), RSI, 8);
        a.mov_rr(RAX, RSI, 8);
        a.pop(R15);
        a.pop(R14);
        a.pop(R13);
        a.pop(R12);
        a.pop(RBX);
        a.bind(made);
        a.ret();
        // open(path = rdi, mode = rsi): the zero-terminated UTF-8 path is converted to UTF-16 in a block of
        // its own (never longer than twice the bytes), then `CreateFileW`: mode 0 reads an existing file, 1
        // creates or empties one, else appends (creating). Sharing is permissive, as on Linux.
        a.bind(rt.os[9]);
        // Without the allocator (a program that never opens a file) there is nothing to convert with.
        if !heap {
            a.mov_ri(RAX, -3);
            a.ret();
        } else {
            a.push(RBX);
            a.push(R12);
            a.push(R13);
            a.push(R14);
            a.mov_rr(RBX, RDI, 8);
            a.mov_rr(R12, RSI, 8);
            a.alu_rr(Alu::Xor, R13, R13, 4);
            let (sl, sd) = (a.new_label(), a.new_label());
            a.bind(sl);
            a.load_ext(RAX, mem_idx(RBX, R13, 1, 0), 1, false, true);
            a.test_rr(RAX, RAX, 4);
            a.jcc(Cc::E, sd);
            a.alu_ri(Alu::Add, R13, 1, 8);
            a.jmp(sl);
            a.bind(sd);
            a.lea(RDI, mem_idx(R13, R13, 1, 2));
            a.call(rt.alloc);
            a.mov_rr(R14, RAX, 8);
            a.mov_ri(RCX, 65001); // CP_UTF8
            a.alu_rr(Alu::Xor, RDX, RDX, 4);
            a.mov_rr(R8, RBX, 8);
            a.mov_ri(R9, -1);
            win_call(a, IMP_MULTI_BYTE, |a| {
                a.store(mem(RSP, 32), R14, 8);
                a.lea(RAX, mem(R13, 1));
                a.store(mem(RSP, 40), RAX, 8);
            });
            let (converted, created, failed, noent, acces, out) = (
                a.new_label(),
                a.new_label(),
                a.new_label(),
                a.new_label(),
                a.new_label(),
                a.new_label(),
            );
            a.test_rr(RAX, RAX, 4);
            a.jcc(Cc::Ne, converted);
            a.mov_ri(RBX, -1);
            a.jmp(created);
            a.bind(converted);
            let (rd, wr, go) = (a.new_label(), a.new_label(), a.new_label());
            a.test_rr(R12, R12, 8);
            a.jcc(Cc::E, rd);
            a.alu_ri(Alu::Cmp, R12, 1, 8);
            a.jcc(Cc::E, wr);
            a.mov_ri(R13, 4); // FILE_APPEND_DATA
            a.mov_ri(R12, 4); // OPEN_ALWAYS
            a.jmp(go);
            a.bind(wr);
            a.mov_ri(R13, 0x4000_0000); // GENERIC_WRITE
            a.mov_ri(R12, 2); // CREATE_ALWAYS
            a.jmp(go);
            a.bind(rd);
            a.mov_ri(R13, 0x8000_0000); // GENERIC_READ
            a.mov_ri(R12, 3); // OPEN_EXISTING
            a.bind(go);
            a.mov_rr(RCX, R14, 8);
            a.mov_rr(RDX, R13, 8);
            a.mov_ri(R8, 7); // share read, write, delete
            a.alu_rr(Alu::Xor, R9, R9, 4);
            win_call7(a, IMP_CREATE_FILE, |a| {
                a.store(mem(RSP, 32), R12, 8);
                a.store_imm(mem(RSP, 40), 0x80, 8); // FILE_ATTRIBUTE_NORMAL
                a.store_imm(mem(RSP, 48), 0, 8);
            });
            a.mov_rr(RBX, RAX, 8);
            a.bind(created);
            // The error number must be read before anything else can change it.
            win_call(a, IMP_GET_LAST_ERROR, |_| {});
            a.mov_rr(R13, RAX, 8);
            a.mov_rr(RDI, R14, 8);
            a.call(rt.free);
            a.alu_ri(Alu::Cmp, RBX, -1, 8);
            a.jcc(Cc::E, failed);
            a.mov_rr(RAX, RBX, 8);
            a.jmp(out);
            a.bind(failed);
            a.alu_ri(Alu::Cmp, R13, 2, 8);
            a.jcc(Cc::E, noent);
            a.alu_ri(Alu::Cmp, R13, 3, 8);
            a.jcc(Cc::E, noent);
            a.alu_ri(Alu::Cmp, R13, 5, 8);
            a.jcc(Cc::E, acces);
            a.mov_ri(RAX, -3);
            a.jmp(out);
            a.bind(noent);
            a.mov_ri(RAX, -4);
            a.jmp(out);
            a.bind(acces);
            a.mov_ri(RAX, -5);
            a.bind(out);
            a.pop(R14);
            a.pop(R13);
            a.pop(R12);
            a.pop(RBX);
            a.ret();
        }
        // close(fd = rdi): 0, or -3.
        a.bind(rt.os[10]);
        a.mov_rr(RCX, RDI, 8);
        win_call(a, IMP_CLOSE_HANDLE, |_| {});
        let bad_close = a.new_label();
        a.test_rr(RAX, RAX, 4);
        a.jcc(Cc::E, bad_close);
        a.alu_rr(Alu::Xor, RAX, RAX, 4);
        a.ret();
        a.bind(bad_close);
        a.mov_ri(RAX, -3);
        a.ret();
        // clock(which = rdi, timespec = rsi): the wall clock from `GetSystemTimePreciseAsFileTime` (100 ns steps
        // since 1601), the monotonic one from the performance counter. `rsi` and `rdi` survive the calls (the
        // Windows convention keeps them).
        let (mono, cbad) = (a.new_label(), a.new_label());
        a.bind(rt.os[11]);
        a.test_rr(RDI, RDI, 8);
        a.jcc(Cc::Ne, mono);
        a.mov_rr(RCX, RSI, 8);
        win_call(a, IMP_WALL_TIME, |_| {});
        a.load(RAX, mem(RSI, 0), 8);
        a.alu_rr(Alu::Xor, RDX, RDX, 4);
        a.mov_ri(RCX, 10_000_000);
        a.div(RCX, 8);
        a.imul_rri(RDX, RDX, 100, 8);
        a.mov_ri(RCX, 11_644_473_600); // seconds from 1601 to 1970
        a.alu_rr(Alu::Sub, RAX, RCX, 8);
        a.store(mem(RSI, 0), RAX, 8);
        a.store(mem(RSI, 8), RDX, 8);
        a.alu_rr(Alu::Xor, RAX, RAX, 4);
        a.ret();
        a.bind(mono);
        a.alu_ri(Alu::Cmp, RDI, 1, 8);
        a.jcc(Cc::Ne, cbad);
        a.lea(RCX, mem(RSI, 8));
        win_call(a, IMP_PERF_FREQUENCY, |_| {});
        a.mov_rr(RCX, RSI, 8);
        win_call(a, IMP_PERF_COUNTER, |_| {});
        a.load(RAX, mem(RSI, 0), 8);
        a.load(R8, mem(RSI, 8), 8);
        a.alu_rr(Alu::Xor, RDX, RDX, 4);
        a.div(R8, 8);
        a.store(mem(RSI, 0), RAX, 8);
        // The remainder is below the frequency, so `remainder * 10^9 / frequency` is below 10^9.
        a.mov_rr(RAX, RDX, 8);
        a.mov_ri(RCX, 1_000_000_000);
        a.mul(RCX, 8);
        a.div(R8, 8);
        a.store(mem(RSI, 8), RAX, 8);
        a.alu_rr(Alu::Xor, RAX, RAX, 4);
        a.ret();
        a.bind(cbad);
        a.mov_ri(RAX, -2);
        a.ret();
        // sleep: no sleep call on Windows yet.
        a.bind(rt.os[15]);
        a.mov_ri(RAX, -3);
        a.ret();
        // fs(op = rdi, path = rsi, aux = rdx): the path calls of `OsKind::Fs` through kernel32, the path (and for
        // `rename` the second one, `aux`) converted to UTF-16 first. 0, a size and kind written at `aux` for `stat`,
        // or -4 (no such file), -5 (denied), -3 (anything else). Without the allocator there is nothing to convert
        // with. `dents` (directory listing) has no Windows form yet.
        if !heap {
            a.bind(rt.os[12]);
            a.mov_ri(RAX, -3);
            a.ret();
        } else {
            let wide = a.new_label();
            let (stat, del, rmd, mkd, ren, fin, fail, ok_exit, bad_op) = (
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
            // wide(utf8 = rdi) -> rax: the UTF-16 copy in a block of its own (never longer than twice the bytes).
            a.bind(wide);
            a.push(RBX);
            a.push(R12);
            a.push(R13);
            a.mov_rr(RBX, RDI, 8);
            a.alu_rr(Alu::Xor, R12, R12, 4);
            let (wl, wd) = (a.new_label(), a.new_label());
            a.bind(wl);
            a.load_ext(RAX, mem_idx(RBX, R12, 1, 0), 1, false, true);
            a.test_rr(RAX, RAX, 4);
            a.jcc(Cc::E, wd);
            a.alu_ri(Alu::Add, R12, 1, 8);
            a.jmp(wl);
            a.bind(wd);
            a.lea(RDI, mem_idx(R12, R12, 1, 2));
            a.call(rt.alloc);
            a.mov_rr(R13, RAX, 8);
            a.mov_ri(RCX, 65001); // CP_UTF8
            a.alu_rr(Alu::Xor, RDX, RDX, 4);
            a.mov_rr(R8, RBX, 8);
            a.mov_ri(R9, -1);
            win_call(a, IMP_MULTI_BYTE, |a| {
                a.store(mem(RSP, 32), R13, 8);
                a.lea(RAX, mem(R12, 1));
                a.store(mem(RSP, 40), RAX, 8);
            });
            a.mov_rr(RAX, R13, 8);
            a.pop(R13);
            a.pop(R12);
            a.pop(RBX);
            a.ret();
            // fs: rbx the path, r14 the second path (`rename`), r13 `aux`, r12 the operation, r15 the `stat` record.
            a.bind(rt.os[12]);
            a.push(RBX);
            a.push(R12);
            a.push(R13);
            a.push(R14);
            a.push(R15);
            a.mov_rr(R12, RDI, 8);
            a.mov_rr(R13, RDX, 8);
            a.alu_rr(Alu::Xor, R14, R14, 4);
            a.mov_rr(RDI, RSI, 8);
            a.call(wide);
            a.mov_rr(RBX, RAX, 8);
            let no2 = a.new_label();
            a.alu_ri(Alu::Cmp, R12, 4, 8);
            a.jcc(Cc::Ne, no2);
            a.mov_rr(RDI, R13, 8);
            a.call(wide);
            a.mov_rr(R14, RAX, 8);
            a.bind(no2);
            for (k, l) in [(0, stat), (5, stat), (1, del), (2, rmd), (3, mkd), (4, ren)] {
                a.alu_ri(Alu::Cmp, R12, k, 8);
                a.jcc(Cc::E, l);
            }
            a.jmp(bad_op);
            // exists (0) and stat (5): `GetFileAttributesExW(path, 0, &data)`; the record is 36 bytes (attributes at 0,
            // size high at 28, size low at 32).
            a.bind(stat);
            a.alu_ri(Alu::Sub, RSP, 48, 8);
            a.mov_rr(R15, RSP, 8);
            a.mov_rr(RCX, RBX, 8);
            a.alu_rr(Alu::Xor, RDX, RDX, 4);
            a.mov_rr(R8, R15, 8);
            win_call(a, IMP_FILE_ATTRIBUTES, |_| {});
            let (statfail, is_dir, put) = (a.new_label(), a.new_label(), a.new_label());
            a.test_rr(RAX, RAX, 4);
            a.jcc(Cc::E, statfail);
            a.alu_rr(Alu::Xor, RAX, RAX, 4);
            a.alu_ri(Alu::Cmp, R12, 0, 8);
            a.jcc(Cc::E, put);
            a.load_ext(RDX, mem(R15, 28), 4, false, true);
            a.shift_imm(Shift::Shl, RDX, 32, 8);
            a.load_ext(RCX, mem(R15, 32), 4, false, true);
            a.alu_rr(Alu::Or, RDX, RCX, 8);
            a.store(mem(R13, 0), RDX, 8);
            a.load_ext(RCX, mem(R15, 0), 4, false, true);
            a.mov_ri(RDX, 1);
            a.alu_ri(Alu::And, RCX, 0x10, 4);
            a.jcc(Cc::E, is_dir);
            a.mov_ri(RDX, 2);
            a.bind(is_dir);
            a.store(mem(R13, 8), RDX, 8);
            a.bind(put);
            a.alu_ri(Alu::Add, RSP, 48, 8);
            a.jmp(ok_exit);
            a.bind(statfail);
            a.alu_ri(Alu::Add, RSP, 48, 8);
            a.jmp(fail);
            for (l, imp) in [(del, IMP_DELETE_FILE), (rmd, IMP_REMOVE_DIRECTORY)] {
                a.bind(l);
                a.mov_rr(RCX, RBX, 8);
                win_call(a, imp, |_| {});
                a.jmp(fin);
            }
            a.bind(mkd);
            a.mov_rr(RCX, RBX, 8);
            a.alu_rr(Alu::Xor, RDX, RDX, 4);
            win_call(a, IMP_CREATE_DIRECTORY, |_| {});
            a.jmp(fin);
            a.bind(ren);
            a.mov_rr(RCX, RBX, 8);
            a.mov_rr(RDX, R14, 8);
            a.mov_ri(R8, 1); // MOVEFILE_REPLACE_EXISTING
            win_call(a, IMP_MOVE_FILE, |_| {});
            // A BOOL in `eax`: 0 is a failure.
            a.bind(fin);
            a.test_rr(RAX, RAX, 4);
            a.jcc(Cc::E, fail);
            a.bind(ok_exit);
            a.alu_rr(Alu::Xor, R12, R12, 4);
            let freeing = a.new_label();
            a.jmp(freeing);
            // The error number, read before anything else can change it.
            a.bind(fail);
            win_call(a, IMP_GET_LAST_ERROR, |_| {});
            let (nf, den, other) = (a.new_label(), a.new_label(), a.new_label());
            a.mov_ri(R12, -3);
            a.alu_ri(Alu::Cmp, RAX, 2, 4);
            a.jcc(Cc::E, nf);
            a.alu_ri(Alu::Cmp, RAX, 3, 4);
            a.jcc(Cc::E, nf);
            a.alu_ri(Alu::Cmp, RAX, 5, 4);
            a.jcc(Cc::E, den);
            a.jmp(other);
            a.bind(nf);
            a.mov_ri(R12, -4);
            a.jmp(other);
            a.bind(den);
            a.mov_ri(R12, -5);
            a.bind(other);
            a.bind(freeing);
            a.mov_rr(RDI, RBX, 8);
            a.call(rt.free);
            let no_free2 = a.new_label();
            a.test_rr(R14, R14, 8);
            a.jcc(Cc::E, no_free2);
            a.mov_rr(RDI, R14, 8);
            a.call(rt.free);
            a.bind(no_free2);
            a.mov_rr(RAX, R12, 8);
            a.pop(R15);
            a.pop(R14);
            a.pop(R13);
            a.pop(R12);
            a.pop(RBX);
            a.ret();
            a.bind(bad_op);
            a.mov_ri(R12, -2);
            a.jmp(freeing);
        }
        a.bind(rt.os[13]);
        a.mov_ri(RAX, -3);
        a.ret();
        // random(buffer = rdi, count = rsi): `RtlGenRandom` (`SystemFunction036`), 1 to 256 bytes.
        let (wbad, wfail) = (a.new_label(), a.new_label());
        a.bind(rt.os[14]);
        a.alu_ri(Alu::Cmp, RSI, 1, 8);
        a.jcc(Cc::L, wbad);
        a.alu_ri(Alu::Cmp, RSI, 256, 8);
        a.jcc(Cc::G, wbad);
        a.mov_rr(RCX, RDI, 8);
        a.mov_rr(RDX, RSI, 8);
        win_call(a, IMP_RTL_GEN_RANDOM, |_| {});
        a.test_rr(RAX, RAX, 1); // a BOOLEAN in `al`
        a.jcc(Cc::E, wfail);
        a.alu_rr(Alu::Xor, RAX, RAX, 4);
        a.ret();
        a.bind(wbad);
        a.mov_ri(RAX, -2);
        a.ret();
        a.bind(wfail);
        a.mov_ri(RAX, -3);
        a.ret();
    }
    a.bind(os_error);
    // Linux: `rax` is the result, or -errno in the last 4095 values.
    let ok = a.new_label();
    a.alu_ri(Alu::Cmp, RAX, -4095, 8);
    a.jcc(Cc::B, ok);
    a.alu_ri(Alu::Cmp, RAX, -12, 8);
    a.jcc(Cc::E, nomem);
    a.alu_ri(Alu::Cmp, RAX, -22, 8);
    a.jcc(Cc::E, bad);
    a.jmp(other);
    a.bind(ok);
    a.ret();
}
