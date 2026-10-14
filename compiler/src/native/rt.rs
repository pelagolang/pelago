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

use super::elf::Blob;
use super::x86::*;

pub const STATE_SIZE: usize = 128;
const CHUNK: i64 = 1 << 20;
const STACK: i64 = 1 << 30;
/// The largest block served from the free lists: header plus payload.
const SMALL_MAX: i32 = 64 * 1024;
const LARGE_CLASS: i32 = 255;

/// The entry points of the runtime.
pub struct Runtime {
    /// `rt_write(fd = rdi, ptr = rsi, len = rdx)`: writes everything or gives up on an error.
    pub write_fd: Label,
    /// `rt_stop(ptr = rdi, len = rsi)`: prints `stopped: <message>` to standard error and exits with status 1.
    pub stop: Label,
    /// `rt_alloc(size = rdi) -> rax`: zeroed memory, 16-byte aligned; never fails (stops when out of memory).
    pub alloc: Label,
    /// `rt_free(ptr = rdi)`: accepts null.
    pub free: Label,
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
pub fn emit(a: &mut Asm, ro: &mut Blob, data: &mut Blob, main: Label, leak_check: bool) -> Runtime {
    let state_at = data.add_zero(STATE_SIZE, 16);
    assert_eq!(state_at, 0, "the runtime state must come first in .data");
    let rt = Runtime {
        write_fd: a.new_label(),
        stop: a.new_label(),
        alloc: a.new_label(),
        free: a.new_label(),
        exit: a.new_label(),
        write_u64: a.new_label(),
    };
    let msg_stopped = ro.add(b"stopped: ", 1);
    let msg_nl = ro.add(b"\n", 1);
    let msg_oom = ro.add(b"out of memory", 1);
    let msg_leak1 = ro.add(b"leaked ", 1);
    let msg_leak2 = ro.add(b" allocation(s)\n", 1);

    // ---- _start ------------------------------------------------------------------------------------
    a.alu_rr(Alu::Xor, RBP, RBP, 4);
    // A big stack of our own (1 GiB, taken lazily), so deep recursion works like it does in the
    // interpreter; if the mapping fails, the stack the system gave us stays.
    a.mov_ri(RAX, 9);
    a.alu_rr(Alu::Xor, RDI, RDI, 4);
    a.mov_ri(RSI, STACK);
    a.mov_ri(RDX, 3);
    a.mov_ri(R10, 0x4022); // private, anonymous, no reservation
    a.mov_ri(R8, -1);
    a.alu_rr(Alu::Xor, R9, R9, 4);
    a.syscall();
    let keep = a.new_label();
    a.alu_ri(Alu::Cmp, RAX, -4095, 8);
    a.jcc(Cc::Ae, keep);
    a.lea(RSP, mem(RAX, STACK as i32));
    a.bind(keep);
    a.alu_ri(Alu::And, RSP, -16, 8);
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
    a.mov_ri(RAX, 231);
    a.syscall();
    a.ud2();

    // ---- rt_write ----------------------------------------------------------------------------------
    a.bind(rt.write_fd);
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

    // ---- rt_stop -----------------------------------------------------------------------------------
    a.bind(rt.stop);
    a.mov_rr(R12, RDI, 8);
    a.mov_rr(R13, RSI, 8);
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

    // ---- out of memory -----------------------------------------------------------------------------
    let oom = a.new_label();
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
    emit_mmap(a, CHUNK);
    a.mov_rr(RDX, RAX, 8);
    a.pop(RSI);
    a.pop(RAX);
    a.alu_ri(Alu::Cmp, RDX, -4095, 8);
    a.jcc(Cc::Ae, oom);
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
    a.mov_rr(R12, RSI, 8);
    emit_mmap_reg(a, RSI);
    a.mov_rr(RDX, RAX, 8);
    a.alu_ri(Alu::Cmp, RDX, -4095, 8);
    a.jcc(Cc::Ae, oom);
    a.store_imm(mem(RDX, 0), LARGE_CLASS, 8);
    a.store(mem(RDX, 8), R12, 8);
    a.bind(out);
    a.lea(RAX, mem(RDX, 16));
    state(a, RSI, 112);
    a.alu_mi(Alu::Add, mem(RSI, 0), 1, 8);
    a.pop(R12);
    a.pop(RBX);
    a.ret();

    // ---- rt_free -----------------------------------------------------------------------------------
    a.bind(rt.free);
    let (fdone, flarge) = (a.new_label(), a.new_label());
    a.test_rr(RDI, RDI, 8);
    a.jcc(Cc::E, fdone);
    a.lea(RDX, mem(RDI, -16));
    a.load(RAX, mem(RDX, 0), 8);
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
    a.load(RSI, mem(RDX, 8), 8);
    a.mov_rr(RDI, RDX, 8);
    a.mov_ri(RAX, 11);
    a.syscall();
    a.ret();
    rt
}

/// `mmap(0, len, read|write, private|anonymous, -1, 0)`: the address (or a negative error) in `rax`.
fn emit_mmap(a: &mut Asm, len: i64) {
    a.mov_ri(RSI, len);
    emit_mmap_reg(a, RSI);
}

/// The same with the length already in `rsi`.
fn emit_mmap_reg(a: &mut Asm, len: Reg) {
    debug_assert_eq!(len, RSI);
    a.mov_ri(RAX, 9);
    a.alu_rr(Alu::Xor, RDI, RDI, 4);
    a.mov_ri(RDX, 3);
    a.mov_ri(R10, 0x22);
    a.mov_ri(R8, -1);
    a.alu_rr(Alu::Xor, R9, R9, 4);
    a.syscall();
}
