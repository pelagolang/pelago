//! `pelago run` through the real binary: native by default (in-process on Linux), the interpreter
//! with `--interp`, and the temporary-file path with `--spawn`. All must agree.

use std::process::Command;

static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn run(flags: &[&str], src: &str) -> (String, String, i32) {
    let dir = std::env::temp_dir().join(format!(
        "pelago-run-cli-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("prog.pel");
    std::fs::write(&file, src).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_pelago"))
        .arg("run")
        .args(flags)
        .arg(&file)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn modes_agree_on_output() {
    let src = "fn main() {\n    print(\"hello\")\n}\n";
    for flags in [&[][..], &["--interp"], &["--spawn"], &["--native"]] {
        let (out, _, code) = run(flags, src);
        assert_eq!(out.trim_end(), "hello", "{flags:?}");
        assert_eq!(code, 0, "{flags:?}");
    }
}

#[test]
fn modes_agree_on_a_stop() {
    let src = "fn main() {\n    print(\"hi\")\n    var xs = [1, 2, 3]\n    print(xs[5])\n}\n";
    for flags in [&[][..], &["--interp"], &["--spawn"]] {
        let (out, err, code) = run(flags, src);
        assert_eq!(out.trim_end(), "hi", "{flags:?}");
        assert!(
            err.contains("stopped: index out of range"),
            "{flags:?}: {err}"
        );
        assert_eq!(code, 1, "{flags:?}");
    }
}

/// Like `run`, with program arguments (after `--`) and text on standard input.
fn run_with(flags: &[&str], src: &str, args: &[&str], stdin: &str) -> (String, String, i32) {
    use std::io::Write;
    let dir = std::env::temp_dir().join(format!(
        "pelago-run-cli-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("prog.pel");
    std::fs::write(&file, src).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_pelago"))
        .arg("run")
        .args(flags)
        .arg(&file)
        .arg("--")
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// The modes that run the descriptor calls (standard streams, files, exit): the interpreter everywhere,
/// native code on every target the compiler builds for.
fn os_modes() -> Vec<&'static [&'static str]> {
    let mut v: Vec<&'static [&'static str]> = vec![&["--interp"]];
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        v.push(&[]);
        v.push(&["--spawn"]);
    }
    if cfg!(any(target_os = "windows", target_os = "macos"))
        || cfg!(all(target_os = "linux", target_arch = "aarch64"))
    {
        v.push(&[]);
    }
    v
}

/// The modes that also run `main(args)`: the start block is built on x86-64 Linux only.
fn args_modes() -> Vec<&'static [&'static str]> {
    let mut v: Vec<&'static [&'static str]> = vec![&["--interp"]];
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        v.push(&[]);
        v.push(&["--spawn"]);
    }
    v
}

#[test]
fn main_gets_its_arguments() {
    let src = "fn main(args: [string]) {\n    print(args.len())\n    var i = 1\n    while i < args.len() { print(args[i])\n        i += 1 }\n}\n";
    for flags in args_modes() {
        let (out, err, code) = run_with(flags, src, &["x", "y z"], "");
        assert_eq!(out, "3\nx\ny z\n", "{flags:?}: {err}");
        assert_eq!(code, 0, "{flags:?}");
    }
}

#[test]
fn environment_variables_are_read_from_the_start_block() {
    // The child process inherits these (a name no other test uses).
    std::env::set_var("PELAGO_ENV_TEST_A", "x=y z");
    std::env::set_var("PELAGO_ENV_TEST_B", "");
    let src = "import std.env\nfn main() {\n    print(env.get(\"PELAGO_ENV_TEST_A\"), env.get(\"PELAGO_ENV_TEST_B\"), env.get(\"PELAGO_ENV_TEST_NONE\"))\n    print(env.has(\"PELAGO_ENV_TEST_A\"), env.has(\"PELAGO_ENV_TEST_NONE\"))\n    var found = 0\n    for p of env.vars() { if p.0 == \"PELAGO_ENV_TEST_A\" { found += 1 } }\n    print(found)\n}\n";
    for flags in args_modes() {
        let (out, err, code) = run_with(flags, src, &[], "");
        assert_eq!(
            out, "some(\"x=y z\") some(\"\") none\ntrue false\n1\n",
            "{flags:?}: {err}"
        );
        assert_eq!(code, 0, "{flags:?}");
    }
}

#[test]
fn clocks_and_durations() {
    let src = "import std.time\n\nfn go() -> void ! io_error {\n    const a = time.now()?\n    const b = time.now()?\n    print(b.since(a) >= time.nanos(0))\n    print(time.seconds(90) + time.millis(500), time.millis(250), time.micros(42), time.nanos(7), -time.seconds(2))\n    print(time.seconds(3).asMillis(), time.millis(1500).asSeconds(), time.millis(1500).asSecondsFloat())\n    const w = time.sinceEpoch()?\n    print(w.asSeconds() > 1700000000)\n    print(a.plus(time.seconds(1)).since(a) == time.seconds(1), time.seconds(1) < time.seconds(2))\n    print((a.elapsed()?) >= time.nanos(0))\n}\n\nfn main() {\n    print(go().isOk())\n}\n";
    for flags in args_modes() {
        let (out, err, code) = run_with(flags, src, &[], "");
        assert_eq!(
            out, "true\n90.5s 250ms 42us 7ns -2s\n3000 1 1.5\ntrue\ntrue true\ntrue\ntrue\n",
            "{flags:?}: {err}"
        );
        assert_eq!(code, 0, "{flags:?}");
    }
}

#[test]
fn sleep_and_select_timeouts_use_the_clock() {
    let src = "import std.time\n\nfn go() -> void ! io_error {\n    const t0 = time.now()?\n    time.sleep(time.millis(30))\n    const waited = (t0.elapsed()?).asMillis()\n    print(waited >= 30)\n    const (tx, rx) = chan<int>()\n    select {\n        case v = rx.recv() { print(\"got\") }\n        case timeout(time.millis(20)) { print(\"timed out\") }\n    }\n    scope s {\n        const t = copy tx\n        s.spawn(() ~> { time.sleep(time.millis(10)); t.send(7) })\n        select {\n            case v = rx.recv() { print(\"got #(v ?? 0)\") }\n            case timeout(time.millis(500)) { print(\"timed out\") }\n        }\n    }\n}\n\nfn main() {\n    print(go().isOk())\n}\n";
    for flags in args_modes() {
        let (out, err, code) = run_with(flags, src, &[], "");
        assert_eq!(out, "true\ntimed out\ngot 7\ntrue\n", "{flags:?}: {err}");
        assert_eq!(code, 0, "{flags:?}");
    }
}

#[test]
fn exit_ends_the_program_with_its_status() {
    let src = "import std.os\nfn main() {\n    print(\"before\")\n    os.exit(7)\n}\n";
    for flags in os_modes() {
        let (out, _, code) = run_with(flags, src, &[], "");
        assert_eq!(out, "before\n", "{flags:?}");
        assert_eq!(code, 7, "{flags:?}");
    }
}

#[test]
fn standard_input_and_standard_error() {
    let src = "import std.io\nfn main() {\n    print(\"a\")\n    io.eprintln(\"warn\")\n    const first = io.readLine()\n    print(first)\n    const rest = io.readAll()\n    print(rest.len())\n    print(io.readLine())\n}\n";
    for flags in os_modes() {
        let (out, err, code) = run_with(flags, src, &[], "one\r\ntwo\nthree");
        assert_eq!(out, "a\nsome(\"one\")\n9\nnone\n", "{flags:?}");
        assert_eq!(err, "warn\n", "{flags:?}");
        assert_eq!(code, 0, "{flags:?}");
    }
}

/// The heap written in the language (`--heap=pelago`) gives the same output as the built-in one, natively, with
/// the optimiser off, and in the interpreter.
#[test]
fn the_language_heap_agrees_with_the_built_in_one() {
    let src = "fn main() {\n    var xs: [int] = []\n    for i of 0..5000 { xs.push!(i * 3) }\n    var t = 0\n    for x of xs { t += x }\n    var s = \"a\"\n    for i of 0..300 { s.pushStr!(\"bc\") }\n    print(t, s.len())\n}\n";
    let (want, _, code) = run(&[], src);
    assert_eq!(code, 0);
    assert_eq!(want.trim_end(), "37492500 601");
    for flags in [
        &["--heap=pelago"][..],
        &["--heap=pelago", "--no-opt"],
        &["--heap=pelago", "--interp"],
    ] {
        let (out, err, code) = run(flags, src);
        assert_eq!(out, want, "{flags:?} {err}");
        assert_eq!(code, 0, "{flags:?}");
    }
}

/// Files: write, append, read whole and in pieces, and every failure as a value (the same in all modes).
#[test]
fn files_are_written_and_read_back() {
    let src = r#"import std.fs
import std.io

fn describe(r: string ! io_error) -> string {
    switch r {
        case ok(text): "ok #(text.len())"
        case err(e): "error #(e.message)"
    }
}

fn run(dir: str) -> void ! io_error {
    var path = ""
    path.pushStr!(dir)
    path.pushStr!("/a.txt")
    fs.writeText(path, "one\r\ntwo\n\nlast")?
    fs.appendText(path, " line")?
    const all = fs.readText(path)?
    print(all.len())
    const bytes = fs.readBytes(path)?
    print(bytes.len(), bytes[0], bytes[bytes.len() - 1])
    var f = fs.open(path)?
    var n = 0
    loop {
        switch f.readLine!()? {
            case some(line):
                n += 1
                print(n, line.len(), line)
            case none: break
        }
    }
    fs.writeText(path, "")?
    const empty = fs.readText(path)?
    print(empty.len())
    var g = fs.create(path)?
    g.write!("x")?
    g.write!("yz")?
    var h = fs.open(path)?
    print(h.readAll!()?)
    print(h.readLine!()?)
}

fn main() {
    const dir = io.readLine() ?? ""
    print(run(dir))
    var missing = ""
    missing.pushStr!(dir)
    missing.pushStr!("/nope/b.txt")
    print(describe(fs.readText(missing)))
    switch fs.open("a\0b") {
        case ok(_): print("opened")
        case err(e): print(e.kind == io_kind.invalid)
    }
}
"#;
    for flags in os_modes() {
        let dir = std::env::temp_dir().join(format!(
            "pelago-files-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_str().unwrap().replace('\\', "/");
        let (out, err, code) = run_with(flags, src, &[], &format!("{d}\n"));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(code, 0, "{flags:?}: {err}");
        let want = format!(
            "19\n19 111 101\n1 3 one\n2 3 two\n3 0 \n4 9 last line\n0\nxyz\nnone\nok(())\nerror opening {d}/nope/b.txt: no such file or directory\ntrue\n"
        );
        assert_eq!(out, want, "{flags:?}");
    }
}

/// Standard input read through the buffer: lines that straddle the 4096-byte chunks, a last line without an
/// ending, `\r\n`, and the rest of the input in one piece.
#[test]
fn standard_input_crosses_buffer_chunks() {
    let src = "import std.io\nfn count() -> int ! io_error {\n    var n = 0\n    var total = 0\n    loop {\n        switch io.tryReadLine()? {\n            case some(line):\n                n += 1\n                total += line.len()\n            case none: break\n        }\n    }\n    print(n, total)\n    ok(n)\n}\nfn main() {\n    const first = io.readLine()\n    print(first)\n    print(count())\n    print(io.readAll().len(), io.readBytes().len())\n}\n";
    let mut input = String::from("head\r\n");
    let mut total = 0;
    for i in 0..3000 {
        let line = format!("line {i}");
        total += line.len();
        input.push_str(&line);
        input.push('\n');
    }
    input.push_str("tail");
    total += 4;
    for flags in os_modes() {
        let (out, err, code) = run_with(flags, src, &[], &input);
        assert_eq!(code, 0, "{flags:?}: {err}");
        assert_eq!(
            out,
            format!("some(\"head\")\n3001 {total}\nok(3001)\n0 0\n"),
            "{flags:?}"
        );
    }
}
