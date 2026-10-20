use super::x86::*;
use super::x86_golden::golden;

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn every_encoding_matches_the_assembler() {
    for (text, want, build) in golden() {
        let mut a = Asm::new();
        build(&mut a);
        assert_eq!(hex(&a.code), want, "{text}");
    }
}

/// When `as` and `objcopy` are installed, ask them again (the golden bytes could be stale). Only on
/// Linux: the tools that come with other systems may default to other settings.
#[cfg(target_os = "linux")]
#[test]
fn the_installed_assembler_agrees() {
    let have = |tool: &str| {
        std::process::Command::new(tool)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    };
    if !(have("as") && have("objcopy")) {
        return;
    }
    let dir = std::env::temp_dir().join(format!("pelago-as-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (i, (text, _, build)) in golden().into_iter().enumerate() {
        let s = dir.join(format!("c{i}.s"));
        let o = dir.join(format!("c{i}.o"));
        let b = dir.join(format!("c{i}.bin"));
        std::fs::write(&s, format!(".intel_syntax noprefix\n{text}\n")).unwrap();
        let ok = std::process::Command::new("as")
            .args(["--64", "-o"])
            .arg(&o)
            .arg(&s)
            .status()
            .unwrap()
            .success();
        assert!(ok, "as failed on {text}");
        let ok = std::process::Command::new("objcopy")
            .args(["-O", "binary", "-j", ".text"])
            .arg(&o)
            .arg(&b)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let mut a = Asm::new();
        build(&mut a);
        assert_eq!(std::fs::read(&b).unwrap(), a.code, "{text}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- Running real executables (Linux on x86-64 only) -----------------------------------------------

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod run {
    use super::super::elf::{self, Blob};
    use super::super::rt::{self, Runtime};
    use super::super::x86::*;

    /// A program: the runtime, then `body` as `main`.
    pub fn program(leak: bool, body: impl FnOnce(&mut Asm, &mut Blob, &Runtime)) -> Vec<u8> {
        let (mut a, mut ro, mut data) = (Asm::new(), Blob::default(), Blob::default());
        let main = a.new_label();
        let rt = rt::emit(
            &mut a,
            &mut ro,
            &mut data,
            main,
            leak,
            super::super::Os::Linux,
        );
        a.bind(main);
        body(&mut a, &mut ro, &rt);
        elf::write(&a, &ro, &data, 0, &[]).unwrap()
    }

    pub use super::super::exec::Ran;

    pub fn run(image: &[u8]) -> Ran {
        super::super::exec::run_image(super::super::Os::Linux, image).unwrap()
    }

    /// Code that writes `text` to standard output.
    pub fn say(a: &mut Asm, ro: &mut Blob, rt: &Runtime, text: &str) {
        let off = ro.add(text.as_bytes(), 1);
        a.mov_ri(RDI, 1);
        a.lea_sect(RSI, Sect::Rodata, off);
        a.mov_ri(RDX, text.len() as i64);
        a.call(rt.write_fd);
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod native_run {
    use super::run::*;
    use super::*;

    #[test]
    fn hello_world_is_a_real_executable() {
        let img = program(true, |a, ro, rt| {
            say(a, ro, rt, "hello, native world\n");
            a.ret();
        });
        assert_eq!(&img[..4], b"\x7fELF");
        let r = run(&img);
        assert_eq!(
            (r.out.as_str(), r.err.as_str(), r.code),
            ("hello, native world\n", "", 0)
        );
    }

    #[test]
    fn stop_prints_a_message_and_exits_with_one() {
        let img = program(false, |a, ro, rt| {
            say(a, ro, rt, "before\n");
            let m = ro.add(b"boom", 1);
            a.lea_sect(RDI, Sect::Rodata, m);
            a.mov_ri(RSI, 4);
            a.call(rt.stop);
        });
        let r = run(&img);
        assert_eq!(
            (r.out.as_str(), r.err.as_str(), r.code),
            ("before\n", "stopped: boom\n", 1)
        );
    }

    #[test]
    fn the_allocator_reuses_clears_and_counts() {
        let img = program(true, |a, ro, rt| {
            a.push(RBX);
            let bad = a.new_label();
            let end = a.new_label();
            // A freed block of the same class comes back, zeroed.
            a.mov_ri(RDI, 100);
            a.call(rt.alloc);
            a.mov_rr(RBX, RAX, 8);
            a.store_imm(mem(RBX, 0), 0x1234_5678, 8);
            a.store_imm(mem(RBX, 99), 7, 1);
            a.mov_rr(RDI, RBX, 8);
            a.call(rt.free);
            a.mov_ri(RDI, 100);
            a.call(rt.alloc);
            a.alu_rr(Alu::Cmp, RAX, RBX, 8);
            a.jcc(Cc::Ne, bad);
            a.load(RCX, mem(RAX, 0), 8);
            a.test_rr(RCX, RCX, 8);
            a.jcc(Cc::Ne, bad);
            a.load_ext(RCX, mem(RAX, 99), 1, false, false);
            a.test_rr(RCX, RCX, 4);
            a.jcc(Cc::Ne, bad);
            a.mov_rr(RDI, RAX, 8);
            a.call(rt.free);
            // A large block has its own mapping.
            a.mov_ri(RDI, 200_000);
            a.call(rt.alloc);
            a.mov_rr(RBX, RAX, 8);
            a.store_imm(mem(RBX, 199_999), 1, 1);
            a.mov_rr(RDI, RBX, 8);
            a.call(rt.free);
            // Free of null is fine; a zero-size allocation is a real block.
            a.alu_rr(Alu::Xor, RDI, RDI, 4);
            a.call(rt.free);
            a.alu_rr(Alu::Xor, RDI, RDI, 4);
            a.call(rt.alloc);
            a.mov_rr(RDI, RAX, 8);
            a.call(rt.free);
            say(a, ro, rt, "ok\n");
            a.jmp(end);
            a.bind(bad);
            say(a, ro, rt, "bad\n");
            a.bind(end);
            a.pop(RBX);
            a.ret();
        });
        let r = run(&img);
        assert_eq!((r.out.as_str(), r.err.as_str(), r.code), ("ok\n", "", 0));
    }

    #[test]
    fn realloc_keeps_the_contents_and_zeroes_the_rest() {
        let img = program(true, |a, ro, rt| {
            a.push(RBX);
            let bad = a.new_label();
            let end = a.new_label();
            // Null allocates (a small block).
            a.alu_rr(Alu::Xor, RDI, RDI, 4);
            a.mov_ri(RSI, 100);
            a.call(rt.realloc);
            a.mov_rr(RBX, RAX, 8);
            a.store_imm(mem(RBX, 99), 7, 1);
            // Small to large: the old byte comes along, the new tail is zero.
            a.mov_rr(RDI, RBX, 8);
            a.mov_ri(RSI, 300_000);
            a.call(rt.realloc);
            a.mov_rr(RBX, RAX, 8);
            a.load_ext(RCX, mem(RBX, 99), 1, false, false);
            a.alu_ri(Alu::Cmp, RCX, 7, 4);
            a.jcc(Cc::Ne, bad);
            a.load_ext(RCX, mem(RBX, 299_999), 1, false, false);
            a.test_rr(RCX, RCX, 4);
            a.jcc(Cc::Ne, bad);
            a.store_imm(mem(RBX, 299_999), 9, 1);
            // Large to larger: remapped, nothing lost, the tail is zero.
            a.mov_rr(RDI, RBX, 8);
            a.mov_ri(RSI, 3_000_000);
            a.call(rt.realloc);
            a.mov_rr(RBX, RAX, 8);
            a.load_ext(RCX, mem(RBX, 99), 1, false, false);
            a.alu_ri(Alu::Cmp, RCX, 7, 4);
            a.jcc(Cc::Ne, bad);
            a.load_ext(RCX, mem(RBX, 299_999), 1, false, false);
            a.alu_ri(Alu::Cmp, RCX, 9, 4);
            a.jcc(Cc::Ne, bad);
            a.load_ext(RCX, mem(RBX, 2_999_999), 1, false, false);
            a.test_rr(RCX, RCX, 4);
            a.jcc(Cc::Ne, bad);
            // Large to small: the first bytes stay.
            a.mov_rr(RDI, RBX, 8);
            a.mov_ri(RSI, 150);
            a.call(rt.realloc);
            a.mov_rr(RBX, RAX, 8);
            a.load_ext(RCX, mem(RBX, 99), 1, false, false);
            a.alu_ri(Alu::Cmp, RCX, 7, 4);
            a.jcc(Cc::Ne, bad);
            // Small to small of another class.
            a.mov_rr(RDI, RBX, 8);
            a.mov_ri(RSI, 5_000);
            a.call(rt.realloc);
            a.mov_rr(RBX, RAX, 8);
            a.load_ext(RCX, mem(RBX, 99), 1, false, false);
            a.alu_ri(Alu::Cmp, RCX, 7, 4);
            a.jcc(Cc::Ne, bad);
            a.mov_rr(RDI, RBX, 8);
            a.call(rt.free);
            say(a, ro, rt, "ok\n");
            a.jmp(end);
            a.bind(bad);
            say(a, ro, rt, "bad\n");
            a.bind(end);
            a.pop(RBX);
            a.ret();
        });
        // The leak check runs too: every block was freed or replaced.
        let r = run(&img);
        assert_eq!((r.out.as_str(), r.err.as_str(), r.code), ("ok\n", "", 0));
    }

    #[test]
    fn many_small_blocks_cross_chunks() {
        let img = program(true, |a, ro, rt| {
            a.push(RBX);
            a.push(R12);
            a.alu_rr(Alu::Xor, RBX, RBX, 4);
            a.mov_ri(R12, 100_000);
            let (alloc_loop, free_loop, done) = (a.new_label(), a.new_label(), a.new_label());
            a.bind(alloc_loop);
            a.mov_ri(RDI, 16);
            a.call(rt.alloc);
            a.store(mem(RAX, 0), RBX, 8);
            a.mov_rr(RBX, RAX, 8);
            a.alu_ri(Alu::Sub, R12, 1, 8);
            a.jcc(Cc::Ne, alloc_loop);
            a.bind(free_loop);
            a.test_rr(RBX, RBX, 8);
            a.jcc(Cc::E, done);
            a.mov_rr(RDI, RBX, 8);
            a.load(RBX, mem(RBX, 0), 8);
            a.call(rt.free);
            a.jmp(free_loop);
            a.bind(done);
            say(a, ro, rt, "freed all\n");
            a.pop(R12);
            a.pop(RBX);
            a.ret();
        });
        let r = run(&img);
        assert_eq!(
            (r.out.as_str(), r.err.as_str(), r.code),
            ("freed all\n", "", 0)
        );
    }

    #[test]
    fn a_leak_is_reported_with_status_three() {
        let img = program(true, |a, _ro, rt| {
            a.mov_ri(RDI, 8);
            a.call(rt.alloc);
            a.mov_ri(RDI, 8);
            a.call(rt.alloc);
            a.ret();
        });
        let r = run(&img);
        assert_eq!((r.err.as_str(), r.code), ("leaked 2 allocation(s)\n", 3));
    }

    #[test]
    fn numbers_are_written_in_decimal() {
        let img = program(false, |a, ro, rt| {
            for n in [0i64, 7, 10, 1_234_567_890_123] {
                a.mov_ri(RDI, 1);
                a.mov_ri(RSI, n);
                a.call(rt.write_u64);
                say(a, ro, rt, " ");
            }
            a.ret();
        });
        assert_eq!(run(&img).out, "0 7 10 1234567890123 ");
    }

    #[test]
    fn the_file_is_readable_by_standard_tools() {
        let img = program(false, |a, _ro, _rt| a.ret());
        let path = std::env::temp_dir().join(format!("pelago-objdump-{}", std::process::id()));
        std::fs::write(&path, &img).unwrap();
        if let Ok(o) = std::process::Command::new("readelf")
            .arg("-lS")
            .arg(&path)
            .output()
        {
            if o.status.success() {
                let text = String::from_utf8_lossy(&o.stdout);
                assert!(text.contains(".text") && text.contains("LOAD"), "{text}");
            }
        }
        let _ = std::fs::remove_file(&path);
    }
}

/// A frame slot is zero when it comes into use, unless the code after it writes every byte first
/// (then the clearing is left out). A slot written only in part must still read as zero: the stack
/// under it is dirtied first by another function.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn slots_read_as_zero_unless_overwritten() {
    use super::gen;
    use crate::ir::*;
    let mut m = Module::default();
    // dirty(): fills 128 bytes of its frame with 0x41.
    let mut dirty = Func::new("dirty", &[], IrTy::Void);
    let b = dirty.entry();
    let s = dirty.emit(b, Op::Slot(128, 8), Some(IrTy::Ptr)).unwrap();
    let k = dirty
        .emit(
            b,
            Op::Const(IrTy::I64, Const::Int(0x4141414141414141)),
            Some(IrTy::I64),
        )
        .unwrap();
    for i in 0..16 {
        let off = dirty
            .emit(b, Op::Const(IrTy::I64, Const::Int(8 * i)), Some(IrTy::I64))
            .unwrap();
        let p = dirty.emit(b, Op::PtrAdd(s, off), Some(IrTy::Ptr)).unwrap();
        dirty.emit(b, Op::Store(p, k), None);
    }
    dirty.terminate(b, Term::Return(None));
    let dirty = m.add(dirty);
    // part(): a 32-byte slot, only the first and the last 8 bytes written; returns the sum of all four words.
    // full(): every byte written, in a different order; returns the sum of the four words.
    let mut funcs = Vec::new();
    for (name, offsets) in [("part", vec![0, 24]), ("full", vec![24, 8, 0, 16])] {
        let mut f = Func::new(name, &[], IrTy::I64);
        let b = f.entry();
        let s = f.emit(b, Op::Slot(32, 8), Some(IrTy::Ptr)).unwrap();
        let mut addr = Vec::new();
        for i in 0..4 {
            let off = f
                .emit(b, Op::Const(IrTy::I64, Const::Int(8 * i)), Some(IrTy::I64))
                .unwrap();
            addr.push(f.emit(b, Op::PtrAdd(s, off), Some(IrTy::Ptr)).unwrap());
        }
        for (n, o) in offsets.iter().enumerate() {
            let v = f
                .emit(
                    b,
                    Op::Const(IrTy::I64, Const::Int(n as i128 + 1)),
                    Some(IrTy::I64),
                )
                .unwrap();
            f.emit(b, Op::Store(addr[o / 8], v), None);
        }
        let mut sum = f
            .emit(b, Op::Load(IrTy::I64, addr[0]), Some(IrTy::I64))
            .unwrap();
        for a in &addr[1..] {
            let x = f.emit(b, Op::Load(IrTy::I64, *a), Some(IrTy::I64)).unwrap();
            sum = f
                .emit(b, Op::Bin(BinOp::Add, sum, x), Some(IrTy::I64))
                .unwrap();
        }
        f.terminate(b, Term::Return(Some(sum)));
        funcs.push(m.add(f));
    }
    // main: dirty the stack, then print the two sums as digits.
    let mut main = Func::new("main", &[], IrTy::Void);
    let b = main.entry();
    main.emit(b, Op::Call(dirty, vec![]), None);
    let out = main.emit(b, Op::Slot(2, 1), Some(IrTy::Ptr)).unwrap();
    for (i, fid) in funcs.iter().enumerate() {
        let r = main
            .emit(b, Op::Call(*fid, vec![]), Some(IrTy::I64))
            .unwrap();
        let d = main
            .emit(b, Op::Const(IrTy::I64, Const::Int(48)), Some(IrTy::I64))
            .unwrap();
        let c = main
            .emit(b, Op::Bin(BinOp::Add, r, d), Some(IrTy::I64))
            .unwrap();
        let c8 = main
            .emit(b, Op::Cast(CastKind::Trunc, c, IrTy::I8), Some(IrTy::I8))
            .unwrap();
        let off = main
            .emit(
                b,
                Op::Const(IrTy::I64, Const::Int(i as i128)),
                Some(IrTy::I64),
            )
            .unwrap();
        let p = main.emit(b, Op::PtrAdd(out, off), Some(IrTy::Ptr)).unwrap();
        main.emit(b, Op::Store(p, c8), None);
    }
    let two = main
        .emit(b, Op::Const(IrTy::I64, Const::Int(2)), Some(IrTy::I64))
        .unwrap();
    main.emit(b, Op::Print(out, two), None);
    main.terminate(b, Term::Return(None));
    m.add(main);
    // part() = 1 + 2 = 3 (the middle words are zero); full() = 1 + 2 + 3 + 4 = 10, as the digit '0' + 10 = ':'.
    for regalloc in [false, true] {
        let opts = gen::Options {
            leak_check: false,
            target: super::Os::Linux,
            regalloc,
        };
        let image = gen::compile(&m, "main", &opts).unwrap();
        let r = super::exec::run_image(super::Os::Linux, &image).unwrap();
        assert_eq!(
            (r.out.as_str(), r.err.as_str(), r.code),
            ("3:", "", 0),
            "regalloc {regalloc}"
        );
    }
}
