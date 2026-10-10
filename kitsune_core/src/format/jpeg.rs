//! JPEG decoder: baseline, extended-sequential and progressive (8-bit, Huffman) images.
//!
//! * Frames `SOF0` (baseline) and `SOF1` (extended sequential, 8-bit): gray (1 component) and
//!   YCbCr (3 components, any sampling factors 1 to 4, so 4:4:4, 4:2:2, 4:2:0, 4:1:1...); an
//!   Adobe `APP14` marker with transform 0 marks the three components as plain RGB.
//! * Interleaved and non-interleaved scans, several scans per frame, restart intervals,
//!   8- and 16-bit quantisation tables, Huffman tables redefined between scans.
//! * Progressive frames (`SOF2`, see `progressive.rs`): spectral selection and successive
//!   approximation, decoded when the file ends; limited to a few megapixels because the
//!   coefficients are kept in memory.
//! * **Not supported**, each reported as [`JpegError::Unsupported`] instead of a wrong picture:
//!   arithmetic coding, lossless, 12-bit samples, CMYK/YCCK and progressive frames too large.
//! * Chroma is replicated (no smoothing), the IDCT is the integer one of `idct.rs`.
//! * A file cut short (or with damaged entropy data) decodes as far as it goes and the rest of
//!   the picture is mid-gray; a file with no scan at all is [`JpegError::Truncated`].
//! * The size is checked against [`image::MAX_PIXELS`] before anything is allocated, and
//!   every loop is bounded by the frame, so hostile input cannot make it run or allocate more
//!   than a valid image of that size would.
//!
//! The output is opaque (alpha 255). Pure and `forbid(unsafe)`; the `image_decode` fuzz target
//! drives it through [`image::decode`].

mod huffman;
mod idct;
mod progressive;

use crate::format::image::{self, Image, ImageError, rgba};
use alloc::vec::Vec;
use core::fmt;
use huffman::{Bits, Huff, extend};
use idct::{ZIGZAG, idct};

/// Why a JPEG was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JpegError {
    /// Does not start with `FF D8`.
    BadSignature,
    /// Ends before a frame and a scan were read.
    Truncated,
    /// A marker where one cannot be, a bad length, or a missing/duplicate frame header.
    BadMarker(u8),
    BadHuffman,
    /// A scan refers to a table that was never defined, or has impossible parameters.
    BadScan,
    BadQuant,
    /// Zero or too many pixels.
    BadSize,
    Unsupported(Feature),
    Image(ImageError),
}

/// What a JPEG uses that this decoder does not do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    /// A progressive frame over [`progressive::MAX_SAMPLES`] samples.
    Progressive,
    Arithmetic,
    Lossless,
    /// Sample precision other than 8 bits.
    Precision,
    /// Four components (CMYK/YCCK) or a count other than 1 and 3.
    Components,
    /// Height given by a DNL marker after the first scan.
    Dnl,
}

impl fmt::Display for JpegError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JpegError::BadSignature => f.write_str("not a JPEG file"),
            JpegError::Truncated => f.write_str("the JPEG is cut short"),
            JpegError::BadMarker(m) => write!(f, "unexpected JPEG marker {m:#04x}"),
            JpegError::BadHuffman => f.write_str("invalid Huffman table"),
            JpegError::BadScan => f.write_str("invalid JPEG scan"),
            JpegError::BadQuant => f.write_str("invalid quantisation table"),
            JpegError::BadSize => f.write_str("the JPEG has an invalid or too large size"),
            JpegError::Unsupported(Feature::Progressive) => {
                f.write_str("this progressive JPEG is too large to decode here")
            }
            JpegError::Unsupported(Feature::Arithmetic) => {
                f.write_str("arithmetic-coded JPEG is not supported")
            }
            JpegError::Unsupported(Feature::Lossless) => {
                f.write_str("lossless JPEG is not supported")
            }
            JpegError::Unsupported(Feature::Precision) => {
                f.write_str("only 8-bit JPEG is supported")
            }
            JpegError::Unsupported(Feature::Components) => {
                f.write_str("only gray and YCbCr/RGB JPEG is supported")
            }
            JpegError::Unsupported(Feature::Dnl) => f.write_str("JPEG with a DNL marker"),
            JpegError::Image(e) => write!(f, "{e}"),
        }
    }
}

impl From<ImageError> for JpegError {
    fn from(e: ImageError) -> Self {
        JpegError::Image(e)
    }
}

/// Does `bytes` start like a JPEG (`FF D8 FF`)?
pub fn is_jpeg(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xD8, 0xFF])
}

/// Dequantised coefficients are limited to 16 bits: a valid file stays far below, a hostile one
/// cannot push the IDCT into silly numbers.
const COEF_MAX: i32 = 1 << 15;

struct Comp {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    /// Plane width/height in samples (whole blocks, whole MCUs).
    pw: usize,
    ph: usize,
    plane: Vec<u8>,
    /// Blocks per row and column of the plane (progressive frames).
    bw: usize,
    bh: usize,
    /// Progressive frames: the coefficients of every block, natural order, not yet dequantised.
    coef: Vec<i16>,
    /// The quantisation table in effect when this component was first scanned (progressive).
    qlatch: Option<[u16; 64]>,
}

struct Frame {
    progressive: bool,
    width: usize,
    height: usize,
    hmax: usize,
    vmax: usize,
    mcux: usize,
    mcuy: usize,
    comps: Vec<Comp>,
}

struct State {
    qt: [Option<[u16; 64]>; 4],
    dc: [Option<Huff>; 4],
    ac: [Option<Huff>; 4],
    restart: usize,
    adobe: Option<u8>,
    frame: Option<Frame>,
    scans: usize,
}

struct Seg<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Seg<'a> {
    fn u8(&mut self) -> Result<u8, JpegError> {
        let v = *self.b.get(self.p).ok_or(JpegError::Truncated)?;
        self.p += 1;
        Ok(v)
    }
    fn u16(&mut self) -> Result<u16, JpegError> {
        Ok((self.u8()? as u16) << 8 | self.u8()? as u16)
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], JpegError> {
        let s = self
            .b
            .get(self.p..self.p.checked_add(n).ok_or(JpegError::Truncated)?)
            .ok_or(JpegError::Truncated)?;
        self.p += n;
        Ok(s)
    }
}

/// Decode a baseline or extended-sequential JPEG.
pub fn decode(bytes: &[u8]) -> Result<Image, JpegError> {
    if !is_jpeg(bytes) {
        return Err(JpegError::BadSignature);
    }
    let mut st = State {
        qt: [None; 4],
        dc: [None, None, None, None],
        ac: [None, None, None, None],
        restart: 0,
        adobe: None,
        frame: None,
        scans: 0,
    };
    let mut pos = 2usize;
    loop {
        // Find the next marker: 0xFF, then a non-0, non-0xFF byte (extra 0xFF are fill).
        while bytes.get(pos) == Some(&0xFF) && bytes.get(pos + 1) == Some(&0xFF) {
            pos += 1;
        }
        let (Some(&0xFF), Some(&m)) = (bytes.get(pos), bytes.get(pos + 1)) else {
            break; // the file ends (or garbage): use what we have
        };
        pos += 2;
        match m {
            0xD8 | 0x01 | 0xD0..=0xD7 => continue,
            0xD9 => break,
            _ => {}
        }
        let mut s = Seg { b: bytes, p: pos };
        let Ok(len) = s.u16().map(usize::from) else {
            break; // cut inside a marker: keep what was decoded
        };
        if len < 2 {
            return Err(JpegError::BadMarker(m));
        }
        let body_end = pos.checked_add(len).ok_or(JpegError::Truncated)?;
        if body_end > bytes.len() {
            // A cut-off segment: nothing more can be read, but earlier scans may be usable.
            break;
        }
        let mut seg = Seg {
            b: &bytes[..body_end],
            p: pos + 2,
        };
        match m {
            0xDB => read_dqt(&mut seg, &mut st)?,
            0xC4 => read_dht(&mut seg, &mut st)?,
            0xC0..=0xC2 => {
                if st.frame.is_some() {
                    return Err(JpegError::BadMarker(m));
                }
                st.frame = Some(read_sof(&mut seg, m == 0xC2)?);
            }
            0xC3 | 0xC5..=0xC7 | 0xCB | 0xCD..=0xCF => {
                return Err(JpegError::Unsupported(Feature::Lossless));
            }
            0xC9 | 0xCA => return Err(JpegError::Unsupported(Feature::Arithmetic)),
            0xCC => return Err(JpegError::Unsupported(Feature::Arithmetic)),
            0xDC => return Err(JpegError::Unsupported(Feature::Dnl)),
            0xDD => {
                if len != 4 {
                    return Err(JpegError::BadMarker(m));
                }
                st.restart = seg.u16()? as usize;
            }
            0xEE => {
                let d = &bytes[pos + 2..body_end];
                if d.len() >= 12 && &d[..5] == b"Adobe" {
                    st.adobe = Some(d[11]);
                }
            }
            0xDA => {
                let end = decode_scan(bytes, body_end, &mut seg, &mut st)?;
                st.scans += 1;
                pos = end;
                continue;
            }
            _ => {}
        }
        pos = body_end;
    }
    let mut frame = st.frame.take().ok_or(JpegError::Truncated)?;
    if st.scans == 0 {
        return Err(JpegError::Truncated);
    }
    if frame.progressive {
        progressive::finish(&mut frame)?;
    }
    to_image(&frame, st.adobe)
}

fn read_dqt(s: &mut Seg<'_>, st: &mut State) -> Result<(), JpegError> {
    while s.p < s.b.len() {
        let pt = s.u8()?;
        let (prec, id) = (pt >> 4, (pt & 15) as usize);
        if prec > 1 || id > 3 {
            return Err(JpegError::BadQuant);
        }
        let mut q = [0u16; 64];
        for &z in ZIGZAG.iter() {
            let v = if prec == 0 { s.u8()? as u16 } else { s.u16()? };
            q[z as usize] = v;
        }
        st.qt[id] = Some(q);
    }
    Ok(())
}

fn read_dht(s: &mut Seg<'_>, st: &mut State) -> Result<(), JpegError> {
    while s.p < s.b.len() {
        let tc = s.u8()?;
        let (class, id) = (tc >> 4, (tc & 15) as usize);
        if class > 1 || id > 3 {
            return Err(JpegError::BadHuffman);
        }
        let mut counts = [0u8; 16];
        counts.copy_from_slice(s.take(16)?);
        let total: usize = counts.iter().map(|&c| c as usize).sum();
        let vals = s.take(total)?;
        let h = Huff::new(&counts, vals)?;
        if class == 0 {
            st.dc[id] = Some(h);
        } else {
            st.ac[id] = Some(h);
        }
    }
    Ok(())
}

fn read_sof(s: &mut Seg<'_>, progressive: bool) -> Result<Frame, JpegError> {
    if s.u8()? != 8 {
        return Err(JpegError::Unsupported(Feature::Precision));
    }
    let height = s.u16()? as usize;
    let width = s.u16()? as usize;
    let n = s.u8()? as usize;
    if width == 0 {
        return Err(JpegError::BadSize);
    }
    if height == 0 {
        return Err(JpegError::Unsupported(Feature::Dnl));
    }
    image::pixel_count(width, height).map_err(|_| JpegError::BadSize)?;
    if n != 1 && n != 3 {
        return Err(JpegError::Unsupported(Feature::Components));
    }
    let mut raw: Vec<(u8, usize, usize, usize)> = Vec::with_capacity(n);
    for _ in 0..n {
        let id = s.u8()?;
        let hv = s.u8()?;
        let tq = s.u8()? as usize;
        let (h, v) = ((hv >> 4) as usize, (hv & 15) as usize);
        if !(1..=4).contains(&h) || !(1..=4).contains(&v) || tq > 3 {
            return Err(JpegError::BadScan);
        }
        if raw.iter().any(|&(i, ..)| i == id) {
            return Err(JpegError::BadScan);
        }
        raw.push((id, h, v, tq));
    }
    let hmax = raw.iter().map(|c| c.1).max().unwrap_or(1);
    let vmax = raw.iter().map(|c| c.2).max().unwrap_or(1);
    let mcux = width.div_ceil(8 * hmax);
    let mcuy = height.div_ceil(8 * vmax);
    let mut comps = Vec::with_capacity(n);
    let mut total = 0usize;
    for (id, h, v, tq) in raw {
        let (pw, ph) = (mcux * h * 8, mcuy * v * 8);
        total = total
            .checked_add(pw.checked_mul(ph).ok_or(JpegError::BadSize)?)
            .ok_or(JpegError::BadSize)?;
        comps.push(Comp {
            id,
            h,
            v,
            tq,
            pw,
            ph,
            plane: Vec::new(),
            bw: pw / 8,
            bh: ph / 8,
            coef: Vec::new(),
            qlatch: None,
        });
    }
    if progressive && total > progressive::MAX_SAMPLES {
        return Err(JpegError::Unsupported(Feature::Progressive));
    }
    // Planes are whole MCUs and sampling can be up to 4x4: bound them like the image itself.
    if total > image::MAX_PIXELS * 3 {
        return Err(JpegError::BadSize);
    }
    for c in &mut comps {
        if progressive {
            // The planes come at the end, once the coefficients are final.
            let mut v: Vec<i16> = Vec::new();
            v.try_reserve_exact(c.pw * c.ph)
                .map_err(|_| JpegError::Image(ImageError::OutOfMemory))?;
            v.resize(c.pw * c.ph, 0);
            c.coef = v;
            continue;
        }
        let mut v: Vec<u8> = Vec::new();
        v.try_reserve_exact(c.pw * c.ph)
            .map_err(|_| JpegError::Image(ImageError::OutOfMemory))?;
        v.resize(c.pw * c.ph, 128);
        c.plane = v;
    }
    Ok(Frame {
        progressive,
        width,
        height,
        hmax,
        vmax,
        mcux,
        mcuy,
        comps,
    })
}

/// One component of a scan.
struct ScanComp {
    ci: usize,
    td: usize,
    ta: usize,
}

/// Decode one scan; returns the position just after its entropy-coded data.
fn decode_scan(
    bytes: &[u8],
    data_start: usize,
    hdr: &mut Seg<'_>,
    st: &mut State,
) -> Result<usize, JpegError> {
    let frame = st.frame.as_mut().ok_or(JpegError::BadScan)?;
    let ns = hdr.u8()? as usize;
    if ns == 0 || ns > frame.comps.len() {
        return Err(JpegError::BadScan);
    }
    let mut sc: Vec<ScanComp> = Vec::with_capacity(ns);
    for _ in 0..ns {
        let cs = hdr.u8()?;
        let t = hdr.u8()?;
        let ci = frame
            .comps
            .iter()
            .position(|c| c.id == cs)
            .ok_or(JpegError::BadScan)?;
        if sc.iter().any(|x| x.ci == ci) {
            return Err(JpegError::BadScan);
        }
        let (td, ta) = ((t >> 4) as usize, (t & 15) as usize);
        if td > 3 || ta > 3 {
            return Err(JpegError::BadScan);
        }
        sc.push(ScanComp { ci, td, ta });
    }
    let (ss, se, ahl) = (hdr.u8()?, hdr.u8()?, hdr.u8()?);
    if frame.progressive {
        let p = progressive::Params {
            ss: ss as usize,
            se: se as usize,
            ah: u32::from(ahl >> 4),
            al: u32::from(ahl & 15),
        };
        if !p.valid(ns) {
            return Err(JpegError::BadScan);
        }
        for x in &sc {
            let c = &mut frame.comps[x.ci];
            // The tables a scan needs: DC first scans use the DC one, every AC scan the AC one.
            let needs_dc = p.ss == 0 && p.ah == 0;
            if (needs_dc && st.dc[x.td].is_none()) || (p.ss > 0 && st.ac[x.ta].is_none()) {
                return Err(JpegError::BadScan);
            }
            if c.qlatch.is_none() {
                c.qlatch = Some(st.qt[c.tq].ok_or(JpegError::BadScan)?);
            }
        }
        return progressive::scan(bytes, data_start, &sc, p, frame, &st.dc, &st.ac, st.restart);
    }
    // Sequential: the whole spectrum, no successive approximation.
    if ss != 0 || se != 63 || ahl != 0 {
        return Err(JpegError::BadScan);
    }
    for x in &sc {
        let c = &frame.comps[x.ci];
        if st.qt[c.tq].is_none() || st.dc[x.td].is_none() || st.ac[x.ta].is_none() {
            return Err(JpegError::BadScan);
        }
    }

    let mut bits = Bits::new(bytes, data_start);
    let mut pred = [0i32; 4];
    let mut unit = 0usize;
    let restart = st.restart;

    let (cols, rows, per_mcu) = scan_grid(frame, &sc);

    // A damaged or cut-off interval is abandoned (its blocks stay mid-gray); with restart markers
    // decoding resumes at the next interval, otherwise the scan ends there.
    let mut skipping = false;
    'scan: for my in 0..rows {
        for mx in 0..cols {
            if restart > 0 && unit > 0 && unit.is_multiple_of(restart) {
                if bits.restart().is_err() {
                    break 'scan;
                }
                pred = [0; 4];
                skipping = false;
            }
            unit += 1;
            if skipping {
                continue;
            }
            'mcu: for (k, x) in sc.iter().enumerate() {
                let (bh, bv) = per_mcu[k];
                for vy in 0..bv {
                    for vx in 0..bh {
                        let (bx, by) = if ns == 1 {
                            (mx, my)
                        } else {
                            (mx * bh + vx, my * bv + vy)
                        };
                        let qt = st.qt[frame.comps[x.ci].tq]
                            .as_ref()
                            .ok_or(JpegError::BadScan)?;
                        let dc = st.dc[x.td].as_ref().ok_or(JpegError::BadScan)?;
                        let ac = st.ac[x.ta].as_ref().ok_or(JpegError::BadScan)?;
                        let mut coef = [0i32; 64];
                        let ok = block(&mut bits, dc, ac, qt, &mut pred[x.ci], &mut coef).is_ok();
                        if !ok || bits.overrun {
                            if restart > 0 {
                                skipping = true;
                                break 'mcu;
                            }
                            break 'scan;
                        }
                        let samples = idct(&coef);
                        let c = &mut frame.comps[x.ci];
                        let (px, py) = (bx * 8, by * 8);
                        if px + 8 <= c.pw && py + 8 <= c.ph {
                            for r in 0..8 {
                                let o = (py + r) * c.pw + px;
                                c.plane[o..o + 8].copy_from_slice(&samples[r * 8..r * 8 + 8]);
                            }
                        }
                    }
                }
            }
        }
    }
    // Resume marker parsing after the entropy data: at the marker the reader stopped on, or the
    // next one found by scanning.
    Ok(next_marker(bytes, bits.pos))
}

/// The MCU grid of a scan: (columns, rows, blocks per MCU of each scan component as (h, v)).
/// A scan of one component is non-interleaved: its MCU is one block and the grid covers the
/// component's real size, not the padded frame.
fn scan_grid(frame: &Frame, sc: &[ScanComp]) -> (usize, usize, Vec<(usize, usize)>) {
    if sc.len() == 1 {
        let c = &frame.comps[sc[0].ci];
        let cw = (frame.width * c.h).div_ceil(frame.hmax);
        let chh = (frame.height * c.v).div_ceil(frame.vmax);
        (cw.div_ceil(8), chh.div_ceil(8), alloc::vec![(1, 1)])
    } else {
        (
            frame.mcux,
            frame.mcuy,
            sc.iter()
                .map(|x| (frame.comps[x.ci].h, frame.comps[x.ci].v))
                .collect(),
        )
    }
}

/// The position of the next marker at or after `from` (not a restart marker, not `FF 00`).
fn next_marker(bytes: &[u8], from: usize) -> usize {
    let mut p = from;
    while p + 1 < bytes.len() {
        if bytes[p] == 0xFF
            && bytes[p + 1] != 0
            && !(0xD0..=0xD7).contains(&bytes[p + 1])
            && bytes[p + 1] != 0xFF
        {
            break;
        }
        p += 1;
    }
    p
}

/// Decode one 8x8 block into dequantised coefficients (natural order).
fn block(
    bits: &mut Bits<'_>,
    dc: &Huff,
    ac: &Huff,
    qt: &[u16; 64],
    pred: &mut i32,
    coef: &mut [i32; 64],
) -> Result<(), JpegError> {
    let t = bits.symbol(dc)? as u32;
    if t > 15 {
        return Err(JpegError::BadHuffman);
    }
    let diff = extend(bits.bits(t), t);
    *pred = pred.wrapping_add(diff);
    coef[0] = pred.wrapping_mul(qt[0] as i32).clamp(-COEF_MAX, COEF_MAX);
    let mut k = 1usize;
    while k < 64 {
        let rs = bits.symbol(ac)?;
        let (r, s) = ((rs >> 4) as usize, (rs & 15) as u32);
        if s == 0 {
            if r == 15 {
                k += 16;
                continue;
            }
            break;
        }
        k += r;
        if k > 63 {
            return Err(JpegError::BadHuffman);
        }
        let v = extend(bits.bits(s), s);
        let p = ZIGZAG[k] as usize;
        coef[p] = v.wrapping_mul(qt[p] as i32).clamp(-COEF_MAX, COEF_MAX);
        k += 1;
    }
    Ok(())
}

fn to_image(f: &Frame, adobe: Option<u8>) -> Result<Image, JpegError> {
    let (w, h) = (f.width, f.height);
    let mut img = Image::new(w, h, 0xFF00_0000)?;
    let px = img.pixels_mut();
    let sample = |c: &Comp, x: usize, y: usize| -> i32 {
        let sx = x * c.h / f.hmax;
        let sy = y * c.v / f.vmax;
        c.plane[sy * c.pw + sx] as i32
    };
    if f.comps.len() == 1 {
        let c = &f.comps[0];
        for y in 0..h {
            for x in 0..w {
                let g = sample(c, x, y) as u8;
                px[y * w + x] = rgba(g, g, g, 255);
            }
        }
        return Ok(img);
    }
    let rgb = adobe == Some(0);
    for y in 0..h {
        for x in 0..w {
            let a = sample(&f.comps[0], x, y);
            let b = sample(&f.comps[1], x, y);
            let c = sample(&f.comps[2], x, y);
            let (r, g, bl) = if rgb {
                (a, b, c)
            } else {
                let (cb, cr) = (b - 128, c - 128);
                let yy = (a << 16) + 32768;
                (
                    (yy + 91881 * cr) >> 16,
                    (yy - 22554 * cb - 46802 * cr) >> 16,
                    (yy + 116130 * cb) >> 16,
                )
            };
            px[y * w + x] = rgba(
                r.clamp(0, 255) as u8,
                g.clamp(0, 255) as u8,
                bl.clamp(0, 255) as u8,
                255,
            );
        }
    }
    Ok(img)
}

/// Width and height from the first `SOF` marker, without decoding anything. `None` when there is
/// no frame header in the bytes given.
pub fn peek_dims(bytes: &[u8]) -> Option<(usize, usize)> {
    if !is_jpeg(bytes) {
        return None;
    }
    let mut p = 2usize;
    while p + 4 <= bytes.len() {
        if bytes[p] != 0xFF {
            p += 1;
            continue;
        }
        let m = bytes[p + 1];
        if m == 0xFF {
            p += 1;
            continue;
        }
        if m == 0 || m == 0x01 || (0xD0..=0xD8).contains(&m) {
            p += 2;
            continue;
        }
        if (0xC0..=0xCF).contains(&m) && m != 0xC4 && m != 0xC8 && m != 0xCC {
            let h = u16::from_be_bytes([*bytes.get(p + 5)?, *bytes.get(p + 6)?]);
            let w = u16::from_be_bytes([*bytes.get(p + 7)?, *bytes.get(p + 8)?]);
            return Some((usize::from(w), usize::from(h)));
        }
        let len = u16::from_be_bytes([bytes[p + 2], bytes[p + 3]]) as usize;
        p += 2 + len;
    }
    None
}

#[cfg(test)]
mod tests;
