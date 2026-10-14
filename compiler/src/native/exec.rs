//! Runs a built executable: written to a temporary file, started, removed.
//!
//! A Linux program runs on a Linux x86-64 host. A Windows program runs directly on a Windows x86-64
//! host, and on Linux through Wine when it is installed (`wine64` or `wine` on the path, or
//! `/usr/lib/wine/wine64`) and only when `PELAGO_WINE=1` is set. Tests use [`can_run`] to skip what
//! this machine cannot do.

use super::Os;
use std::path::PathBuf;

pub struct Ran {
    pub out: String,
    pub err: String,
    pub code: i32,
}

fn wine() -> Option<PathBuf> {
    // Wine makes the tests several times slower, so it is opt-in: `PELAGO_WINE=1 cargo test`.
    std::env::var_os("PELAGO_WINE")?;
    for name in ["wine64", "wine"] {
        let ok = std::process::Command::new(name)
            .arg("--version")
            .env("WINEDEBUG", "-all")
            .output()
            .is_ok_and(|o| o.status.success());
        if ok {
            return Some(PathBuf::from(name));
        }
    }
    let p = PathBuf::from("/usr/lib/wine/wine64");
    p.exists().then_some(p)
}

fn wine_prefix() -> PathBuf {
    std::env::temp_dir().join("pelago-wineprefix")
}

/// Wine sets up its prefix on first use; do that once, before tests start Wine in parallel.
fn wine_prefix_ready() {
    static READY: std::sync::Once = std::sync::Once::new();
    READY.call_once(|| {
        if let Some(w) = wine() {
            let _ = std::process::Command::new(w)
                .args(["cmd", "/c", "exit"])
                .env("WINEDEBUG", "-all")
                .env("WINEPREFIX", wine_prefix())
                .output();
        }
    });
}

/// Can this machine run programs built for `os`?
pub fn can_run(os: Os) -> bool {
    match os {
        Os::Linux => cfg!(all(target_os = "linux", target_arch = "x86_64")),
        Os::Windows => {
            cfg!(all(target_os = "windows", target_arch = "x86_64"))
                || (cfg!(all(target_os = "linux", target_arch = "x86_64")) && wine().is_some())
        }
    }
}

/// Starts the program and waits for it, collecting its output.
pub fn run_image(os: Os, image: &[u8]) -> Result<Ran, String> {
    if !can_run(os) {
        return Err(format!("cannot run {os:?} programs on this machine"));
    }
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let ext = if os == Os::Windows { ".exe" } else { "" };
    let path = std::env::temp_dir().join(format!("pelago-prog-{}-{n}{ext}", std::process::id()));
    std::fs::write(&path, image).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    let make = || {
        if os == Os::Windows && !cfg!(windows) {
            wine_prefix_ready();
            let mut c = std::process::Command::new(wine().expect("checked by can_run"));
            c.arg(&path)
                .env("WINEDEBUG", "-all")
                .env("WINEPREFIX", wine_prefix());
            c
        } else {
            std::process::Command::new(&path)
        }
    };
    // "Text file busy" happens when another thread still holds the file open for writing.
    let mut tries = 0;
    let o = loop {
        match make().output() {
            Ok(o) => break o,
            Err(e) if e.raw_os_error() == Some(26) && tries < 100 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                return Err(format!("cannot run the executable: {e}"));
            }
        }
    };
    let _ = std::fs::remove_file(&path);
    let mut err = String::from_utf8_lossy(&o.stderr).into_owned();
    if os == Os::Windows && !cfg!(windows) {
        // Wine talks about its own set-up on standard error; that is not the program's.
        err = err
            .lines()
            .filter(|l| !l.starts_with("wine: "))
            .map(|l| format!("{l}\n"))
            .collect();
    }
    Ok(Ran {
        out: String::from_utf8_lossy(&o.stdout).into_owned(),
        err,
        code: o.status.code().unwrap_or(-1),
    })
}
