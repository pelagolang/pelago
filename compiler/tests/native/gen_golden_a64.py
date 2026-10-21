#!/usr/bin/env python3
"""Regenerates compiler/src/native/a64_golden.rs: each AArch64 instruction form with the bytes the
system assembler (`aarch64-linux-gnu-as`) produces for the same text. Run from the repository root
after changing the encoder (apt: binutils-aarch64-linux-gnu):

    python3 compiler/tests/native/gen_golden_a64.py
"""
import os, subprocess, tempfile

CASES = [
 # moves
 ("mov x0, #5", "a.mov_ri(X0, 5)"), ("mov x1, #0", "a.mov_ri(X1, 0)"), ("mov x2, #-1", "a.mov_ri(X2, -1)"),
 ("mov x3, #-2", "a.mov_ri(X3, -2)"), ("mov x4, #0x1234", "a.mov_ri(X4, 0x1234)"),
 ("movz x5, #0x5678, lsl #16\nmovk x5, #0x1234, lsl #32", "a.mov_ri(X5, 0x1234_5678_0000)"),
 ("movz x6, #0x1111\nmovk x6, #0x2222, lsl #16\nmovk x6, #0x3333, lsl #32\nmovk x6, #0x4444, lsl #48", "a.mov_ri(X6, 0x4444_3333_2222_1111)"),
 ("movn x7, #0x1234", "a.mov_ri(X7, !0x1234)"),
 ("movn x8, #0xfffa\nmovk x8, #0xfffe, lsl #32", "a.mov_ri(X8, (!(1i64 << 32) & !0xFFFF) | 5)"),
 ("mov x9, #0x8000000000000000", "a.mov_ri(X9, i64::MIN)"),
 ("mov x10, #0x7fffffffffffffff", "a.mov_ri(X10, i64::MAX)"),
 ("movz x11, #7", "a.movz(X11, 7, 0)"), ("movk x11, #9, lsl #48", "a.movk(X11, 9, 3)"),
 ("mov x1, x2", "a.mov_rr(X1, X2)"), ("mov w3, w4", "a.mov_w(X3, X4)"),
 ("mov x5, sp", "a.mov_from_sp(X5)"), ("mov sp, x6", "a.mov_to_sp(X6)"),
 # arithmetic
 ("add x0, x1, #5", "a.add_ri(X0, X1, 5)"), ("add x0, x1, #4095", "a.add_ri(X0, X1, 4095)"),
 ("add x0, x1, #4096", "a.add_ri(X0, X1, 4096)"), ("add sp, sp, #16", "a.add_ri(SP, SP, 16)"),
 ("sub sp, sp, #4096", "a.sub_ri(SP, SP, 4096)"), ("sub x2, x3, #1", "a.sub_ri(X2, X3, 1)"),
 ("adds x2, x3, #1", "a.adds_ri(X2, X3, 1)"), ("subs x2, x3, #8", "a.subs_ri(X2, X3, 8)"),
 ("cmp x4, #0", "a.cmp_ri(X4, 0)"), ("cmp x4, #4095", "a.cmp_ri(X4, 4095)"),
 ("add x0, x1, #8192\nadd x0, x0, #5", "a.add_imm(X0, X1, 8192 + 5)"),
 ("sub x0, x1, #12288\nsub x0, x0, #7", "a.add_imm(X0, X1, -(12288 + 7))"),
 ("mov x17, #0x10000000\nadd x0, x1, x17, uxtx", "a.add_imm(X0, X1, 0x1000_0000)"),
 ("add x0, sp, x16, uxtx", "a.add_ext(X0, SP, X16)"),
 ("add x0, x1, x2", "a.add_rr(X0, X1, X2)"), ("sub x0, x1, x2", "a.sub_rr(X0, X1, X2)"),
 ("adds x0, x1, x2", "a.adds_rr(X0, X1, X2)"), ("subs x0, x1, x2", "a.subs_rr(X0, X1, X2)"),
 ("add x0, x1, x2, lsl #3", "a.add_rr_sh(X0, X1, X2, Shift::Lsl, 3)"),
 ("cmp x1, x2", "a.cmp_rr(X1, X2)"), ("cmp x1, x2, asr #63", "a.cmp_rr_sh(X1, X2, Shift::Asr, 63)"),
 ("neg x1, x2", "a.neg(X1, X2)"),
 ("and x0, x1, x2", "a.and_rr(X0, X1, X2)"), ("orr x0, x1, x2", "a.orr_rr(X0, X1, X2)"),
 ("eor x0, x1, x2", "a.eor_rr(X0, X1, X2)"), ("tst x1, x2", "a.tst_rr(X1, X2)"), ("mvn x0, x1", "a.mvn(X0, X1)"),
 ("and x0, x1, #0xff", "assert!(a.and_ri(X0, X1, 0xff))"),
 ("and x0, x1, #0xffffffff", "assert!(a.and_ri(X0, X1, 0xffff_ffff))"),
 ("and x0, x1, #1", "assert!(a.and_ri(X0, X1, 1))"),
 ("orr x0, x1, #0x8000000000000000", "assert!(a.orr_ri(X0, X1, 1 << 63))"),
 ("eor x0, x1, #0x5555555555555555", "assert!(a.eor_ri(X0, X1, 0x5555_5555_5555_5555))"),
 ("and x0, x1, #0x7ffffffffffffffe", "assert!(a.and_ri(X0, X1, 0x7fff_ffff_ffff_fffe))"),
 ("and x0, x1, #0xf0f0f0f0f0f0f0f0", "assert!(a.and_ri(X0, X1, 0xf0f0_f0f0_f0f0_f0f0))"),
 ("and x0, x1, #0x3ffc", "assert!(a.and_ri(X0, X1, 0x3ffc))"),
 ("tst x1, #0x80", "assert!(a.tst_ri(X1, 0x80))"),
 ("mul x0, x1, x2", "a.mul(X0, X1, X2)"), ("madd x0, x1, x2, x3", "a.madd(X0, X1, X2, X3)"),
 ("msub x0, x1, x2, x3", "a.msub(X0, X1, X2, X3)"),
 ("smulh x0, x1, x2", "a.smulh(X0, X1, X2)"), ("umulh x0, x1, x2", "a.umulh(X0, X1, X2)"),
 ("sdiv x0, x1, x2", "a.sdiv(X0, X1, X2)"), ("udiv x0, x1, x2", "a.udiv(X0, X1, X2)"),
 ("lsl x0, x1, x2", "a.lslv(X0, X1, X2)"), ("lsr x0, x1, x2", "a.lsrv(X0, X1, X2)"), ("asr x0, x1, x2", "a.asrv(X0, X1, X2)"),
 ("lsl x0, x1, #3", "a.lsl_ri(X0, X1, 3)"), ("lsl x0, x1, #63", "a.lsl_ri(X0, X1, 63)"), ("lsl x0, x1, #0", "a.lsl_ri(X0, X1, 0)"),
 ("lsr x0, x1, #7", "a.lsr_ri(X0, X1, 7)"), ("asr x0, x1, #63", "a.asr_ri(X0, X1, 63)"),
 ("ubfx x0, x1, #4, #12", "a.ubfx(X0, X1, 4, 12)"),
 ("uxtb w0, w1", "a.uxt(X0, X1, 8)"), ("uxth w0, w1", "a.uxt(X0, X1, 16)"), ("mov w0, w1", "a.uxt(X0, X1, 32)"),
 ("sxtb x0, w1", "a.sxt(X0, X1, 8)"), ("sxth x0, w1", "a.sxt(X0, X1, 16)"), ("sxtw x0, w1", "a.sxt(X0, X1, 32)"),
 ("clz x0, x1", "a.clz(X0, X1)"),
 ("cset w0, eq", "a.cset(X0, Cond::Eq)"), ("cset w1, lt", "a.cset(X1, Cond::Lt)"), ("cset w2, hi", "a.cset(X2, Cond::Hi)"),
 ("cset w2, vs", "a.cset(X2, Cond::Vs)"), ("cset w2, ls", "a.cset(X2, Cond::Ls)"),
 ("csel x0, x1, x2, ne", "a.csel(X0, X1, X2, Cond::Ne)"), ("csel x3, x4, x5, ge", "a.csel(X3, X4, X5, Cond::Ge)"),
 # control flow
 ("br x16", "a.br(X16)"), ("blr x15", "a.blr(X15)"), ("ret", "a.ret()"), ("svc #0", "a.svc()"),
 ("brk #0", "a.brk()"), ("nop", "a.nop()"),
 # memory
 ("ldr x0, [x1]", "a.ldr(X0, X1, 0, 8)"), ("ldr x0, [sp, #16]", "a.ldr(X0, SP, 16, 8)"),
 ("ldr x0, [x1, #32760]", "a.ldr(X0, X1, 32760, 8)"), ("ldr w0, [x1, #4]", "a.ldr(X0, X1, 4, 4)"),
 ("ldrh w0, [x1, #2]", "a.ldr(X0, X1, 2, 2)"), ("ldrb w0, [x1, #1]", "a.ldr(X0, X1, 1, 1)"),
 ("ldrsb x0, [x1, #1]", "a.ldrs(X0, X1, 1, 1)"), ("ldrsh x0, [x1, #2]", "a.ldrs(X0, X1, 2, 2)"), ("ldrsw x0, [x1, #4]", "a.ldrs(X0, X1, 4, 4)"),
 ("str x0, [x1]", "a.str(X0, X1, 0, 8)"), ("str w0, [x1, #8]", "a.str(X0, X1, 8, 4)"),
 ("strh w0, [x1, #6]", "a.str(X0, X1, 6, 2)"), ("strb w0, [x1, #3]", "a.str(X0, X1, 3, 1)"),
 ("ldur x0, [x1, #-8]", "a.ldr(X0, X1, -8, 8)"), ("stur x0, [x1, #-256]", "a.str(X0, X1, -256, 8)"),
 ("ldur x0, [x1, #3]", "a.ldr(X0, X1, 3, 8)"), ("ldur w0, [x1, #-1]", "a.ldr(X0, X1, -1, 4)"),
 ("ldursb x0, [x1, #-1]", "a.ldrs(X0, X1, -1, 1)"),
 ("mov x16, #0x10000\nadd x16, sp, x16, uxtx\nldr x0, [x16]", "a.ldr(X0, SP, 0x10000, 8)"),
 ("mov x16, #-4096\nadd x16, x29, x16, uxtx\nstr x2, [x16]", "a.str(X2, FP, -4096, 8)"),
 ("ldr s0, [x1, #4]", "a.ldr_f(V(0), X1, 4, 4)"), ("ldr d1, [sp, #8]", "a.ldr_f(V(1), SP, 8, 8)"),
 ("str s2, [x1]", "a.str_f(V(2), X1, 0, 4)"), ("str d3, [x29, #-8]", "a.str_f(V(3), FP, -8, 8)"),
 ("ldr q0, [x1, #16]", "a.ldr_f(V(0), X1, 16, 16)"), ("str q1, [sp, #32]", "a.str_f(V(1), SP, 32, 16)"),
 ("ldur q0, [x1, #-16]", "a.ldr_f(V(0), X1, -16, 16)"), ("stur q2, [x1, #1]", "a.str_f(V(2), X1, 1, 16)"),
 ("ldr x0, [x1, x2]", "a.ldr_idx(X0, X1, X2, 8, false)"), ("ldr x0, [x1, x2, lsl #3]", "a.ldr_idx(X0, X1, X2, 8, true)"),
 ("ldrb w0, [x1, x2]", "a.ldr_idx(X0, X1, X2, 1, false)"), ("ldr w0, [x1, x2, lsl #2]", "a.ldr_idx(X0, X1, X2, 4, true)"),
 ("str x0, [x1, x2, lsl #3]", "a.str_idx(X0, X1, X2, 8, true)"), ("strb w0, [x1, x2]", "a.str_idx(X0, X1, X2, 1, false)"),
 ("stp x29, x30, [sp, #-16]!", "a.stp_pre(FP, LR, SP, -16)"), ("ldp x29, x30, [sp], #16", "a.ldp_post(FP, LR, SP, 16)"),
 ("stp x19, x20, [x29, #-32]", "a.stp(X19, X20, FP, -32)"), ("ldp x19, x20, [sp, #16]", "a.ldp(X19, X20, SP, 16)"),
 ("stp xzr, xzr, [x9]", "a.stp(XZR, XZR, X9, 0)"),
 # floating point
 ("fadd d0, d1, d2", "a.fadd(true, V(0), V(1), V(2))"), ("fadd s0, s1, s2", "a.fadd(false, V(0), V(1), V(2))"),
 ("fsub d3, d4, d5", "a.fsub(true, V(3), V(4), V(5))"), ("fmul s3, s4, s5", "a.fmul(false, V(3), V(4), V(5))"),
 ("fmadd d0, d1, d2, d3", "a.fmadd(true, V(0), V(1), V(2), V(3))"), ("fmadd s4, s5, s6, s7", "a.fmadd(false, V(4), V(5), V(6), V(7))"),
 ("fdiv d6, d7, d8", "a.fdiv(true, V(6), V(7), V(8))"), ("fneg d0, d1", "a.fneg(true, V(0), V(1))"),
 ("fneg s0, s1", "a.fneg(false, V(0), V(1))"), ("fmov d0, d1", "a.fmov_rr(true, V(0), V(1))"),
 ("fmov s0, s1", "a.fmov_rr(false, V(0), V(1))"),
 ("fcmp d0, d1", "a.fcmp(true, V(0), V(1))"), ("fcmp s2, s3", "a.fcmp(false, V(2), V(3))"),
 ("fcmp d0, #0.0", "a.fcmp_zero(true, V(0))"), ("fcmp s5, #0.0", "a.fcmp_zero(false, V(5))"),
 ("fmov d0, x1", "a.fmov_from_gp(true, V(0), X1)"), ("fmov s0, w1", "a.fmov_from_gp(false, V(0), X1)"),
 ("fmov x1, d0", "a.fmov_to_gp(true, X1, V(0))"), ("fmov w1, s0", "a.fmov_to_gp(false, X1, V(0))"),
 ("scvtf d0, x1", "a.scvtf(true, V(0), X1)"), ("scvtf s0, x1", "a.scvtf(false, V(0), X1)"),
 ("fcvtzs x0, d1", "a.fcvtzs(true, X0, V(1))"), ("fcvtzs x0, s1", "a.fcvtzs(false, X0, V(1))"),
 ("fcvt d0, s1", "a.fcvt(true, V(0), V(1))"), ("fcvt s0, d1", "a.fcvt(false, V(0), V(1))"),
 ("fsqrt d0, d1", "a.fsqrt(true, V(0), V(1))"),
 # vectors
 ("dup v0.2d, x1", "a.vdup(V(0), X1)"), ("add v0.2d, v1.2d, v2.2d", "a.vadd(V(0), V(1), V(2))"),
 ("sub v0.2d, v1.2d, v2.2d", "a.vsub(V(0), V(1), V(2))"), ("and v0.16b, v1.16b, v2.16b", "a.vand(V(0), V(1), V(2))"),
 ("orr v0.16b, v1.16b, v2.16b", "a.vorr(V(0), V(1), V(2))"), ("eor v0.16b, v1.16b, v2.16b", "a.veor(V(0), V(1), V(2))"),
 ("shl v0.2d, v1.2d, #5", "a.vshl(V(0), V(1), 5)"), ("shl v0.2d, v1.2d, #63", "a.vshl(V(0), V(1), 63)"),
 ("ushr v0.2d, v1.2d, #5", "a.vushr(V(0), V(1), 5)"), ("ushr v0.2d, v1.2d, #1", "a.vushr(V(0), V(1), 1)"),
 ("addp d0, v1.2d", "a.vaddp(V(0), V(1))"), ("mov v0.d[1], x2", "a.vins(V(0), 1, X2)"), ("mov v0.d[0], x2", "a.vins(V(0), 0, X2)"),
 ("mov x1, v2.d[1]", "a.vmov_to_gp(X1, V(2), 1)"), ("mov x1, v2.d[0]", "a.vmov_to_gp(X1, V(2), 0)"),
 ("mov v3.16b, v4.16b", "a.vmov(V(3), V(4))"),
]

# Instructions that reach a label: the label is bound at the end, a fixed distance away.
LABEL_CASES = [
 ("b 1f\nnop\nnop\n1:", "let l = a.new_label(); a.b(l); a.nop(); a.nop(); a.bind(l)"),
 ("bl 1f\nnop\n1:", "let l = a.new_label(); a.bl(l); a.nop(); a.bind(l)"),
 ("1:\nnop\nb 1b", "let l = a.new_label(); a.bind(l); a.nop(); a.b(l)"),
 ("b.ne 1f\nnop\n1:", "let l = a.new_label(); a.b_cond(Cond::Ne, l); a.nop(); a.bind(l)"),
 ("b.lo 1f\nnop\nnop\nnop\n1:", "let l = a.new_label(); a.b_cond(Cond::Lo, l); a.nop(); a.nop(); a.nop(); a.bind(l)"),
 ("cbz x3, 1f\nnop\n1:", "let l = a.new_label(); a.cbz(X3, l); a.nop(); a.bind(l)"),
 ("1:\nnop\ncbnz x4, 1b", "let l = a.new_label(); a.bind(l); a.nop(); a.cbnz(X4, l)"),
]

def assemble(text):
    with tempfile.TemporaryDirectory() as d:
        s = os.path.join(d, "a.s"); o = os.path.join(d, "a.o"); b = os.path.join(d, "a.bin")
        open(s, "w").write(text + "\n")
        subprocess.run(["aarch64-linux-gnu-as", "-o", o, s], check=True)
        subprocess.run(["aarch64-linux-gnu-objcopy", "-O", "binary", "-j", ".text", o, b], check=True)
        return open(b, "rb").read().hex(" ", 4)

def rust(text, code):
    t = text.replace("\n", "\\n")
    return f'        ("{t}", "{assemble(text)}", |a| {{ {code} }}),'

out = ["// Generated by compiler/tests/native/gen_golden_a64.py: do not edit.",
       "// Each form with the words the system assembler produces for the same instruction.", "",
       "use super::a64::*;", "",
       "type Case = (&'static str, &'static str, fn(&mut Asm));", "",
       "pub fn golden() -> Vec<Case> {", "    vec!["]
for text, code in CASES + LABEL_CASES:
    out.append(rust(text, code))
out += ["    ]", "}", ""]
path = os.path.join(os.path.dirname(__file__), "..", "..", "src", "native", "a64_golden.rs")
open(path, "w").write("\n".join(out))
print(len(CASES) + len(LABEL_CASES), "cases written to", os.path.normpath(path))
