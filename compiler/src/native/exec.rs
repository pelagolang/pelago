//! Runs a built executable (Linux on x86-64 only): written to a temporary file, started, removed.

use std::os::unix::fs::PermissionsExt;

pub struct Ran {
    pub out: String,
    pub err: String,
    pub code: i32,
}

/// Starts the program and waits for it, collecting its output.
pub fn run_image(image: &[u8]) -> Result<Ran, String> {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("pelago-elf-{}-{n}", std::process::id()));
    std::fs::write(&path, image).map_err(|e| e.to_string())?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| e.to_string())?;
    // "Text file busy" happens when another thread still holds the file open for writing.
    let mut tries = 0;
    let o = loop {
        match std::process::Command::new(&path).output() {
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
    Ok(Ran {
        out: String::from_utf8_lossy(&o.stdout).into_owned(),
        err: String::from_utf8_lossy(&o.stderr).into_owned(),
        code: o.status.code().unwrap_or(-1),
    })
}
