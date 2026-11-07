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

/// The modes that run the operating system layer: the interpreter everywhere, native code where the
/// layer is built (x86-64 Linux).
fn os_modes() -> Vec<&'static [&'static str]> {
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
    for flags in os_modes() {
        let (out, err, code) = run_with(flags, src, &["x", "y z"], "");
        assert_eq!(out, "3\nx\ny z\n", "{flags:?}: {err}");
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
