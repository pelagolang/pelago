//! Writes a static ELF64 executable for Linux x86-64 or AArch64.
//!
//! Layout (one file, two loadable segments, page aligned):
//!
//! ```text
//! 0x400000  ELF header, program headers      |
//!           .text                            | segment 1: read + execute
//!           .rodata                          |
//! next page .data, then .bss (zeroed)        | segment 2: read + write
//! (file end) section headers, symbols        (for gdb and objdump; the loader ignores them)
//! ```

use super::x86::{Asm, SectAddrs};
use super::Arch;

pub const BASE: u64 = 0x40_0000;
const PAGE: u64 = 0x1000;
/// AArch64 kernels run with 4, 16 or 64 KiB pages: segments are aligned to the largest.
const PAGE_A64: u64 = 0x1_0000;
const EHDR: u64 = 64;
const PHDR: u64 = 56;
/// Where the code starts in the file: after the header and three program headers.
const TEXT_OFF: u64 = 0x100;

/// A growing block of constant or initial data.
#[derive(Default)]
pub struct Blob {
    pub bytes: Vec<u8>,
}

impl Blob {
    /// Appends bytes (aligned to `align`, a power of two) and gives their offset.
    pub fn add(&mut self, b: &[u8], align: usize) -> u32 {
        while !self.bytes.len().is_multiple_of(align.max(1)) {
            self.bytes.push(0);
        }
        let at = self.bytes.len();
        self.bytes.extend_from_slice(b);
        at as u32
    }

    pub fn add_zero(&mut self, n: usize, align: usize) -> u32 {
        self.add(&vec![0; n], align)
    }
}

/// A named function in the code, for the symbol table.
pub struct Symbol {
    pub name: String,
    /// Offset in `.text`.
    pub offset: u64,
    pub size: u64,
}

/// Where each section ends up in memory.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub text: u64,
    pub rodata: u64,
    pub data: u64,
    pub bss: u64,
}

fn align_up(v: u64, a: u64) -> u64 {
    v.div_ceil(a) * a
}

/// The addresses of the sections for the given sizes.
pub fn layout(text_len: usize, rodata_len: usize, data_len: usize) -> Layout {
    layout_for(Arch::X86_64, text_len, rodata_len, data_len)
}

fn page(arch: Arch) -> u64 {
    match arch {
        Arch::X86_64 => PAGE,
        Arch::Aarch64 => PAGE_A64,
    }
}

pub fn layout_for(arch: Arch, text_len: usize, rodata_len: usize, data_len: usize) -> Layout {
    let text = BASE + TEXT_OFF;
    let rodata = align_up(text + text_len as u64, 16);
    let seg1_end = rodata + rodata_len as u64;
    let data = align_up(seg1_end, page(arch));
    Layout {
        text,
        rodata,
        data,
        bss: data + data_len as u64,
    }
}

struct W(Vec<u8>);

impl W {
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

/// Builds the executable: `asm` is the code (entry at offset 0), `rodata` and `data` the sections,
/// `bss` the zeroed bytes that follow `data`, `syms` the functions to name.
pub fn write(
    asm: &Asm,
    rodata: &Blob,
    data: &Blob,
    bss: usize,
    syms: &[Symbol],
) -> Result<Vec<u8>, String> {
    write_with(
        Arch::X86_64,
        asm.code.len(),
        |a| asm.finish(a),
        rodata,
        data,
        bss,
        syms,
    )
}

/// The same for AArch64 code.
pub fn write_a64(
    asm: &super::a64::Asm,
    rodata: &Blob,
    data: &Blob,
    bss: usize,
    syms: &[Symbol],
) -> Result<Vec<u8>, String> {
    write_with(
        Arch::Aarch64,
        asm.code.len(),
        |a| asm.finish(a),
        rodata,
        data,
        bss,
        syms,
    )
}

/// `finish` turns the code into its final bytes once the addresses of the sections are known.
fn write_with(
    arch: Arch,
    code_len: usize,
    finish: impl FnOnce(SectAddrs) -> Result<Vec<u8>, String>,
    rodata: &Blob,
    data: &Blob,
    bss: usize,
    syms: &[Symbol],
) -> Result<Vec<u8>, String> {
    let align = page(arch);
    let lay = layout_for(arch, code_len, rodata.bytes.len(), data.bytes.len());
    let text = finish(SectAddrs {
        text: lay.text,
        rodata: lay.rodata,
        data: lay.data,
        idata: 0,
    })?;
    let ro_off = lay.rodata - BASE;
    let data_off = lay.data - BASE;
    let seg1_size = ro_off + rodata.bytes.len() as u64;

    let mut w = W(Vec::new());
    // ELF header
    w.0.extend_from_slice(&[0x7F, b'E', b'L', b'F', 2, 1, 1, 0]);
    w.0.extend_from_slice(&[0; 8]);
    w.u16(2); // executable
    w.u16(match arch {
        Arch::X86_64 => 62,
        Arch::Aarch64 => 183,
    });
    w.u32(1);
    w.u64(lay.text); // entry
    w.u64(EHDR); // program headers
    let shoff_at = w.0.len();
    w.u64(0); // section headers: patched below
    w.u32(0);
    w.u16(EHDR as u16);
    w.u16(PHDR as u16);
    w.u16(3);
    w.u16(64); // section header size
    let shnum_at = w.0.len();
    w.u16(0);
    w.u16(0);
    // Program headers: code, data, and a note that the stack is not executable.
    let phdr =
        |w: &mut W, flags: u32, off: u64, vaddr: u64, filesz: u64, memsz: u64, align: u64| {
            w.u32(if flags == 0 { 0x6474_E551 } else { 1 });
            w.u32(flags);
            w.u64(off);
            w.u64(vaddr);
            w.u64(vaddr);
            w.u64(filesz);
            w.u64(memsz);
            w.u64(align);
        };
    phdr(&mut w, 5, 0, BASE, seg1_size, seg1_size, align);
    phdr(
        &mut w,
        6,
        data_off,
        lay.data,
        data.bytes.len() as u64,
        data.bytes.len() as u64 + bss as u64,
        align,
    );
    // PT_GNU_STACK with flags 6 (read, write; no execute)
    w.u32(0x6474_E551);
    w.u32(6);
    for _ in 0..5 {
        w.u64(0);
    }
    w.u64(16);
    w.pad_to(TEXT_OFF as usize);
    w.0.extend_from_slice(&text);
    w.pad_to(ro_off as usize);
    w.0.extend_from_slice(&rodata.bytes);
    w.pad_to(data_off as usize);
    w.0.extend_from_slice(&data.bytes);

    // Section headers, symbols and names (for tools only).
    let mut shstr: Vec<u8> = vec![0];
    let name = |s: &str, tab: &mut Vec<u8>| -> u32 {
        let at = tab.len() as u32;
        tab.extend_from_slice(s.as_bytes());
        tab.push(0);
        at
    };
    let n_text = name(".text", &mut shstr);
    let n_ro = name(".rodata", &mut shstr);
    let n_data = name(".data", &mut shstr);
    let n_bss = name(".bss", &mut shstr);
    let n_symtab = name(".symtab", &mut shstr);
    let n_strtab = name(".strtab", &mut shstr);
    let n_shstr = name(".shstrtab", &mut shstr);

    let mut strtab: Vec<u8> = vec![0];
    let mut symtab = W(Vec::new());
    symtab.0.extend_from_slice(&[0; 24]);
    for s in syms {
        let n = name(&s.name, &mut strtab);
        symtab.u32(n);
        symtab.0.push(0x12); // global function
        symtab.0.push(0);
        symtab.u16(1); // .text
        symtab.u64(lay.text + s.offset);
        symtab.u64(s.size);
    }

    w.pad_to(align_up(w.0.len() as u64, 8) as usize);
    let symtab_off = w.0.len() as u64;
    w.0.extend_from_slice(&symtab.0);
    let strtab_off = w.0.len() as u64;
    w.0.extend_from_slice(&strtab);
    let shstr_off = w.0.len() as u64;
    w.0.extend_from_slice(&shstr);
    w.pad_to(align_up(w.0.len() as u64, 8) as usize);
    let sh_off = w.0.len() as u64;

    let sh = |w: &mut W,
              name: u32,
              ty: u32,
              flags: u64,
              addr: u64,
              off: u64,
              size: u64,
              link: u32,
              info: u32,
              align: u64,
              ent: u64| {
        w.u32(name);
        w.u32(ty);
        w.u64(flags);
        w.u64(addr);
        w.u64(off);
        w.u64(size);
        w.u32(link);
        w.u32(info);
        w.u64(align);
        w.u64(ent);
    };
    w.0.extend_from_slice(&[0; 64]);
    sh(
        &mut w,
        n_text,
        1,
        6,
        lay.text,
        TEXT_OFF,
        text.len() as u64,
        0,
        0,
        16,
        0,
    );
    sh(
        &mut w,
        n_ro,
        1,
        2,
        lay.rodata,
        ro_off,
        rodata.bytes.len() as u64,
        0,
        0,
        16,
        0,
    );
    sh(
        &mut w,
        n_data,
        1,
        3,
        lay.data,
        data_off,
        data.bytes.len() as u64,
        0,
        0,
        16,
        0,
    );
    sh(
        &mut w,
        n_bss,
        8,
        3,
        lay.bss,
        data_off + data.bytes.len() as u64,
        bss as u64,
        0,
        0,
        16,
        0,
    );
    sh(
        &mut w,
        n_symtab,
        2,
        0,
        0,
        symtab_off,
        symtab.0.len() as u64,
        6,
        1,
        8,
        24,
    );
    sh(
        &mut w,
        n_strtab,
        3,
        0,
        0,
        strtab_off,
        strtab.len() as u64,
        0,
        0,
        1,
        0,
    );
    sh(
        &mut w,
        n_shstr,
        3,
        0,
        0,
        shstr_off,
        shstr.len() as u64,
        0,
        0,
        1,
        0,
    );

    w.0[shoff_at..shoff_at + 8].copy_from_slice(&sh_off.to_le_bytes());
    w.0[shnum_at..shnum_at + 2].copy_from_slice(&8u16.to_le_bytes());
    w.0[shnum_at + 2..shnum_at + 4].copy_from_slice(&7u16.to_le_bytes());
    Ok(w.0)
}
