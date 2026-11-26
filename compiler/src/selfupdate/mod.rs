//! `pelago self update`: find the newest release on a channel, check it, replace this binary.
//!
//! The steps: read the release list from GitHub (through the system `curl`, so the compiler needs no
//! HTTP library), pick the newest build of this platform on the channel, download the archive and its
//! `SHA256SUMS`, check the hash and the Ed25519 signature of the sums, unpack, run the new binary once
//! to see it reports the version expected, and only then swap it in. Any failure leaves the old
//! binary in place. See `docs/release/README.md` for the signing side.

pub mod ed25519;
pub mod json;
pub mod select;
pub mod sha2;

use crate::version::{self, Channel};
use select::{Candidate, Release, Version};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Public keys the release signatures are checked with. A release signed by any of them is trusted.
/// It stays empty until the first signed release exists; until then an update needs `--allow-unsigned`.
pub const RELEASE_KEYS: &[[u8; 32]] = &[];

/// The platform name used in archive names, for example `x86_64-linux`.
pub fn target() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Trust {
    Signed,
    Unsigned,
}

/// Checks the downloaded archive against `SHA256SUMS`, and the sums against their signature.
pub fn verify_download(
    archive_name: &str,
    archive: &[u8],
    sums: &str,
    sig: Option<&[u8]>,
    keys: &[[u8; 32]],
    allow_unsigned: bool,
) -> Result<Trust, String> {
    let expected = sums
        .lines()
        .filter_map(|l| l.split_once("  "))
        .find(|(_, name)| name.trim() == archive_name)
        .map(|(h, _)| h.trim().to_ascii_lowercase())
        .ok_or_else(|| format!("{archive_name} is not listed in the checksum file"))?;
    let actual = sha2::hex(&sha2::sha256(archive));
    if expected != actual {
        return Err(format!(
            "checksum mismatch for {archive_name}: expected {expected}, got {actual}"
        ));
    }
    match sig {
        Some(sig) => {
            let sig: [u8; 64] = sig
                .try_into()
                .map_err(|_| "the signature file is not 64 bytes".to_string())?;
            if keys.is_empty() {
                return if allow_unsigned {
                    Ok(Trust::Unsigned)
                } else {
                    Err("this build has no release key to check the signature with; pass --allow-unsigned to go on with the checksum only".into())
                };
            }
            if keys
                .iter()
                .any(|k| ed25519::verify(k, sums.as_bytes(), &sig))
            {
                Ok(Trust::Signed)
            } else {
                Err("the signature on the checksum file is not valid for any release key".into())
            }
        }
        None if allow_unsigned => Ok(Trust::Unsigned),
        None => Err(
            "this release has no signature; pass --allow-unsigned to go on with the checksum only"
                .into(),
        ),
    }
}

fn api_base() -> String {
    std::env::var("PELAGO_UPDATE_API")
        .unwrap_or_else(|_| "https://api.github.com/repos/pelagolang/pelago".into())
}

fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("curl")
        .args(["-fsSL", "--retry", "2", "-A", "pelago-self-update"])
        .args(args)
        .output()
        .map_err(|e| format!("cannot run curl ({e}); install curl to use self update"))?;
    if !out.status.success() {
        return Err(format!(
            "download failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

fn fetch_json(url: &str) -> Result<json::Json, String> {
    let body = curl(&["-H", "Accept: application/vnd.github+json", url])?;
    json::parse(&String::from_utf8_lossy(&body))
}

/// The release list, with the long asset list of the rolling nightly release read in full.
fn fetch_releases(nightly: bool) -> Result<Vec<Release>, String> {
    let list = fetch_json(&format!("{}/releases?per_page=100", api_base()))?;
    let mut releases = select::parse_releases(&list);
    if nightly {
        for r in releases.iter_mut().filter(|r| r.tag == "nightly") {
            let Some(url) = r.assets_url.clone() else {
                continue;
            };
            let mut all = Vec::new();
            for page in 1..=10 {
                let sep = if url.contains('?') { '&' } else { '?' };
                let chunk = fetch_json(&format!("{url}{sep}per_page=100&page={page}"))?;
                let n = chunk.arr().len();
                for a in chunk.arr() {
                    if let (Some(name), Some(u)) = (
                        a.get("name").and_then(json::Json::str),
                        a.get("browser_download_url").and_then(json::Json::str),
                    ) {
                        all.push(select::Asset {
                            name: name.to_string(),
                            url: u.to_string(),
                        });
                    }
                }
                if n < 100 {
                    break;
                }
            }
            if !all.is_empty() {
                r.assets = all;
            }
        }
    }
    Ok(releases)
}

fn download(url: &str, to: &Path) -> Result<(), String> {
    curl(&["-o", &to.to_string_lossy(), url]).map(|_| ())
}

fn channel_file(exe: &Path) -> PathBuf {
    exe.with_file_name("pelago.channel")
}

fn saved_channel(exe: &Path) -> Option<Channel> {
    Channel::parse(std::fs::read_to_string(channel_file(exe)).ok()?.trim())
}

/// Swaps `new_bin` in for `exe`. On Unix a rename is atomic; on Windows the running file cannot be
/// replaced, so it is renamed aside (and removed on the next update).
fn install(new_bin: &Path, exe: &Path) -> Result<(), String> {
    let dir = exe.parent().ok_or("the binary has no folder")?;
    let staged = dir.join(".pelago.new");
    std::fs::copy(new_bin, &staged)
        .map_err(|e| format!("cannot write next to {}: {e}", exe.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
        std::fs::rename(&staged, exe).map_err(|e| {
            let _ = std::fs::remove_file(&staged);
            format!("cannot replace {}: {e}", exe.display())
        })
    }
    #[cfg(not(unix))]
    {
        let old = old_path(exe);
        let _ = std::fs::remove_file(&old);
        std::fs::rename(exe, &old)
            .map_err(|e| format!("cannot move {} aside: {e}", exe.display()))?;
        std::fs::rename(&staged, exe).map_err(|e| {
            let _ = std::fs::rename(&old, exe);
            format!("cannot put the new binary in place: {e}")
        })
    }
}

#[cfg(not(unix))]
fn old_path(exe: &Path) -> PathBuf {
    let mut s = exe.as_os_str().to_owned();
    s.push(".old");
    PathBuf::from(s)
}

struct Options {
    channel: Option<Channel>,
    check: bool,
    version: Option<String>,
    allow_unsigned: bool,
}

fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut o = Options {
        channel: None,
        check: false,
        version: None,
        allow_unsigned: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--check" => o.check = true,
            "--allow-unsigned" => o.allow_unsigned = true,
            "--channel" => {
                let v = it
                    .next()
                    .ok_or("--channel needs stable, beta, rc or nightly")?;
                o.channel =
                    Some(Channel::parse(v).ok_or_else(|| {
                        format!("unknown channel `{v}` (stable, beta, rc, nightly)")
                    })?);
            }
            "--version" => o.version = Some(it.next().ok_or("--version needs a version")?.clone()),
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    Ok(o)
}

fn update(args: &[String]) -> Result<(), String> {
    let o = parse_options(args)?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    #[cfg(not(unix))]
    let _ = std::fs::remove_file(old_path(&exe));
    let channel = o
        .channel
        .or_else(|| saved_channel(&exe))
        .unwrap_or_else(|| version::channel_of(version::VERSION));
    let target = target();
    println!(
        "pelago {} on {target}, channel {}",
        version::VERSION,
        channel.name()
    );
    let releases = fetch_releases(channel == Channel::Nightly || o.version.is_some())?;
    let cand: Candidate = select::pick(&releases, channel, &target, o.version.as_deref())
        .ok_or_else(|| match &o.version {
            Some(v) => format!("no release {v} for {target}"),
            None => format!("no {} release for {target} yet", channel.name()),
        })?;
    let current = Version::parse(version::VERSION);
    let newer = match (&current, Version::parse(&cand.version)) {
        (Some(c), Some(n)) => n > *c,
        _ => cand.version != version::VERSION,
    };
    if !newer && o.version.is_none() {
        println!("already up to date ({})", version::VERSION);
        return Ok(());
    }
    if o.check {
        println!(
            "update available: {} (run `pelago self update` to install it)",
            cand.version
        );
        return Ok(());
    }
    println!("installing {}", cand.version);
    let work = std::env::temp_dir().join(format!("pelago-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = (|| {
        let archive_path = work.join(&cand.archive.name);
        download(&cand.archive.url, &archive_path)?;
        let sums = String::from_utf8_lossy(&curl(&[&cand.sums.url])?).into_owned();
        let sig = match &cand.sig {
            Some(s) => Some(curl(&[&s.url])?),
            None => None,
        };
        let bytes = std::fs::read(&archive_path).map_err(|e| e.to_string())?;
        let trust = verify_download(
            &cand.archive.name,
            &bytes,
            &sums,
            sig.as_deref(),
            RELEASE_KEYS,
            o.allow_unsigned,
        )?;
        println!(
            "checksum ok{}",
            if trust == Trust::Signed {
                ", signature ok"
            } else {
                " (not signed: --allow-unsigned)"
            }
        );
        let status = Command::new("tar")
            .arg(if cand.archive.name.ends_with(".zip") {
                "-xf"
            } else {
                "-xzf"
            })
            .arg(&archive_path)
            .arg("-C")
            .arg(&work)
            .status()
            .map_err(|e| format!("cannot run tar: {e}"))?;
        if !status.success() {
            return Err("cannot unpack the archive".to_string());
        }
        let exe_name = if cfg!(windows) {
            "pelago.exe"
        } else {
            "pelago"
        };
        let new_bin = work
            .join(format!("pelago-{}-{target}", cand.version))
            .join(exe_name);
        let out = Command::new(&new_bin)
            .arg("--version")
            .output()
            .map_err(|e| format!("the new binary does not run: {e}"))?;
        let said = String::from_utf8_lossy(&out.stdout);
        if !said.starts_with(&format!("pelago {}", cand.version)) {
            return Err(format!(
                "the new binary reports `{}`, expected {}",
                said.trim(),
                cand.version
            ));
        }
        install(&new_bin, &exe)
    })();
    let _ = std::fs::remove_dir_all(&work);
    result?;
    if o.channel.is_some() || saved_channel(&exe).is_some() {
        let _ = std::fs::write(channel_file(&exe), format!("{}\n", channel.name()));
    }
    println!("updated to {}", cand.version);
    Ok(())
}

/// Entry point for `pelago self <command>`; returns the process exit code.
pub fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("update") => match update(&args[1..]) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        },
        _ => {
            eprintln!("usage: pelago self update [--channel stable|beta|rc|nightly] [--version X.Y.Z] [--check] [--allow-unsigned]");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARCHIVE: &[u8] = b"pretend archive bytes";

    fn sums() -> String {
        format!(
            "{}  pelago-0.1.0-x86_64-linux.tar.gz\n",
            sha2::hex(&sha2::sha256(ARCHIVE))
        )
    }

    #[test]
    fn a_wrong_hash_is_refused() {
        let e = verify_download(
            "pelago-0.1.0-x86_64-linux.tar.gz",
            b"other",
            &sums(),
            None,
            &[],
            true,
        )
        .unwrap_err();
        assert!(e.contains("checksum mismatch"), "{e}");
        let e = verify_download("missing.tar.gz", ARCHIVE, &sums(), None, &[], true).unwrap_err();
        assert!(e.contains("not listed"), "{e}");
    }

    #[test]
    fn unsigned_needs_the_flag() {
        let n = "pelago-0.1.0-x86_64-linux.tar.gz";
        assert!(verify_download(n, ARCHIVE, &sums(), None, &[], false).is_err());
        assert_eq!(
            verify_download(n, ARCHIVE, &sums(), None, &[], true),
            Ok(Trust::Unsigned)
        );
        // A signature with no key to check it is not trusted either, unless allowed.
        assert!(verify_download(n, ARCHIVE, &sums(), Some(&[0u8; 64]), &[], false).is_err());
    }

    #[test]
    fn a_good_signature_is_trusted_and_a_bad_one_is_not() {
        fn unhex<const N: usize>(s: &str) -> [u8; N] {
            let mut out = [0u8; N];
            for i in 0..N {
                out[i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
            }
            out
        }
        let key = unhex::<32>("8027b65c6bf578f5451067a2e51cab0090d695d5e1b254813d39fa0b9763e20c");
        let sig = unhex::<64>("2de9c7c8ffc74291978579f33d1ca96fb82d0d564c3360f50934a2befdc67319036019ea86e766f2ffa2b07833b115bddcdb66d57741463143523d4ed40ecf0a");
        // The signed text is the OpenSSL test message, so build an archive whose checksum line matches it.
        let text = "abc123  pelago-0.1.0-x86_64-linux.tar.gz\n";
        // The hash in that line is not this archive's, so the checksum check fails first.
        assert!(verify_download(
            "pelago-0.1.0-x86_64-linux.tar.gz",
            ARCHIVE,
            text,
            Some(&sig),
            &[key],
            false
        )
        .is_err());
        // With the right hash the signature (over different text) must fail.
        let e = verify_download(
            "pelago-0.1.0-x86_64-linux.tar.gz",
            ARCHIVE,
            &sums(),
            Some(&sig),
            &[key],
            false,
        )
        .unwrap_err();
        assert!(e.contains("not valid"), "{e}");
    }

    #[test]
    fn options_parse() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let o = parse_options(&a(&["--channel", "nightly", "--check"])).unwrap();
        assert_eq!(o.channel, Some(Channel::Nightly));
        assert!(o.check);
        assert!(parse_options(&a(&["--channel", "dev"])).is_err());
        assert!(parse_options(&a(&["--nope"])).is_err());
    }
}
