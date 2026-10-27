//! Runs a built Linux image inside the current process (`pelago run`, docs/design/run-mode.md).
//!
//! The image is a static executable with no libc: it sits at a fixed address, its entry builds a
//! stack of its own and the program ends in `exit_group`. So instead of writing a file and starting a
//! child, [`run`] maps the segments at their addresses and jumps to the entry; the program then ends
//! the process with its own exit status. Any failure before the jump comes back as `Err`, and the
//! caller falls back to a temporary file.

use std::ffi::c_void;

extern "C" {
    fn mmap(addr: *mut c_void, len: usize, prot: i32, flags: i32, fd: i32, off: i64)
        -> *mut c_void;
    fn mprotect(addr: *mut c_void, len: usize, prot: i32) -> i32;
    fn munmap(addr: *mut c_void, len: usize) -> i32;
}

const PROT_READ: i32 = 1;
const PROT_WRITE: i32 = 2;
const PROT_EXEC: i32 = 4;
const MAP_PRIVATE: i32 = 0x2;
const MAP_ANONYMOUS: i32 = 0x20;
const MAP_FIXED_NOREPLACE: i32 = 0x10_0000;

/// One loadable segment of an image.
#[derive(Debug, PartialEq, Eq)]
pub struct Segment {
    pub vaddr: u64,
    pub offset: u64,
    pub filesz: u64,
    pub memsz: u64,
    /// ELF flags: 1 execute, 2 write, 4 read.
    pub flags: u32,
}

fn u16_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?) as u64)
}
fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// The entry address and the loadable segments of an ELF64 image, checked to lie inside the file.
pub fn segments(image: &[u8]) -> Result<(u64, Vec<Segment>), String> {
    let bad = || "malformed image".to_string();
    if image.get(..4) != Some(&[0x7F, b'E', b'L', b'F']) {
        return Err(bad());
    }
    let entry = u64_at(image, 24).ok_or_else(bad)?;
    let phoff = u64_at(image, 32).ok_or_else(bad)? as usize;
    let phentsize = u16_at(image, 54).ok_or_else(bad)? as usize;
    let phnum = u16_at(image, 56).ok_or_else(bad)? as usize;
    let mut out = Vec::new();
    for i in 0..phnum {
        let at = phoff + i * phentsize;
        if u32_at(image, at).ok_or_else(bad)? != 1 {
            continue;
        }
        let s = Segment {
            flags: u32_at(image, at + 4).ok_or_else(bad)?,
            offset: u64_at(image, at + 8).ok_or_else(bad)?,
            vaddr: u64_at(image, at + 16).ok_or_else(bad)?,
            filesz: u64_at(image, at + 32).ok_or_else(bad)?,
            memsz: u64_at(image, at + 40).ok_or_else(bad)?,
        };
        if s.offset
            .checked_add(s.filesz)
            .is_none_or(|e| e > image.len() as u64)
            || s.filesz > s.memsz
        {
            return Err(bad());
        }
        out.push(s);
    }
    if out.is_empty() {
        return Err(bad());
    }
    Ok((entry, out))
}

/// Maps `image` at its own addresses and jumps to its entry. Only returns on failure.
pub fn run(image: &[u8]) -> Result<std::convert::Infallible, String> {
    let (entry, segs) = segments(image)?;
    // The images align segments to the largest page any supported kernel uses.
    let page: u64 = if cfg!(target_arch = "aarch64") {
        0x1_0000
    } else {
        0x1000
    };
    let down = |v: u64| v & !(page - 1);
    let up = |v: u64| v.div_ceil(page) * page;
    let lo = segs.iter().map(|s| down(s.vaddr)).min().unwrap_or(0);
    let hi = segs
        .iter()
        .map(|s| up(s.vaddr + s.memsz))
        .max()
        .unwrap_or(0);
    let len = (hi - lo) as usize;
    // SAFETY: a fresh anonymous mapping at an address nothing else uses (the kernel refuses to
    // replace an existing mapping with MAP_FIXED_NOREPLACE; older kernels ignore the flag, which the
    // address check below catches). The segments are copied inside it, and only then is it made
    // executable. The jump goes to the image's own entry, which never returns.
    unsafe {
        let got = mmap(
            lo as *mut c_void,
            len,
            PROT_READ | PROT_WRITE,
            MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE,
            -1,
            0,
        );
        if got as usize as u64 != lo {
            if got as isize != -1 {
                munmap(got, len);
            }
            return Err("the address of the program is taken".into());
        }
        for s in &segs {
            let src = &image[s.offset as usize..(s.offset + s.filesz) as usize];
            std::ptr::copy_nonoverlapping(src.as_ptr(), s.vaddr as *mut u8, src.len());
        }
        for s in &segs {
            let prot = (if s.flags & 4 != 0 { PROT_READ } else { 0 })
                | (if s.flags & 2 != 0 { PROT_WRITE } else { 0 })
                | (if s.flags & 1 != 0 { PROT_EXEC } else { 0 });
            let start = down(s.vaddr);
            if mprotect(
                start as *mut c_void,
                (up(s.vaddr + s.memsz) - start) as usize,
                prot,
            ) != 0
            {
                munmap(lo as *mut c_void, len);
                return Err("cannot protect the program's memory".into());
            }
        }
        let main: extern "C" fn() -> ! = std::mem::transmute(entry as usize);
        main()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_garbage() {
        assert!(segments(b"nope").is_err());
        assert!(segments(&[0x7F, b'E', b'L', b'F', 2, 1, 1, 0]).is_err());
    }

    #[test]
    fn reads_the_segments_of_a_built_image() {
        use crate::native::elf::{write, Blob, Symbol};
        use crate::native::x86::Asm;
        let mut asm = Asm::new();
        asm.ret();
        let mut ro = Blob::default();
        ro.add(b"abc", 1);
        let mut data = Blob::default();
        data.add(b"xy", 1);
        let img = write(&asm, &ro, &data, 16, &[] as &[Symbol]).unwrap();
        let (entry, segs) = segments(&img).unwrap();
        assert_eq!(entry, 0x40_0100);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].vaddr, 0x40_0000);
        assert_eq!(segs[0].flags, 5);
        assert_eq!(segs[1].flags, 6);
        assert_eq!(segs[1].memsz, segs[1].filesz + 16);
    }
}
