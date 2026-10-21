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
/// Linux on x86-64: the tools that come with other systems may default to other settings.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
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
    fn a_bad_address_is_a_stop_not_a_crash() {
        let img = program(false, |a, ro, rt| {
            say(a, ro, rt, "before\n");
            a.mov_ri(RAX, 8);
            a.load(RCX, mem(RAX, 0), 8);
            say(a, ro, rt, "not reached\n");
        });
        let r = run(&img);
        assert_eq!(
            (r.out.as_str(), r.err.as_str(), r.code),
            ("before\n", "stopped: invalid memory access\n", 1)
        );
    }

    #[test]
    fn running_out_of_stack_is_a_stop() {
        let img = program(false, |a, ro, rt| {
            // Every call pushes a return address (and 8 more bytes); nothing ever returns.
            let f = a.new_label();
            a.bind(f);
            a.push(RAX);
            a.call(f);
            say(a, ro, rt, "not reached\n");
        });
        let r = run(&img);
        assert_eq!(
            (r.out.as_str(), r.err.as_str(), r.code),
            ("", "stopped: stack overflow\n", 1)
        );
    }

    #[test]
    fn buffered_output_comes_out_before_the_stop_message() {
        let img = program(false, |a, ro, rt| {
            let m = ro.add(b"printed\n", 1);
            a.lea_sect(RSI, Sect::Rodata, m);
            a.mov_ri(RDX, 8);
            a.call(rt.print);
            let m = ro.add(b"boom", 1);
            a.lea_sect(RDI, Sect::Rodata, m);
            a.mov_ri(RSI, 4);
            a.call(rt.stop);
        });
        let r = run(&img);
        assert_eq!(
            (r.out.as_str(), r.err.as_str(), r.code),
            ("printed\n", "stopped: boom\n", 1)
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
    if let Some(r) = super::exec::build_and_run_a64(&m, false) {
        assert_eq!(
            (r.out.as_str(), r.err.as_str(), r.code),
            ("3:", "", 0),
            "AArch64"
        );
    }
}

// ---- AArch64 encoder (card A-10, stage R1) ----------------------------------------------------------

fn words(bytes: &[u8]) -> String {
    bytes
        .chunks(4)
        .map(|w| w.iter().map(|b| format!("{b:02x}")).collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `a.code` as the little-endian words in the golden file's notation (grouped by instruction word).
fn a64_words(build: fn(&mut super::a64::Asm)) -> String {
    let mut a = super::a64::Asm::new();
    build(&mut a);
    let code = a
        .finish(super::a64::SectAddrs {
            text: 0,
            rodata: 0,
            data: 0,
            idata: 0,
        })
        .unwrap();
    // The assembler prints the bytes of each word in memory order too.
    words(&code)
}

#[test]
fn every_a64_encoding_matches_the_assembler() {
    for (text, want, build) in super::a64_golden::golden() {
        let want = want.split(' ').collect::<Vec<_>>().join("");
        let got = a64_words(build).split(' ').collect::<Vec<_>>().join("");
        assert_eq!(got, want, "{text}");
    }
}

/// When the AArch64 assembler is installed, ask it again (the golden words could be stale).
#[cfg(target_os = "linux")]
#[test]
fn the_installed_a64_assembler_agrees() {
    let have = |tool: &str| {
        std::process::Command::new(tool)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    };
    // On an ARM machine the ordinary tools are the AArch64 ones; elsewhere the cross tools are.
    let (asm, objcopy) = if cfg!(target_arch = "aarch64") {
        ("as", "objcopy")
    } else {
        ("aarch64-linux-gnu-as", "aarch64-linux-gnu-objcopy")
    };
    if !(have(asm) && have(objcopy)) {
        return;
    }
    let dir = std::env::temp_dir().join(format!("pelago-a64as-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (i, (text, _, build)) in super::a64_golden::golden().into_iter().enumerate() {
        let s = dir.join(format!("c{i}.s"));
        let o = dir.join(format!("c{i}.o"));
        let b = dir.join(format!("c{i}.bin"));
        std::fs::write(&s, format!("{text}\n")).unwrap();
        let ok = std::process::Command::new(asm)
            .arg("-o")
            .arg(&o)
            .arg(&s)
            .status()
            .unwrap()
            .success();
        assert!(ok, "as failed on {text}");
        let ok = std::process::Command::new(objcopy)
            .args(["-O", "binary", "-j", ".text"])
            .arg(&o)
            .arg(&b)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        assert_eq!(
            words(&std::fs::read(&b).unwrap()),
            a64_words(build),
            "{text}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- Running real AArch64 executables (natively on ARM, under qemu elsewhere) ------------------------

mod a64_run {
    pub use super::super::a64::*;
    use super::super::elf::{self, Blob};
    use super::super::exec::{self, Ran};
    use super::super::macho;
    use super::super::pe;
    use super::super::rt_a64::{self, Runtime};
    use super::super::{Arch, Os};

    /// The system these programs are built for: Windows on a Windows ARM machine, Linux elsewhere.
    pub fn os() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        }
    }

    /// A program: the runtime, then `body` as `main` (which may use `x19` to `x22` and must end in
    /// `ret`; its link register is saved for it).
    pub fn program(
        leak: bool,
        body: impl FnOnce(&mut Asm, &mut Blob, &mut Blob, &Runtime),
    ) -> Vec<u8> {
        let (mut a, mut ro, mut data) = (Asm::new(), Blob::default(), Blob::default());
        let main = a.new_label();
        let rt = rt_a64::emit(&mut a, &mut ro, &mut data, main, leak, os());
        a.bind(main);
        a.stp_pre(FP, LR, SP, -16);
        body(&mut a, &mut ro, &mut data, &rt);
        a.ldp_post(FP, LR, SP, 16);
        a.ret();
        match os() {
            Os::Linux => elf::write_a64(&a, &ro, &data, 0, &[]).unwrap(),
            Os::Windows => pe::write_a64(&a, &ro, &data, 0).unwrap(),
            Os::MacOs => macho::write_a64(&a, &ro, &data, 0).unwrap(),
        }
    }

    /// Runs the image, or `None` where AArch64 programs cannot run.
    pub fn run(image: &[u8]) -> Option<Ran> {
        exec::can_run_a64_on(os())
            .then(|| exec::run_image_arch(Arch::Aarch64, os(), image).unwrap())
    }

    /// Code that writes `text` to standard output.
    pub fn say(a: &mut Asm, ro: &mut Blob, rt: &Runtime, text: &str) {
        let off = ro.add(text.as_bytes(), 1);
        a.mov_ri(X0, 1);
        a.lea_sect(X1, Sect::Rodata, off);
        a.mov_ri(X2, text.len() as i64);
        a.bl(rt.write_fd);
    }
}

#[test]
fn a64_hello_world_is_a_real_executable() {
    use a64_run::*;
    let img = program(true, |a, ro, _, rt| say(a, ro, rt, "hello, arm world\n"));
    if os() == super::Os::Linux {
        assert_eq!(&img[..4], b"\x7fELF");
        assert_eq!(&img[18..20], &[183, 0], "EM_AARCH64");
    } else if os() == super::Os::MacOs {
        assert_eq!(&img[..4], &[0xCF, 0xFA, 0xED, 0xFE], "MH_MAGIC_64");
    } else {
        assert_eq!(&img[..2], b"MZ");
        assert_eq!(&img[0x44..0x46], &[0x64, 0xAA], "IMAGE_FILE_MACHINE_ARM64");
    }
    if let Some(r) = run(&img) {
        assert_eq!(
            (r.out.as_str(), r.err.as_str(), r.code),
            ("hello, arm world\n", "", 0)
        );
    }
}

#[test]
fn a64_stop_prints_a_message_and_exits_with_one() {
    use a64_run::*;
    let img = program(false, |a, ro, _, rt| {
        say(a, ro, rt, "before\n");
        let m = ro.add(b"boom", 1);
        a.lea_sect(X0, Sect::Rodata, m);
        a.mov_ri(X1, 4);
        a.bl(rt.stop);
    });
    if let Some(r) = run(&img) {
        assert_eq!(
            (r.out.as_str(), r.err.as_str(), r.code),
            ("before\n", "stopped: boom\n", 1)
        );
    }
}

#[test]
fn a64_a_bad_address_is_a_stop_not_a_crash() {
    use a64_run::*;
    let img = program(false, |a, ro, _, rt| {
        say(a, ro, rt, "before\n");
        a.mov_ri(X0, 8);
        a.ldr(X1, X0, 0, 8);
        say(a, ro, rt, "not reached\n");
    });
    if let Some(r) = run(&img) {
        assert_eq!(
            (r.out.as_str(), r.err.as_str(), r.code),
            ("before\n", "stopped: invalid memory access\n", 1)
        );
    }
}

#[test]
fn a64_running_out_of_stack_is_a_stop() {
    use a64_run::*;
    let img = program(false, |a, ro, _, rt| {
        // Every call pushes a frame record; nothing ever returns.
        let f = a.new_label();
        let go = a.new_label();
        a.b(go);
        a.bind(f);
        a.stp_pre(FP, LR, SP, -16);
        a.bl(f);
        a.bind(go);
        a.bl(f);
        say(a, ro, rt, "not reached\n");
    });
    if let Some(r) = run(&img) {
        assert_eq!(
            (r.out.as_str(), r.err.as_str(), r.code),
            ("", "stopped: stack overflow\n", 1)
        );
    }
}

#[test]
fn a64_buffered_output_comes_out_before_the_stop_message() {
    use a64_run::*;
    let img = program(false, |a, ro, _, rt| {
        let m = ro.add(b"printed\n", 1);
        a.lea_sect(X0, Sect::Rodata, m);
        a.mov_ri(X1, 8);
        a.bl(rt.print);
        let m = ro.add(b"boom", 1);
        a.lea_sect(X0, Sect::Rodata, m);
        a.mov_ri(X1, 4);
        a.bl(rt.stop);
    });
    if let Some(r) = run(&img) {
        assert_eq!(
            (r.out.as_str(), r.err.as_str(), r.code),
            ("printed\n", "stopped: boom\n", 1)
        );
    }
}

#[test]
fn a64_the_allocator_reuses_clears_and_counts() {
    use a64_run::*;
    let img = program(true, |a, ro, _, rt| {
        let (bad, end) = (a.new_label(), a.new_label());
        // A freed block of the same class comes back, zeroed.
        a.mov_ri(X0, 100);
        a.bl(rt.alloc);
        a.mov_rr(X19, X0);
        a.mov_ri(X9, 0x55);
        a.str(X9, X19, 0, 1);
        a.str(X9, X19, 99, 1);
        a.mov_rr(X0, X19);
        a.bl(rt.free);
        a.mov_ri(X0, 100);
        a.bl(rt.alloc);
        a.cmp_rr(X0, X19);
        a.b_cond(Cond::Ne, bad);
        a.ldr(X9, X0, 0, 1);
        a.cbnz(X9, bad);
        a.ldr(X9, X0, 99, 1);
        a.cbnz(X9, bad);
        a.tst_ri(X0, 15);
        a.b_cond(Cond::Ne, bad);
        a.bl(rt.free);
        say(a, ro, rt, "reused\n");
        // Free of null is fine.
        a.mov_ri(X0, 0);
        a.bl(rt.free);
        // 100,000 small blocks across several chunks, all distinct, then all freed.
        a.mov_ri(X0, 100_000 * 8);
        a.bl(rt.alloc);
        a.mov_rr(X20, X0);
        a.mov_ri(X21, 0);
        let (fill, fdone) = (a.new_label(), a.new_label());
        a.bind(fill);
        a.mov_ri(X9, 100_000);
        a.cmp_rr(X21, X9);
        a.b_cond(Cond::Hs, fdone);
        a.mov_ri(X0, 40);
        a.bl(rt.alloc);
        a.str_idx(X0, X20, X21, 8, true);
        a.add_ri(X21, X21, 1);
        a.b(fill);
        a.bind(fdone);
        a.mov_ri(X21, 0);
        let (rel, rdone) = (a.new_label(), a.new_label());
        a.bind(rel);
        a.mov_ri(X9, 100_000);
        a.cmp_rr(X21, X9);
        a.b_cond(Cond::Hs, rdone);
        a.ldr_idx(X0, X20, X21, 8, true);
        a.bl(rt.free);
        a.add_ri(X21, X21, 1);
        a.b(rel);
        a.bind(rdone);
        a.mov_rr(X0, X20);
        a.bl(rt.free);
        say(a, ro, rt, "many\n");
        a.b(end);
        a.bind(bad);
        say(a, ro, rt, "bad\n");
        a.bind(end);
    });
    if let Some(r) = run(&img) {
        assert_eq!(
            (r.out.as_str(), r.err.as_str(), r.code),
            ("reused\nmany\n", "", 0)
        );
    }
}

#[test]
fn a64_a_leak_is_reported_with_status_three() {
    use a64_run::*;
    let img = program(true, |a, _, _, rt| {
        a.mov_ri(X0, 24);
        a.bl(rt.alloc);
    });
    if let Some(r) = run(&img) {
        assert_eq!(
            (r.out.as_str(), r.err.as_str(), r.code),
            ("", "leaked 1 allocation(s)\n", 3)
        );
    }
}

#[test]
fn a64_large_blocks_grow_in_place_and_keep_their_contents() {
    use a64_run::*;
    let img = program(true, |a, ro, _, rt| {
        let bad = a.new_label();
        let end = a.new_label();
        // 4 MiB, marked at both ends, grown to 64 MiB: the marks stay and the new tail is zero.
        a.mov_ri(X0, 4 << 20);
        a.bl(rt.alloc);
        a.mov_rr(X19, X0);
        a.mov_ri(X9, 0x7B);
        a.str(X9, X19, 0, 1);
        a.mov_ri(X10, (4 << 20) - 1);
        a.str_idx(X9, X19, X10, 1, false);
        a.mov_rr(X0, X19);
        a.mov_ri(X1, 64 << 20);
        a.bl(rt.realloc);
        a.mov_rr(X19, X0);
        a.ldr(X9, X19, 0, 1);
        a.cmp_ri(X9, 0x7B);
        a.b_cond(Cond::Ne, bad);
        a.mov_ri(X10, (4 << 20) - 1);
        a.ldr_idx(X9, X19, X10, 1, false);
        a.cmp_ri(X9, 0x7B);
        a.b_cond(Cond::Ne, bad);
        a.mov_ri(X10, 40 << 20);
        a.ldr_idx(X9, X19, X10, 1, false);
        a.cbnz(X9, bad);
        // A small block grown past its class keeps its bytes (the copying path).
        a.mov_rr(X0, X19);
        a.bl(rt.free);
        a.mov_ri(X0, 40);
        a.bl(rt.alloc);
        a.mov_rr(X19, X0);
        a.mov_ri(X9, 0x42);
        a.str(X9, X19, 39, 1);
        a.mov_rr(X0, X19);
        a.mov_ri(X1, 5000);
        a.bl(rt.realloc);
        a.mov_rr(X19, X0);
        a.ldr(X9, X19, 39, 1);
        a.cmp_ri(X9, 0x42);
        a.b_cond(Cond::Ne, bad);
        a.ldr(X9, X19, 4000, 1);
        a.cbnz(X9, bad);
        a.mov_rr(X0, X19);
        a.bl(rt.free);
        // Realloc of null allocates.
        a.mov_ri(X0, 0);
        a.mov_ri(X1, 64);
        a.bl(rt.realloc);
        a.bl(rt.free);
        say(a, ro, rt, "grown\n");
        a.b(end);
        a.bind(bad);
        say(a, ro, rt, "bad\n");
        a.bind(end);
    });
    if let Some(r) = run(&img) {
        assert_eq!((r.out.as_str(), r.err.as_str(), r.code), ("grown\n", "", 0));
    }
}

/// A large block grown in several steps (inside its reservation where the runtime keeps one), shrunk
/// and grown again: the bytes stay and every new tail is zero.
#[test]
fn a64_large_blocks_grow_in_steps_and_shrink() {
    use a64_run::*;
    let img = program(true, |a, ro, _, rt| {
        let bad = a.new_label();
        let end = a.new_label();
        a.mov_ri(X0, 1 << 20);
        a.bl(rt.alloc);
        a.mov_rr(X19, X0);
        a.mov_ri(X9, 0x5A);
        a.str(X9, X19, 0, 1);
        a.mov_ri(X10, (1 << 20) - 1);
        a.str_idx(X9, X19, X10, 1, false);
        // 1 MiB, then 2, 4, 6 and 7 MiB (inside eight times its length), then 40 MiB (outside it).
        for mib in [2i64, 4, 6, 7, 40] {
            a.mov_rr(X0, X19);
            a.mov_ri(X1, mib << 20);
            a.bl(rt.realloc);
            a.mov_rr(X19, X0);
            a.ldr(X9, X19, 0, 1);
            a.cmp_ri(X9, 0x5A);
            a.b_cond(Cond::Ne, bad);
            a.mov_ri(X10, (1 << 20) - 1);
            a.ldr_idx(X9, X19, X10, 1, false);
            a.cmp_ri(X9, 0x5A);
            a.b_cond(Cond::Ne, bad);
            // The byte just past the old end, and the last byte, are zero.
            a.mov_ri(X10, 1 << 20);
            a.ldr_idx(X9, X19, X10, 1, false);
            a.cbnz(X9, bad);
            a.mov_ri(X10, (mib << 20) - 1);
            a.ldr_idx(X9, X19, X10, 1, false);
            a.cbnz(X9, bad);
            // Dirty the end, so a later growth that reuses the pages would show it.
            a.mov_ri(X9, 0x77);
            a.str_idx(X9, X19, X10, 1, false);
            a.mov_ri(X10, (mib << 20) - 4096);
            a.str_idx(X9, X19, X10, 1, false);
        }
        // Back to 2 MiB (the tail is gone), then to 3 MiB: the tail is zero again.
        a.mov_rr(X0, X19);
        a.mov_ri(X1, 2 << 20);
        a.bl(rt.realloc);
        a.mov_rr(X19, X0);
        a.mov_rr(X0, X19);
        a.mov_ri(X1, 3 << 20);
        a.bl(rt.realloc);
        a.mov_rr(X19, X0);
        a.mov_ri(X10, (3 << 20) - 4096);
        a.ldr_idx(X9, X19, X10, 1, false);
        a.cbnz(X9, bad);
        a.mov_ri(X10, (2 << 20) + 100);
        a.ldr_idx(X9, X19, X10, 1, false);
        a.cbnz(X9, bad);
        a.mov_ri(X10, (1 << 20) - 1);
        a.ldr_idx(X9, X19, X10, 1, false);
        a.cmp_ri(X9, 0x5A);
        a.b_cond(Cond::Ne, bad);
        a.mov_rr(X0, X19);
        a.bl(rt.free);
        say(a, ro, rt, "stepped\n");
        a.b(end);
        a.bind(bad);
        say(a, ro, rt, "bad\n");
        a.bind(end);
    });
    if let Some(r) = run(&img) {
        assert_eq!(
            (r.out.as_str(), r.err.as_str(), r.code),
            ("stepped\n", "", 0)
        );
    }
}

/// `fmod` bit by bit against Rust's `%` for the edge cases and 30,000 random pairs.
#[test]
fn a64_fmod_is_exact() {
    use a64_run::*;
    let mut cases: Vec<(f64, f64)> = Vec::new();
    let edges = [
        0.0,
        -0.0,
        1.0,
        -1.0,
        0.5,
        2.0,
        3.0,
        7.5,
        -7.5,
        0.1,
        0.03,
        1e300,
        -1e300,
        1e-300,
        5e-324,
        -5e-324,
        2.2250738585072014e-308,
        1.7976931348623157e308,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
        4.9e-320,
        3e-310,
        123456789.123,
        1e15,
        1e-15,
    ];
    for &x in &edges {
        for &y in &edges {
            cases.push((x, y));
        }
    }
    let mut s = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    for i in 0..30_000 {
        let (x, y) = (next(), next());
        // Mix whole random bit patterns with close exponents (the interesting loops) and small ones.
        let (x, y) = match i % 3 {
            0 => (f64::from_bits(x), f64::from_bits(y)),
            1 => (
                f64::from_bits(x),
                f64::from_bits((y & 0x800F_FFFF_FFFF_FFFF) | (x & 0x7FF0_0000_0000_0000)),
            ),
            _ => (
                (x % 1_000_000) as f64 / 64.0,
                (y % 1000) as f64 / 8.0 + 0.125,
            ),
        };
        cases.push((x, y));
    }
    let n = cases.len();
    let img = program(false, |a, ro, _, rt| {
        // Each entry: x, y and the answer Rust gives; the guest compares bits (any NaN is a NaN).
        let mut table = Vec::new();
        for (x, y) in &cases {
            for v in [*x, *y, *x % *y] {
                table.extend_from_slice(&v.to_bits().to_le_bytes());
            }
        }
        let t = ro.add(&table, 16);
        a.lea_sect(X19, Sect::Rodata, t);
        a.mov_ri(X20, 0);
        let (top, ok, bad, done) = (a.new_label(), a.new_label(), a.new_label(), a.new_label());
        a.bind(top);
        a.mov_ri(X9, n as i64);
        a.cmp_rr(X20, X9);
        a.b_cond(Cond::Hs, done);
        a.ldr_f(V(0), X19, 0, 8);
        a.ldr_f(V(1), X19, 8, 8);
        a.bl(rt.fmod);
        a.ldr(X10, X19, 16, 8);
        a.fmov_to_gp(true, X9, V(0));
        a.cmp_rr(X9, X10);
        a.b_cond(Cond::Eq, ok);
        // Different bits are fine when both are NaNs.
        a.fcmp(true, V(0), V(0));
        a.b_cond(Cond::Vc, bad);
        a.fmov_from_gp(true, V(1), X10);
        a.fcmp(true, V(1), V(1));
        a.b_cond(Cond::Vc, bad);
        a.bind(ok);
        a.add_ri(X19, X19, 24);
        a.add_ri(X20, X20, 1);
        a.b(top);
        a.bind(bad);
        // Say which pair was wrong.
        a.mov_ri(X0, 1);
        a.mov_rr(X1, X20);
        a.bl(rt.write_u64);
        a.mov_ri(X0, 1);
        a.bl(rt.exit);
        a.bind(done);
        say(a, ro, rt, "exact\n");
    });
    if let Some(r) = a64_run::run(&img) {
        assert_eq!(
            (r.out.as_str(), r.err.as_str(), r.code),
            ("exact\n", "", 0),
            "the first pair that differs is {:?}",
            r.out.trim().parse::<usize>().ok().map(|i| cases[i])
        );
    }
}
