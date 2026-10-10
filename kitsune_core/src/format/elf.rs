//! Loader-side parser for statically linked x86-64 ELF executables (user processes).
//!
//! Accepts only what the kernel can load without guessing: ELF64, little endian, `EM_X86_64`,
//! `ET_EXEC`, `PT_LOAD` segments (plus a few harmless notes). Dynamic linking, TLS and anything
//! unknown is refused. Every segment must lie inside the caller's user address range, never be
//! writable and executable at once, and never share a 4 KiB page with another segment, so the
//! kernel can map each one with its own page permissions. Every header read is bounds-checked
//! and the only allocation is the segment list (at most [`MAX_SEGMENTS`] entries).

use alloc::vec::Vec;

/// Most `PT_LOAD` segments accepted.
pub const MAX_SEGMENTS: usize = 16;

const PAGE: u64 = 4096;
const EHDR_SIZE: usize = 64;
const PHDR_SIZE: usize = 56;

const PT_NULL: u32 = 0;
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_NOTE: u32 = 4;
const PT_PHDR: u32 = 6;
const PT_TLS: u32 = 7;
const PT_GNU_EH_FRAME: u32 = 0x6474_e550;
const PT_GNU_STACK: u32 = 0x6474_e551;
const PT_GNU_RELRO: u32 = 0x6474_e552;

const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

/// One loadable segment: `file_size` bytes at `file_off` are copied to `vaddr`, the rest up to
/// `mem_size` is zero (`.bss`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub vaddr: u64,
    pub mem_size: u64,
    pub file_off: usize,
    pub file_size: usize,
    pub read: bool,
    pub write: bool,
    pub exec: bool,
}

/// A validated program: entry point and its segments, sorted by `vaddr`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub entry: u64,
    pub segments: Vec<Segment>,
}

/// Why an image was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElfError {
    /// The file ends inside a header.
    Truncated,
    /// Not `\x7fELF`.
    BadMagic,
    /// A valid ELF this loader does not support (the reason names what).
    Unsupported(&'static str),
    /// A segment breaks a rule (the reason names which).
    BadSegment(&'static str),
    /// The segments need more memory than the caller allows.
    TooBig,
}

fn u16_at(d: &[u8], off: usize) -> Result<u16, ElfError> {
    let b = d.get(off..off.checked_add(2).ok_or(ElfError::Truncated)?);
    b.map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or(ElfError::Truncated)
}

fn u32_at(d: &[u8], off: usize) -> Result<u32, ElfError> {
    let b = d.get(off..off.checked_add(4).ok_or(ElfError::Truncated)?);
    b.map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or(ElfError::Truncated)
}

fn u64_at(d: &[u8], off: usize) -> Result<u64, ElfError> {
    let b = d.get(off..off.checked_add(8).ok_or(ElfError::Truncated)?);
    b.map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
        .ok_or(ElfError::Truncated)
}

/// Parse a statically linked x86-64 ELF executable for a user process. Segments must lie in
/// `[user_lo, user_hi)` and need at most `max_total_mem` bytes of memory in total.
pub fn parse(
    data: &[u8],
    user_lo: u64,
    user_hi: u64,
    max_total_mem: u64,
) -> Result<Program, ElfError> {
    if data.len() < 4 {
        return Err(ElfError::Truncated);
    }
    if &data[..4] != b"\x7fELF" {
        return Err(ElfError::BadMagic);
    }
    if data.len() < EHDR_SIZE {
        return Err(ElfError::Truncated);
    }
    if data[4] != 2 {
        return Err(ElfError::Unsupported("not ELFCLASS64"));
    }
    if data[5] != 1 {
        return Err(ElfError::Unsupported("not little endian"));
    }
    if data[6] != 1 {
        return Err(ElfError::Unsupported("bad ELF version"));
    }
    match u16_at(data, 16)? {
        2 => {}
        3 => return Err(ElfError::Unsupported("ET_DYN (position independent)")),
        _ => return Err(ElfError::Unsupported("not ET_EXEC")),
    }
    if u16_at(data, 18)? != 62 {
        return Err(ElfError::Unsupported("not x86-64"));
    }
    if u32_at(data, 20)? != 1 {
        return Err(ElfError::Unsupported("bad ELF version"));
    }
    let entry = u64_at(data, 24)?;
    let phoff = u64_at(data, 32)?;
    if usize::from(u16_at(data, 54)?) != PHDR_SIZE {
        return Err(ElfError::Unsupported("bad program header size"));
    }
    let phnum = usize::from(u16_at(data, 56)?);
    if phnum == 0 {
        return Err(ElfError::BadSegment("no program headers"));
    }
    let ph_end = u64::try_from(phnum * PHDR_SIZE)
        .ok()
        .and_then(|n| phoff.checked_add(n))
        .ok_or(ElfError::Truncated)?;
    if ph_end > data.len() as u64 {
        return Err(ElfError::Truncated);
    }
    // In range of `data`, so it fits a usize.
    let phoff = phoff as usize;

    let mut segments: Vec<Segment> = Vec::new();
    for i in 0..phnum {
        let h = phoff + i * PHDR_SIZE;
        let kind = u32_at(data, h)?;
        match kind {
            PT_LOAD => {}
            PT_NULL | PT_NOTE | PT_PHDR | PT_GNU_EH_FRAME | PT_GNU_STACK | PT_GNU_RELRO => {
                continue;
            }
            PT_INTERP => return Err(ElfError::Unsupported("PT_INTERP")),
            PT_DYNAMIC => return Err(ElfError::Unsupported("PT_DYNAMIC")),
            PT_TLS => return Err(ElfError::Unsupported("PT_TLS")),
            _ => return Err(ElfError::Unsupported("unknown program header type")),
        }
        if segments.len() == MAX_SEGMENTS {
            return Err(ElfError::BadSegment("too many PT_LOAD segments"));
        }
        let flags = u32_at(data, h + 4)?;
        let off = u64_at(data, h + 8)?;
        let vaddr = u64_at(data, h + 16)?;
        let filesz = u64_at(data, h + 32)?;
        let memsz = u64_at(data, h + 40)?;

        if memsz == 0 {
            return Err(ElfError::BadSegment("empty segment"));
        }
        if filesz > memsz {
            return Err(ElfError::BadSegment("file size exceeds memory size"));
        }
        let file_end = off
            .checked_add(filesz)
            .ok_or(ElfError::BadSegment("file range overflows"))?;
        if file_end > data.len() as u64 {
            return Err(ElfError::BadSegment("file range outside the image"));
        }
        let mem_end = vaddr
            .checked_add(memsz)
            .ok_or(ElfError::BadSegment("address range overflows"))?;
        if vaddr < user_lo || mem_end > user_hi {
            return Err(ElfError::BadSegment("outside the user address range"));
        }
        if flags & PF_W != 0 && flags & PF_X != 0 {
            return Err(ElfError::BadSegment("writable and executable"));
        }
        if vaddr % PAGE != off % PAGE {
            return Err(ElfError::BadSegment("vaddr and file offset not congruent"));
        }
        segments.push(Segment {
            vaddr,
            mem_size: memsz,
            // Both are within `data`, so they fit a usize.
            file_off: off as usize,
            file_size: filesz as usize,
            read: flags & PF_R != 0,
            write: flags & PF_W != 0,
            exec: flags & PF_X != 0,
        });
    }
    if segments.is_empty() {
        return Err(ElfError::BadSegment("no PT_LOAD segment"));
    }

    segments.sort_by_key(|s| s.vaddr);
    let mut total: u64 = 0;
    let mut prev_end_page = 0u64;
    for (i, s) in segments.iter().enumerate() {
        total = total.checked_add(s.mem_size).ok_or(ElfError::TooBig)?;
        if total > max_total_mem {
            return Err(ElfError::TooBig);
        }
        // `vaddr + mem_size <= user_hi` was checked, so only the page round-up can overflow.
        let end_page = (s.vaddr + s.mem_size)
            .checked_add(PAGE - 1)
            .map(|e| e & !(PAGE - 1))
            .ok_or(ElfError::BadSegment("address range overflows"))?;
        if i > 0 && s.vaddr & !(PAGE - 1) < prev_end_page {
            return Err(ElfError::BadSegment("segments share a page"));
        }
        prev_end_page = end_page;
    }

    let entry_ok = segments
        .iter()
        .any(|s| s.exec && entry >= s.vaddr && entry - s.vaddr < s.mem_size);
    if !entry_ok {
        return Err(ElfError::BadSegment(
            "entry point not in an executable segment",
        ));
    }
    Ok(Program { entry, segments })
}

#[cfg(test)]
mod tests;
