//! Build script: records the commit hash and date for `pelago --version`.
//!
//! `PELAGO_BUILD_VERSION` (set by the release workflows) overrides the crate version, so a nightly
//! can be called `0.2.0-nightly.20261126+abc1234` without editing `Cargo.toml`. Outside a git
//! checkout the commit and date are `unknown`. No dependencies: it shells out to `git`.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn main() {
    let commit = std::env::var("PELAGO_BUILD_COMMIT")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| git(&["rev-parse", "--short=7", "HEAD"]))
        .unwrap_or_else(|| "unknown".into());
    let date = std::env::var("PELAGO_BUILD_DATE")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| git(&["show", "-s", "--format=%cs", "HEAD"]))
        .unwrap_or_else(|| "unknown".into());
    let version = std::env::var("PELAGO_BUILD_VERSION")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap_or_default());
    println!("cargo:rustc-env=PELAGO_COMMIT={commit}");
    println!("cargo:rustc-env=PELAGO_COMMIT_DATE={date}");
    println!("cargo:rustc-env=PELAGO_VERSION={version}");
    println!("cargo:rerun-if-env-changed=PELAGO_BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=PELAGO_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=PELAGO_BUILD_DATE");
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs");
}
