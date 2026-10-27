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
