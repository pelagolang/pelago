//! Writes a macOS AArch64 executable (Mach-O, card A-10 stage R4): static, no `dyld`, no `libSystem`.
//! The program starts at the entry of an `LC_UNIXTHREAD` command and talks to the kernel with system
//! calls, like the Linux one.
//!
//! Apple Silicon refuses to run code that is not signed, so the file ends with an **ad-hoc code
//! signature**: a `CodeDirectory` that holds the SHA-256 of every 4096-byte page of the file before it
//! (no certificate, no requirements). The kernel checks the hashes when it maps the pages.
//!
//! Layout (pages are 16 KiB on Apple Silicon):
//!
//! ```text
//! __PAGEZERO  4 GiB of inaccessible address space, no file bytes (required of 64-bit programs)
//! __TEXT      file offset 0: the headers (one page), then the code, then the constants; r-x
//! __DATA      the initialised data, then zeroed space; rw-
//! __LINKEDIT  the code signature; r--
//! ```

use super::elf::Blob;
use super::x86::SectAddrs;

pub const IMAGE_BASE: u64 = 0x1_0000_0000;
const PAGE: u64 = 0x4000;
/// Where the code starts in the file and in memory (after the headers).
const CODE_AT: u64 = PAGE;

const LOAD_COMMANDS: u32 = 6;
const SEGMENT_SIZE: u32 = 72;
const THREAD_SIZE: u32 = 288;
const SIGNATURE_CMD_SIZE: u32 = 16;
const COMMANDS_SIZE: u32 = 4 * SEGMENT_SIZE + THREAD_SIZE + SIGNATURE_CMD_SIZE;

fn align_up(v: u64, a: u64) -> u64 {
    v.div_ceil(a) * a
}

struct W(Vec<u8>);

impl W {
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn pad_to(&mut self, n: usize) {
        while self.0.len() < n {
            self.0.push(0);
        }
    }
    fn name(&mut self, s: &str) {
        let mut n = s.as_bytes().to_vec();
        n.resize(16, 0);
        self.0.extend_from_slice(&n);
    }
    #[allow(clippy::too_many_arguments)]
    fn segment(
        &mut self,
        name: &str,
        vmaddr: u64,
        vmsize: u64,
        fileoff: u64,
        filesize: u64,
        prot: u32,
    ) {
        self.u32(0x19); // LC_SEGMENT_64
        self.u32(SEGMENT_SIZE);
        self.name(name);
        self.u64(vmaddr);
        self.u64(vmsize);
        self.u64(fileoff);
        self.u64(filesize);
        self.u32(prot); // maximum protection
        self.u32(prot); // initial protection
        self.u32(0); // sections
        self.u32(0); // flags
    }
}

/// Builds the executable for the code of `asm` (the entry at offset 0), `rodata` and `data`; `bss`
/// zeroed bytes follow `data`.
pub fn write_a64(
    asm: &super::a64::Asm,
    rodata: &Blob,
    data: &Blob,
    bss: usize,
) -> Result<Vec<u8>, String> {
    let ro_at = align_up(CODE_AT + asm.code.len() as u64, 16);
    let text_end = ro_at + rodata.bytes.len() as u64;
    let text_size = align_up(text_end, PAGE);
    let data_vm = text_size;
    let data_file = align_up(data.bytes.len() as u64, PAGE);
    let data_mem = align_up(data.bytes.len() as u64 + bss as u64, PAGE).max(PAGE);
    let link_off = text_size + data_file;
    let code = asm.finish(SectAddrs {
        text: IMAGE_BASE + CODE_AT,
        rodata: IMAGE_BASE + ro_at,
        data: IMAGE_BASE + data_vm,
        idata: 0,
    })?;

    // The file up to the signature.
    let mut w = W(Vec::new());
    w.u32(0xFEED_FACF); // MH_MAGIC_64
    w.u32(0x0100_000C); // CPU_TYPE_ARM64
    w.u32(0); // CPU_SUBTYPE_ARM64_ALL
    w.u32(2); // MH_EXECUTE
    w.u32(LOAD_COMMANDS);
    w.u32(COMMANDS_SIZE);
    w.u32(1); // MH_NOUNDEFS
    w.u32(0);
    w.segment("__PAGEZERO", 0, IMAGE_BASE, 0, 0, 0);
    w.segment("__TEXT", IMAGE_BASE, text_size, 0, text_size, 5);
    w.segment(
        "__DATA",
        IMAGE_BASE + data_vm,
        data_mem,
        data_vm,
        data_file,
        3,
    );
    // The signature is built last (it hashes everything before it), but its size is known now.
    let sig_len = signature_len(link_off);
    w.segment(
        "__LINKEDIT",
        IMAGE_BASE + data_vm + data_mem,
        align_up(sig_len, PAGE),
        link_off,
        sig_len,
        1,
    );
    // LC_UNIXTHREAD: ARM_THREAD_STATE64 with only the program counter set (the kernel makes the stack).
    w.u32(0x5);
    w.u32(THREAD_SIZE);
    w.u32(6); // ARM_THREAD_STATE64
    w.u32(68); // its size in 32-bit words
    for r in 0..33 {
        // x0 to x28, fp, lr, sp, pc
        w.u64(if r == 32 { IMAGE_BASE + CODE_AT } else { 0 });
    }
    w.u32(0); // cpsr
    w.u32(0);
    w.u32(0x1D); // LC_CODE_SIGNATURE
    w.u32(SIGNATURE_CMD_SIZE);
    w.u32(link_off as u32);
    w.u32(sig_len as u32);
    debug_assert_eq!(w.0.len() as u32, 32 + COMMANDS_SIZE);
    w.pad_to(CODE_AT as usize);
    w.0.extend_from_slice(&code);
    w.pad_to(ro_at as usize);
    w.0.extend_from_slice(&rodata.bytes);
    w.pad_to(text_size as usize);
    w.0.extend_from_slice(&data.bytes);
    w.pad_to(link_off as usize);
    let sig = signature(&w.0, text_size);
    debug_assert_eq!(sig.len() as u64, sig_len);
    w.0.extend_from_slice(&sig);
    Ok(w.0)
}

// ---- The ad-hoc code signature ------------------------------------------------------------------------

const IDENT: &[u8] = b"pelago\0";
const CD_HEADER: usize = 88;
const HASH_PAGE: usize = 4096;

fn signature_len(code_limit: u64) -> u64 {
    let pages = (code_limit as usize).div_ceil(HASH_PAGE);
    (20 + CD_HEADER + IDENT.len() + 32 * pages) as u64
}

/// A signature blob (all numbers big-endian) with one `CodeDirectory` for `file`, the bytes before it;
/// `text_size` is the size of the `__TEXT` segment.
fn signature(file: &[u8], text_size: u64) -> Vec<u8> {
    let pages = file.len().div_ceil(HASH_PAGE);
    let cd_len = CD_HEADER + IDENT.len() + 32 * pages;
    let mut b: Vec<u8> = Vec::new();
    let be32 = |b: &mut Vec<u8>, v: u32| b.extend_from_slice(&v.to_be_bytes());
    let be64 = |b: &mut Vec<u8>, v: u64| b.extend_from_slice(&v.to_be_bytes());
    // SuperBlob: magic, length, count; one index entry (slot 0, the code directory) at offset 20.
    be32(&mut b, 0xFADE_0CC0);
    be32(&mut b, (20 + cd_len) as u32);
    be32(&mut b, 1);
    be32(&mut b, 0);
    be32(&mut b, 20);
    // CodeDirectory.
    be32(&mut b, 0xFADE_0C02);
    be32(&mut b, cd_len as u32);
    be32(&mut b, 0x0002_0400); // version
    be32(&mut b, 2); // CS_ADHOC
    be32(&mut b, (CD_HEADER + IDENT.len()) as u32); // hash offset
    be32(&mut b, CD_HEADER as u32); // identifier offset
    be32(&mut b, 0); // special slots
    be32(&mut b, pages as u32);
    be32(&mut b, file.len() as u32); // code limit
    b.push(32); // hash size
    b.push(2); // SHA-256
    b.push(0); // platform
    b.push(12); // log2 of the page size
    be32(&mut b, 0); // spare
    be32(&mut b, 0); // scatter offset
    be32(&mut b, 0); // team offset
    be32(&mut b, 0); // spare
    be64(&mut b, 0); // code limit, 64 bits
    be64(&mut b, 0); // executable segment: file offset
    be64(&mut b, text_size); // its size
    be64(&mut b, 1); // CS_EXECSEG_MAIN_BINARY
    debug_assert_eq!(b.len(), 20 + CD_HEADER);
    b.extend_from_slice(IDENT);
    for page in file.chunks(HASH_PAGE) {
        b.extend_from_slice(&sha256(page));
    }
    b
}

/// SHA-256 (FIPS 180-4).
fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&(data.len() as u64 * 8).to_be_bytes());
    for block in msg.chunks(64) {
        let mut w = [0u32; 64];
        for (i, c) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 32];
    for (i, x) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn sha256_matches_the_standard_vectors() {
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }
}
