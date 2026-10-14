//! Writes a Windows x86-64 executable (PE32+, console subsystem) that imports a few functions from
//! `kernel32.dll` and nothing else.
//!
//! Layout (file alignment 0x200, section alignment 0x1000, image base 0x1_4000_0000, no relocations:
//! the image does not ask for address randomisation):
//!
//! ```text
//! headers   DOS header, PE signature, COFF header, optional header, section table
//! .text     code, entry point at its start
//! .rdata    constants
//! .data     initialised data, then zeroed space
//! .idata    import directory, name tables, and the import address table the code calls through
//! ```

use super::elf::Blob;
use super::rt::IMPORTS;
use super::x86::{Asm, SectAddrs};

pub const IMAGE_BASE: u64 = 0x1_4000_0000;
const SECT_ALIGN: u64 = 0x1000;
const FILE_ALIGN: u64 = 0x200;
const HEADERS: u64 = 0x200;

/// Offset of the first import address table slot inside `.idata`: two import descriptors (one real,
/// one null), then the lookup table of `IMPORTS.len() + 1` entries, then the address table.
const DESCRIPTORS: u32 = 40;

/// Offset of the n-th import's address slot in `.idata`; the code calls through it.
pub const fn iat_slot(n: usize) -> u32 {
    DESCRIPTORS + 8 * (IMPORTS.len() as u32 + 1) + 8 * n as u32
}

fn align_up(v: u64, a: u64) -> u64 {
    v.div_ceil(a) * a
}

struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
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
}

/// The `.idata` section placed at relative address `rva`.
fn idata(rva: u32) -> Vec<u8> {
    let table = DESCRIPTORS;
    let lookup = table;
    let iat = iat_slot(0);
    let names_at = iat + 8 * (IMPORTS.len() as u32 + 1);
    // Names: one hint/name record per function, then the library name.
    let mut names: Vec<u8> = Vec::new();
    let mut name_rvas: Vec<u32> = Vec::new();
    for n in IMPORTS {
        if names.len() % 2 == 1 {
            names.push(0);
        }
        name_rvas.push(rva + names_at + names.len() as u32);
        names.extend_from_slice(&[0, 0]); // hint
        names.extend_from_slice(n.as_bytes());
        names.push(0);
    }
    let dll_rva = rva + names_at + names.len() as u32;
    names.extend_from_slice(b"KERNEL32.dll\0");

    let mut w = W(Vec::new());
    // Import descriptor: lookup table, time stamp, forwarder chain, library name, address table.
    w.u32(rva + lookup);
    w.u32(0);
    w.u32(0);
    w.u32(dll_rva);
    w.u32(rva + iat);
    w.0.extend_from_slice(&[0; 20]); // the null descriptor
    debug_assert_eq!(w.0.len() as u32, DESCRIPTORS);
    // Lookup table and address table start out the same: the loader overwrites the second.
    for _ in 0..2 {
        for r in &name_rvas {
            w.u64(*r as u64);
        }
        w.u64(0);
    }
    w.0.extend_from_slice(&names);
    w.0
}

/// Builds the executable.
pub fn write(asm: &Asm, rodata: &Blob, data: &Blob, bss: usize) -> Result<Vec<u8>, String> {
    let text_rva = SECT_ALIGN;
    let ro_rva = align_up(text_rva + asm.code.len() as u64, SECT_ALIGN);
    let data_rva = align_up(ro_rva + (rodata.bytes.len() as u64).max(1), SECT_ALIGN);
    let data_mem = data.bytes.len() as u64 + bss as u64;
    let id_rva = align_up(data_rva + data_mem.max(1), SECT_ALIGN);
    let id = idata(id_rva as u32);
    let image_size = align_up(id_rva + id.len() as u64, SECT_ALIGN);
    let text = asm.finish(SectAddrs {
        text: IMAGE_BASE + text_rva,
        rodata: IMAGE_BASE + ro_rva,
        data: IMAGE_BASE + data_rva,
        idata: IMAGE_BASE + id_rva,
    })?;

    // Raw file layout.
    let text_raw = HEADERS;
    let ro_raw = text_raw + align_up(text.len() as u64, FILE_ALIGN);
    let data_raw = ro_raw + align_up(rodata.bytes.len() as u64, FILE_ALIGN);
    let id_raw = data_raw + align_up(data.bytes.len() as u64, FILE_ALIGN);

    let mut w = W(Vec::new());
    // DOS header: "MZ" and the offset of the PE header at 0x3C.
    w.u16(0x5A4D);
    w.pad_to(0x3C);
    w.u32(0x40);
    w.pad_to(0x40);
    // PE signature and COFF header.
    w.0.extend_from_slice(b"PE\0\0");
    w.u16(0x8664); // x86-64
    w.u16(4); // sections
    w.u32(0); // time stamp
    w.u32(0);
    w.u32(0);
    w.u16(240); // size of the optional header
    w.u16(0x0023); // executable, no relocations, can handle addresses above 2 GB
                   // Optional header (PE32+).
    w.u16(0x20B);
    w.u8(14);
    w.u8(0);
    w.u32(align_up(text.len() as u64, FILE_ALIGN) as u32); // size of code
    w.u32(
        (align_up(rodata.bytes.len() as u64, FILE_ALIGN)
            + align_up(data.bytes.len() as u64, FILE_ALIGN)
            + id.len() as u64) as u32,
    );
    w.u32(bss as u32);
    w.u32(text_rva as u32); // entry point
    w.u32(text_rva as u32); // base of code
    w.u64(IMAGE_BASE);
    w.u32(SECT_ALIGN as u32);
    w.u32(FILE_ALIGN as u32);
    w.u16(6); // operating system version 6.0
    w.u16(0);
    w.u16(0);
    w.u16(0);
    w.u16(6); // subsystem version
    w.u16(0);
    w.u32(0);
    w.u32(image_size as u32);
    w.u32(HEADERS as u32);
    w.u32(0); // checksum
    w.u16(3); // console
    w.u16(0x8100); // terminal server aware, no-execute compatible
    w.u64(0x10_0000); // stack reserve
    w.u64(0x1000);
    w.u64(0x10_0000); // heap reserve
    w.u64(0x1000);
    w.u32(0);
    w.u32(16); // data directories
    for i in 0..16 {
        match i {
            1 => {
                w.u32((id_rva) as u32);
                w.u32(DESCRIPTORS);
            }
            12 => {
                w.u32((id_rva) as u32 + iat_slot(0));
                w.u32(8 * (IMPORTS.len() as u32 + 1));
            }
            _ => {
                w.u32(0);
                w.u32(0);
            }
        }
    }
    // Section table.
    let section =
        |w: &mut W, name: &[u8], vsize: u64, rva: u64, raw_size: u64, raw: u64, flags: u32| {
            let mut n = name.to_vec();
            n.resize(8, 0);
            w.0.extend_from_slice(&n);
            w.u32(vsize as u32);
            w.u32(rva as u32);
            w.u32(raw_size as u32);
            w.u32(if raw_size == 0 { 0 } else { raw as u32 });
            w.u32(0);
            w.u32(0);
            w.u16(0);
            w.u16(0);
            w.u32(flags);
        };
    section(
        &mut w,
        b".text",
        text.len() as u64,
        text_rva,
        align_up(text.len() as u64, FILE_ALIGN),
        text_raw,
        0x6000_0020,
    );
    section(
        &mut w,
        b".rdata",
        rodata.bytes.len() as u64,
        ro_rva,
        align_up(rodata.bytes.len() as u64, FILE_ALIGN),
        ro_raw,
        0x4000_0040,
    );
    section(
        &mut w,
        b".data",
        data_mem,
        data_rva,
        align_up(data.bytes.len() as u64, FILE_ALIGN),
        data_raw,
        0xC000_0040,
    );
    section(
        &mut w,
        b".idata",
        id.len() as u64,
        id_rva,
        align_up(id.len() as u64, FILE_ALIGN),
        id_raw,
        0xC000_0040,
    );
    w.pad_to(HEADERS as usize);
    w.0.extend_from_slice(&text);
    w.pad_to(ro_raw as usize);
    w.0.extend_from_slice(&rodata.bytes);
    w.pad_to(data_raw as usize);
    w.0.extend_from_slice(&data.bytes);
    w.pad_to(id_raw as usize);
    w.0.extend_from_slice(&id);
    w.pad_to((id_raw + align_up(id.len() as u64, FILE_ALIGN)) as usize);
    Ok(w.0)
}
