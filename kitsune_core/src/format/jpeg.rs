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

mod headers;
mod output;
mod scan;
use headers::*;
pub use output::*;
use scan::*;

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
#[cfg(test)]
mod tests;
