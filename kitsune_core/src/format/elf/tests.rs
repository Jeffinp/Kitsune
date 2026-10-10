use super::*;
use alloc::vec;

const LO: u64 = 0x40_0000;
const HI: u64 = 0x7fff_0000_0000;
const MAX: u64 = 1 << 30;

/// (type, flags, offset, vaddr, filesz, memsz)
type Ph = (u32, u32, u64, u64, u64, u64);

/// Build an image: header, `phs` right after it, then `body` at offset 4096.
fn build(entry: u64, phs: &[Ph], body: &[u8]) -> Vec<u8> {
    let mut v = vec![0u8; EHDR_SIZE];
    v[..4].copy_from_slice(b"\x7fELF");
    v[4] = 2;
    v[5] = 1;
    v[6] = 1;
    v[16..18].copy_from_slice(&2u16.to_le_bytes());
    v[18..20].copy_from_slice(&62u16.to_le_bytes());
    v[20..24].copy_from_slice(&1u32.to_le_bytes());
    v[24..32].copy_from_slice(&entry.to_le_bytes());
    v[32..40].copy_from_slice(&64u64.to_le_bytes());
    v[52..54].copy_from_slice(&64u16.to_le_bytes());
    v[54..56].copy_from_slice(&56u16.to_le_bytes());
    v[56..58].copy_from_slice(&(phs.len() as u16).to_le_bytes());
    for &(t, f, off, va, fs, ms) in phs {
        v.extend_from_slice(&t.to_le_bytes());
        v.extend_from_slice(&f.to_le_bytes());
        v.extend_from_slice(&off.to_le_bytes());
        v.extend_from_slice(&va.to_le_bytes());
        v.extend_from_slice(&va.to_le_bytes());
        v.extend_from_slice(&fs.to_le_bytes());
        v.extend_from_slice(&ms.to_le_bytes());
        v.extend_from_slice(&4096u64.to_le_bytes());
    }
    v.resize(4096, 0);
    v.extend_from_slice(body);
    v
}

const RX: u32 = PF_R | PF_X;
const RW: u32 = PF_R | PF_W;

/// Text at 0x400000 (file 0x1000, 0x100 bytes), data at 0x401000 (file 0x1100.., bss).
fn good_phs() -> Vec<Ph> {
    vec![
        (PT_LOAD, RX, 0x1000, LO, 0x100, 0x100),
        (PT_LOAD, RW, 0x1000, LO + 0x1000, 0x80, 0x2000),
    ]
}

fn good() -> Vec<u8> {
    build(LO + 0x10, &good_phs(), &[0x90; 0x200])
}

fn p(d: &[u8]) -> Result<Program, ElfError> {
    parse(d, LO, HI, MAX)
}

#[test]
fn parses_a_good_program() {
    let prog = p(&good()).unwrap();
    assert_eq!(prog.entry, LO + 0x10);
    assert_eq!(prog.segments.len(), 2);
    let t = &prog.segments[0];
    assert_eq!(
        (t.vaddr, t.mem_size, t.file_off, t.file_size),
        (LO, 0x100, 0x1000, 0x100)
    );
    assert!(t.read && t.exec && !t.write);
    let d = &prog.segments[1];
    assert_eq!(
        (d.vaddr, d.mem_size, d.file_size),
        (LO + 0x1000, 0x2000, 0x80)
    );
    assert!(d.read && d.write && !d.exec);
}

#[test]
fn segments_come_back_sorted() {
    let mut phs = good_phs();
    phs.reverse();
    let prog = p(&build(LO, &phs, &[0; 0x200])).unwrap();
    assert!(prog.segments[0].vaddr < prog.segments[1].vaddr);
}

#[test]
fn ignores_harmless_headers() {
    let mut phs = good_phs();
    phs.push((PT_NOTE, PF_R, 0, 0, 0, 0));
    phs.push((PT_GNU_STACK, RW | PF_X, 0, 0, 0, 0));
    phs.push((PT_GNU_RELRO, PF_R, 0, 0, 0, 0));
    phs.push((PT_NULL, 0, 0, 0, 0, 0));
    assert!(p(&build(LO, &phs, &[0; 0x200])).is_ok());
}

#[test]
fn rejects_header_problems() {
    assert_eq!(p(&[]), Err(ElfError::Truncated));
    assert_eq!(p(b"\x7fEL"), Err(ElfError::Truncated));
    assert_eq!(p(b"MZ\x90\x00more"), Err(ElfError::BadMagic));
    assert_eq!(p(b"\x7fELF"), Err(ElfError::Truncated));
    let g = good();
    let mut d = g.clone();
    d[4] = 1;
    assert!(matches!(p(&d), Err(ElfError::Unsupported(_))));
    let mut d = g.clone();
    d[5] = 2;
    assert!(matches!(p(&d), Err(ElfError::Unsupported(_))));
    let mut d = g.clone();
    d[6] = 2;
    assert!(matches!(p(&d), Err(ElfError::Unsupported(_))));
    let mut d = g.clone();
    d[16] = 3; // ET_DYN
    assert!(matches!(p(&d), Err(ElfError::Unsupported(_))));
    let mut d = g.clone();
    d[16] = 1; // ET_REL
    assert!(matches!(p(&d), Err(ElfError::Unsupported(_))));
    let mut d = g.clone();
    d[18] = 3; // i386
    assert!(matches!(p(&d), Err(ElfError::Unsupported(_))));
    let mut d = g.clone();
    d[20] = 2;
    assert!(matches!(p(&d), Err(ElfError::Unsupported(_))));
    let mut d = g.clone();
    d[54] = 32;
    assert!(matches!(p(&d), Err(ElfError::Unsupported(_))));
}

#[test]
fn rejects_program_header_table_problems() {
    let mut d = good();
    d[56] = 0;
    d[57] = 0;
    assert!(matches!(p(&d), Err(ElfError::BadSegment(_))));
    let mut d = good();
    d[56..58].copy_from_slice(&0xffffu16.to_le_bytes());
    d.truncate(4096);
    assert_eq!(p(&d), Err(ElfError::Truncated));
    let mut d = good();
    d[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
    assert_eq!(p(&d), Err(ElfError::Truncated));
    let mut d = good();
    d[32..40].copy_from_slice(&(u64::MAX - 10).to_le_bytes());
    assert_eq!(p(&d), Err(ElfError::Truncated));
}

#[test]
fn rejects_unsupported_program_headers() {
    for t in [PT_INTERP, PT_DYNAMIC, PT_TLS, 5, 0x1234_5678] {
        let mut phs = good_phs();
        phs.push((t, PF_R, 0, 0, 0, 0));
        let r = p(&build(LO, &phs, &[0; 0x200]));
        assert!(matches!(r, Err(ElfError::Unsupported(_))), "type {t}");
    }
}

#[test]
fn rejects_no_load_segments() {
    let d = build(LO, &[(PT_NOTE, PF_R, 0, 0, 0, 0)], &[]);
    assert!(matches!(p(&d), Err(ElfError::BadSegment(_))));
}

#[test]
fn rejects_too_many_loads() {
    let phs: Vec<Ph> = (0..17u64)
        .map(|i| (PT_LOAD, RX, 0x1000, LO + i * 0x1000, 1, 1))
        .collect();
    let d = build(LO, &phs, &[0; 16]);
    assert!(matches!(p(&d), Err(ElfError::BadSegment(_))));
    // Exactly 16 is fine.
    let d = build(LO, &phs[..16], &[0; 16]);
    assert_eq!(p(&d).unwrap().segments.len(), 16);
}

fn one(ph: Ph, entry: u64) -> Result<Program, ElfError> {
    p(&build(entry, &[ph], &[0x90; 0x300]))
}

#[test]
fn rejects_bad_segments() {
    let bad = |ph: Ph| {
        assert!(
            matches!(one(ph, LO), Err(ElfError::BadSegment(_))),
            "{ph:?}"
        )
    };
    bad((PT_LOAD, RX, 0x1000, LO, 0, 0)); // empty
    bad((PT_LOAD, RX, 0x1000, LO, 0x101, 0x100)); // filesz > memsz
    bad((PT_LOAD, RX, 0x1000 + 0x1_0000, LO, 0x10, 0x10)); // file range outside
    bad((PT_LOAD, RX, 0x1000, LO, u64::MAX, u64::MAX)); // huge sizes
    bad((PT_LOAD, RX, u64::MAX, LO, 2, 2)); // offset + size overflows
    bad((PT_LOAD, RX, 0x1000, u64::MAX - 5, 0, 0x100)); // vaddr + memsz overflows
    bad((PT_LOAD, RX, 0x1000, LO - 0x1000, 0x10, 0x10)); // below user_lo
    bad((PT_LOAD, RX, 0x1000, HI - 0x10, 0x10, 0x1000)); // crosses user_hi
    bad((PT_LOAD, RX, 0x1000, HI, 0x10, 0x10)); // starts at user_hi
    bad((PT_LOAD, PF_R | PF_W | PF_X, 0x1000, LO, 0x10, 0x10)); // W+X
    bad((PT_LOAD, RX, 0x1000, LO + 8, 0x10, 0x10)); // vaddr/offset not congruent
    bad((PT_LOAD, RX, 0x1008, LO, 0x10, 0x10)); // same, other side
}

#[test]
fn segment_ending_exactly_at_user_hi_is_ok() {
    let r = one(
        (PT_LOAD, RX, 0x1000, HI - 0x1000, 0x10, 0x1000),
        HI - 0x1000,
    );
    assert!(r.is_ok());
    // Page round-up of the end may exceed user_hi when it is unaligned: still fine.
    let r = parse(
        &build(LO, &[(PT_LOAD, RX, 0x1000, LO, 0x10, 0x10)], &[0; 16]),
        LO,
        LO + 0x10,
        MAX,
    );
    assert!(r.is_ok());
}

#[test]
fn rejects_page_sharing_and_overlap() {
    // Second segment starts in the last page of the first.
    let phs = [
        (PT_LOAD, RX, 0x1000, LO, 0x100, 0x100),
        (PT_LOAD, RW, 0x1100, LO + 0x100, 0x10, 0x10),
    ];
    let d = build(LO, &phs, &[0; 0x300]);
    assert_eq!(p(&d), Err(ElfError::BadSegment("segments share a page")));
    // True overlap.
    let phs = [
        (PT_LOAD, RX, 0x1000, LO, 0x10, 0x3000),
        (PT_LOAD, RW, 0x1000, LO + 0x1000, 0x10, 0x10),
    ];
    let d = build(LO, &phs, &[0; 0x300]);
    assert_eq!(p(&d), Err(ElfError::BadSegment("segments share a page")));
    // Identical ranges.
    let phs = [
        (PT_LOAD, RX, 0x1000, LO, 0x10, 0x10),
        (PT_LOAD, RW, 0x1000, LO, 0x10, 0x10),
    ];
    assert!(p(&build(LO, &phs, &[0; 0x300])).is_err());
    // Unsorted input is still caught (later header first in the table).
    let phs = [
        (PT_LOAD, RW, 0x1100, LO + 0x100, 0x10, 0x10),
        (PT_LOAD, RX, 0x1000, LO, 0x100, 0x100),
    ];
    assert!(p(&build(LO, &phs, &[0; 0x300])).is_err());
    // Adjacent pages are fine.
    let phs = [
        (PT_LOAD, RX, 0x1000, LO, 0x1000, 0x1000),
        (PT_LOAD, RW, 0x1000, LO + 0x1000, 0x10, 0x10),
    ];
    assert!(p(&build(LO, &phs, &[0; 0x1100])).is_ok());
}

#[test]
fn total_memory_limit() {
    let d = good(); // 0x100 + 0x2000
    assert!(parse(&d, LO, HI, 0x2100).is_ok());
    assert_eq!(parse(&d, LO, HI, 0x20ff), Err(ElfError::TooBig));
    assert_eq!(parse(&d, LO, HI, 0), Err(ElfError::TooBig));
    // Sizes whose sum overflows u64 are refused too.
    let phs = [
        (PT_LOAD, RX, 0x1000, LO, 0, u64::MAX / 2),
        (
            PT_LOAD,
            RW,
            0x1000,
            LO + ((u64::MAX / 2) & !0xfff) + 0x1000,
            0,
            u64::MAX / 2,
        ),
    ];
    let r = parse(&build(LO, &phs, &[]), 0, u64::MAX, u64::MAX);
    assert!(r.is_err());
}

#[test]
fn entry_must_be_in_an_exec_segment() {
    let d = |e| p(&build(e, &good_phs(), &[0x90; 0x200]));
    assert!(d(LO).is_ok());
    assert!(d(LO + 0xff).is_ok());
    for e in [0, LO - 1, LO + 0x100, LO + 0x1000, LO + 0x1800, u64::MAX] {
        assert!(matches!(d(e), Err(ElfError::BadSegment(_))), "entry {e:#x}");
    }
}

#[test]
fn bounds_with_empty_range() {
    assert!(parse(&good(), HI, LO, MAX).is_err());
    assert!(parse(&good(), 0, 0, MAX).is_err());
}

#[test]
fn every_truncation_is_an_error_or_ok_without_panic() {
    let g = good();
    for n in 0..g.len() {
        let _ = p(&g[..n]);
    }
    // Cutting inside the header or the phdr table is always Truncated.
    for n in 0..(64 + 2 * 56) {
        assert!(p(&g[..n]).is_err(), "prefix {n}");
    }
}

#[test]
fn single_byte_corruption_never_panics_and_stays_valid() {
    let g = good();
    for i in 0..(64 + 2 * 56 + 8) {
        for v in [0u8, 1, 2, 3, 0x7f, 0x80, 0xff] {
            let mut d = g.clone();
            d[i] = v;
            if let Ok(prog) = p(&d) {
                for s in &prog.segments {
                    assert!(s.vaddr >= LO && s.vaddr + s.mem_size <= HI);
                    assert!(!(s.write && s.exec));
                    assert!(s.file_size as u64 <= s.mem_size);
                    assert!(s.file_off + s.file_size <= d.len());
                }
            }
        }
    }
}
