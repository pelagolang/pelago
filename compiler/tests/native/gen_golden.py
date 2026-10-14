#!/usr/bin/env python3
"""Regenerates compiler/src/native/x86_golden.rs: each instruction form with the bytes the system
assembler (`as`) produces for the same text. Run from the repository root after changing the encoder:

    python3 compiler/tests/native/gen_golden.py
"""
import os, subprocess, tempfile, sys

CASES = [
 # moves
 ("mov rax, rbx", "a.mov_rr(RAX, RBX, 8)"), ("mov r8, r15", "a.mov_rr(R8, R15, 8)"),
 ("mov eax, ecx", "a.mov_rr(RAX, RCX, 4)"), ("mov ax, cx", "a.mov_rr(RAX, RCX, 2)"),
 ("mov al, cl", "a.mov_rr(RAX, RCX, 1)"), ("mov sil, dil", "a.mov_rr(RSI, RDI, 1)"),
 ("mov r9b, sil", "a.mov_rr(R9, RSI, 1)"),
 ("mov eax, 5", "a.mov_ri(RAX, 5)"), ("mov r9d, 7", "a.mov_ri(R9, 7)"),
 ("mov rax, -1", "a.mov_ri(RAX, -1)"), ("movabs rax, 0x123456789a", "a.mov_ri(RAX, 0x123456789a)"),
 ("mov r10, -2", "a.mov_ri(R10, -2)"), ("movabs r11, 0x1122334455667788", "a.mov_ri(R11, 0x1122334455667788)"),
 ("mov eax, 0xffffffff", "a.mov_ri(RAX, 0xffffffff)"),
 ("mov rax, [rbx]", "a.load(RAX, mem(RBX, 0), 8)"), ("mov rax, [rbp-8]", "a.load(RAX, mem(RBP, -8), 8)"),
 ("mov eax, [rsp+16]", "a.load(RAX, mem(RSP, 16), 4)"), ("mov r8, [r12]", "a.load(R8, mem(R12, 0), 8)"),
 ("mov r9, [r13]", "a.load(R9, mem(R13, 0), 8)"), ("mov rax, [rcx+0x1000]", "a.load(RAX, mem(RCX, 0x1000), 8)"),
 ("mov al, [rdi]", "a.load(RAX, mem(RDI, 0), 1)"), ("mov sil, [rax+1]", "a.load(RSI, mem(RAX, 1), 1)"),
 ("mov ax, [rdx]", "a.load(RAX, mem(RDX, 0), 2)"), ("mov r10d, [r11+4]", "a.load(R10, mem(R11, 4), 4)"),
 ("mov [rbx], rax", "a.store(mem(RBX, 0), RAX, 8)"), ("mov [rbp-8], rcx", "a.store(mem(RBP, -8), RCX, 8)"),
 ("mov [rsp+8], r15", "a.store(mem(RSP, 8), R15, 8)"), ("mov [r12+3], dil", "a.store(mem(R12, 3), RDI, 1)"),
 ("mov [rax], cx", "a.store(mem(RAX, 0), RCX, 2)"), ("mov [rax+4], edx", "a.store(mem(RAX, 4), RDX, 4)"),
 ("mov qword ptr [rax], 5", "a.store_imm(mem(RAX, 0), 5, 8)"), ("mov dword ptr [rbp-4], -1", "a.store_imm(mem(RBP, -4), -1, 4)"),
 ("mov byte ptr [rsp], 7", "a.store_imm(mem(RSP, 0), 7, 1)"), ("mov word ptr [rax], 300", "a.store_imm(mem(RAX, 0), 300, 2)"),
 ("movzx eax, byte ptr [rbx]", "a.load_ext(RAX, mem(RBX, 0), 1, false, false)"),
 ("movzx rax, word ptr [rbx]", "a.load_ext(RAX, mem(RBX, 0), 2, false, true)"),
 ("movsx rax, byte ptr [rbx]", "a.load_ext(RAX, mem(RBX, 0), 1, true, true)"),
 ("movsx eax, word ptr [rcx+2]", "a.load_ext(RAX, mem(RCX, 2), 2, true, false)"),
 ("movsxd rax, dword ptr [rbp-4]", "a.load_ext(RAX, mem(RBP, -4), 4, true, true)"),
 ("mov eax, dword ptr [rdx]", "a.load_ext(RAX, mem(RDX, 0), 4, false, true)"),
 ("movzx eax, cl", "a.extend_rr(RAX, RCX, 1, false, false)"), ("movzx eax, sil", "a.extend_rr(RAX, RSI, 1, false, false)"),
 ("movsx rax, dil", "a.extend_rr(RAX, RDI, 1, true, true)"), ("movsx rdx, cx", "a.extend_rr(RDX, RCX, 2, true, true)"),
 ("movsxd rax, ecx", "a.extend_rr(RAX, RCX, 4, true, true)"), ("movzx r8d, r9b", "a.extend_rr(R8, R9, 1, false, false)"),
 ("lea rax, [rbp-16]", "a.lea(RAX, mem(RBP, -16))"), ("lea r9, [rsp+8]", "a.lea(R9, mem(RSP, 8))"),
 ("push rax", "a.push(RAX)"), ("push r12", "a.push(R12)"), ("pop rbx", "a.pop(RBX)"), ("pop r15", "a.pop(R15)"),
 # arithmetic
 ("add rax, rbx", "a.alu_rr(Alu::Add, RAX, RBX, 8)"), ("sub r8, r9", "a.alu_rr(Alu::Sub, R8, R9, 8)"),
 ("and eax, ecx", "a.alu_rr(Alu::And, RAX, RCX, 4)"), ("or ax, dx", "a.alu_rr(Alu::Or, RAX, RDX, 2)"),
 ("xor al, bl", "a.alu_rr(Alu::Xor, RAX, RBX, 1)"), ("cmp rax, rdx", "a.alu_rr(Alu::Cmp, RAX, RDX, 8)"),
 ("cmp sil, dil", "a.alu_rr(Alu::Cmp, RSI, RDI, 1)"),
 ("add rax, [rbx]", "a.alu_rm(Alu::Add, RAX, mem(RBX, 0), 8)"), ("cmp ecx, [rbp-8]", "a.alu_rm(Alu::Cmp, RCX, mem(RBP, -8), 4)"),
 ("sub rdx, [rsp+16]", "a.alu_rm(Alu::Sub, RDX, mem(RSP, 16), 8)"),
 ("add rax, 1", "a.alu_ri(Alu::Add, RAX, 1, 8)"), ("add rax, 1000", "a.alu_ri(Alu::Add, RAX, 1000, 8)"),
 ("sub rsp, 32", "a.alu_ri(Alu::Sub, RSP, 32, 8)"), ("and rsp, -16", "a.alu_ri(Alu::And, RSP, -16, 8)"),
 ("cmp eax, 0", "a.alu_ri(Alu::Cmp, RAX, 0, 4)"), ("cmp r8, 255", "a.alu_ri(Alu::Cmp, R8, 255, 8)"),
 ("cmp al, 3", "a.alu_ri(Alu::Cmp, RAX, 3, 1)"), ("xor ecx, 0x7fffffff", "a.alu_ri(Alu::Xor, RCX, 0x7fffffff, 4)"),
 ("add ax, 5", "a.alu_ri(Alu::Add, RAX, 5, 2)"), ("add ax, 1000", "a.alu_ri(Alu::Add, RAX, 1000, 2)"),
 ("cmp sil, 1", "a.alu_ri(Alu::Cmp, RSI, 1, 1)"),
 ("add qword ptr [rax], 1", "a.alu_mi(Alu::Add, mem(RAX, 0), 1, 8)"), ("cmp dword ptr [rbp-4], 0", "a.alu_mi(Alu::Cmp, mem(RBP, -4), 0, 4)"),
 ("sub byte ptr [rsp], 1", "a.alu_mi(Alu::Sub, mem(RSP, 0), 1, 1)"), ("add qword ptr [rcx], 100000", "a.alu_mi(Alu::Add, mem(RCX, 0), 100000, 8)"),
 ("test rax, rax", "a.test_rr(RAX, RAX, 8)"), ("test ecx, edx", "a.test_rr(RCX, RDX, 4)"),
 ("test al, al", "a.test_rr(RAX, RAX, 1)"), ("test sil, sil", "a.test_rr(RSI, RSI, 1)"),
 ("imul rax, rbx", "a.imul_rr(RAX, RBX, 8)"), ("imul eax, ecx", "a.imul_rr(RAX, RCX, 4)"),
 ("imul r8, r9", "a.imul_rr(R8, R9, 8)"), ("imul ax, cx", "a.imul_rr(RAX, RCX, 2)"),
 ("neg rax", "a.neg(RAX, 8)"), ("neg ecx", "a.neg(RCX, 4)"), ("neg r9", "a.neg(R9, 8)"), ("neg sil", "a.neg(RSI, 1)"),
 ("not rdx", "a.not(RDX, 8)"), ("not al", "a.not(RAX, 1)"),
 ("mul rcx", "a.mul(RCX, 8)"), ("mul ecx", "a.mul(RCX, 4)"), ("mul r9", "a.mul(R9, 8)"),
 ("div rcx", "a.div(RCX, 8)"), ("idiv rbx", "a.idiv(RBX, 8)"), ("idiv r8", "a.idiv(R8, 8)"),
 ("div ecx", "a.div(RCX, 4)"), ("idiv ecx", "a.idiv(RCX, 4)"),
 ("cqo", "a.sign_extend_acc(8)"), ("cdq", "a.sign_extend_acc(4)"),
 ("shl rax, cl", "a.shift_cl(Shift::Shl, RAX, 8)"), ("sar rdx, cl", "a.shift_cl(Shift::Sar, RDX, 8)"),
 ("shr r8d, cl", "a.shift_cl(Shift::Shr, R8, 4)"), ("shl al, cl", "a.shift_cl(Shift::Shl, RAX, 1)"),
 ("shr ax, cl", "a.shift_cl(Shift::Shr, RAX, 2)"),
 ("shl rax, 3", "a.shift_imm(Shift::Shl, RAX, 3, 8)"), ("sar edx, 2", "a.shift_imm(Shift::Sar, RDX, 2, 4)"),
 ("shr r9, 63", "a.shift_imm(Shift::Shr, R9, 63, 8)"), ("shl al, 4", "a.shift_imm(Shift::Shl, RAX, 4, 1)"),
 ("sete al", "a.setcc(Cc::E, RAX)"), ("setne cl", "a.setcc(Cc::Ne, RCX)"), ("setl sil", "a.setcc(Cc::L, RSI)"),
 ("setb r9b", "a.setcc(Cc::B, R9)"), ("setg r8b", "a.setcc(Cc::G, R8)"), ("seto dil", "a.setcc(Cc::O, RDI)"),
 ("setp al", "a.setcc(Cc::P, RAX)"),
 ("cmove rax, rbx", "a.cmov(Cc::E, RAX, RBX, 8)"), ("cmovl ecx, edx", "a.cmov(Cc::L, RCX, RDX, 4)"),
 ("cmovne r8, r9", "a.cmov(Cc::Ne, R8, R9, 8)"),
 ("call rax", "a.call_reg(RAX)"), ("call r11", "a.call_reg(R11)"), ("jmp rcx", "a.jmp_reg(RCX)"), ("jmp r9", "a.jmp_reg(R9)"),
 ("ret", "a.ret()"), ("syscall", "a.syscall()"), ("ud2", "a.ud2()"),
 ("rep movsb", "a.rep_movsb()"), ("rep stosb", "a.rep_stosb()"),
 # SSE
 ("movsd xmm0, [rax]", "a.fload(Xmm(0), mem(RAX, 0), true)"), ("movss xmm1, [rbp-8]", "a.fload(Xmm(1), mem(RBP, -8), false)"),
 ("movsd [rsp+8], xmm2", "a.fstore(mem(RSP, 8), Xmm(2), true)"), ("movss [rax], xmm9", "a.fstore(mem(RAX, 0), Xmm(9), false)"),
 ("movaps xmm1, xmm2", "a.fmov_rr(Xmm(1), Xmm(2))"),
 ("addsd xmm0, xmm1", "a.farith(Sse::Add, Xmm(0), Xmm(1), true)"), ("subss xmm3, xmm4", "a.farith(Sse::Sub, Xmm(3), Xmm(4), false)"),
 ("mulsd xmm8, xmm9", "a.farith(Sse::Mul, Xmm(8), Xmm(9), true)"), ("divss xmm0, xmm15", "a.farith(Sse::Div, Xmm(0), Xmm(15), false)"),
 ("ucomisd xmm0, xmm1", "a.fcmp(Xmm(0), Xmm(1), true)"), ("ucomiss xmm2, xmm3", "a.fcmp(Xmm(2), Xmm(3), false)"),
 ("cvtsi2sd xmm0, rax", "a.cvt_int_to_float(Xmm(0), RAX, true, true)"), ("cvtsi2sd xmm1, eax", "a.cvt_int_to_float(Xmm(1), RAX, false, true)"),
 ("cvtsi2ss xmm2, rcx", "a.cvt_int_to_float(Xmm(2), RCX, true, false)"),
 ("cvttsd2si rax, xmm0", "a.cvt_float_to_int(RAX, Xmm(0), true, true)"), ("cvttsd2si eax, xmm1", "a.cvt_float_to_int(RAX, Xmm(1), false, true)"),
 ("cvttss2si rdx, xmm2", "a.cvt_float_to_int(RDX, Xmm(2), true, false)"),
 ("cvtss2sd xmm0, xmm1", "a.cvt_float_resize(Xmm(0), Xmm(1), true)"), ("cvtsd2ss xmm1, xmm2", "a.cvt_float_resize(Xmm(1), Xmm(2), false)"),
 ("xorps xmm0, xmm0", "a.fxor(Xmm(0), Xmm(0))"),
 ("movq xmm0, rax", "a.movq_to_xmm(Xmm(0), RAX, true)"), ("movd xmm1, ecx", "a.movq_to_xmm(Xmm(1), RCX, false)"),
 ("movq rax, xmm0", "a.movq_from_xmm(RAX, Xmm(0), true)"), ("movd ecx, xmm1", "a.movq_from_xmm(RCX, Xmm(1), false)"),
 ("movq xmm9, r10", "a.movq_to_xmm(Xmm(9), R10, true)"),
 ("fld qword ptr [rax]", "a.x87_load(mem(RAX, 0), true)"), ("fld dword ptr [rbp-8]", "a.x87_load(mem(RBP, -8), false)"),
 ("fstp qword ptr [rsp+8]", "a.x87_store_pop(mem(RSP, 8), true)"), ("fstp dword ptr [rax]", "a.x87_store_pop(mem(RAX, 0), false)"),
 ("fprem", "a.x87_prem()"), ("fnstsw ax", "a.x87_status_ax()"), ("fstp st(1)", "a.x87_drop_second()"),
]

def assemble(text):
    with tempfile.TemporaryDirectory() as d:
        s = os.path.join(d, "a.s"); o = os.path.join(d, "a.o"); b = os.path.join(d, "a.bin")
        open(s, "w").write(".intel_syntax noprefix\n" + text + "\n")
        subprocess.run(["as", "--64", "-o", o, s], check=True)
        subprocess.run(["objcopy", "-O", "binary", "-j", ".text", o, b], check=True)
        return open(b, "rb").read().hex(" ")

out = ["// Generated by compiler/tests/native/gen_golden.py: do not edit.",
       "// Each form with the bytes the system assembler produces for the same instruction.", "",
       "use super::x86::*;", "",
       "type Case = (&'static str, &'static str, fn(&mut Asm));", "",
       "pub fn golden() -> Vec<Case> {", "    vec!["]
for text, rust in CASES:
    out.append(f'        ({text!r}, "{assemble(text)}", |a| {rust}),'.replace("'", '"', 2) if False else f'        ("{text}", "{assemble(text)}", |a| {rust}),')
out += ["    ]", "}", ""]
path = os.path.join(os.path.dirname(__file__), "..", "..", "src", "native", "x86_golden.rs")
open(path, "w").write("\n".join(out))
print(len(CASES), "cases written to", os.path.normpath(path))
