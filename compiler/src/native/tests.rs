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

/// When `as` and `objcopy` are installed, ask them again (the golden bytes could be stale).
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
