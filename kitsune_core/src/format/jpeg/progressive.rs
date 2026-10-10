//! Progressive JPEG (`SOF2`): spectral selection and successive approximation (ITU T.81 G).
//!
//! A progressive file sends the coefficients of each block in several scans (the DC terms first,
//! then bands of AC terms, each at a coarser and then a finer bit position), so every scan edits
//! the coefficients of the whole image. They are kept in memory as 16-bit values until the file
//! ends; only then are they dequantised and run through the IDCT into the component planes.
//! That costs two bytes per sample, so a frame is limited to [`MAX_SAMPLES`] samples.

use super::huffman::{Bits, Huff, extend};
use super::idct::{ZIGZAG, idct};
use super::{COEF_MAX, Frame, JpegError, ScanComp};

/// Most samples (all components, whole MCUs) of a progressive frame: 12 MiB of coefficients.
pub(super) const MAX_SAMPLES: usize = 6 * 1024 * 1024;

/// The spectral and successive-approximation parameters of a scan.
#[derive(Clone, Copy)]
pub(super) struct Params {
    pub ss: usize,
    pub se: usize,
    pub ah: u32,
    pub al: u32,
}

impl Params {
    /// The checks of T.81 G.1.1.1.1 that matter for decoding safely.
    pub(super) fn valid(&self, ns: usize) -> bool {
        self.ss <= self.se
            && self.se <= 63
            && self.al <= 13
            && self.ah <= 13
            // A DC scan may be interleaved, an AC scan covers one component.
            && if self.ss == 0 { self.se == 0 } else { ns == 1 }
    }
}

fn clamp16(v: i32) -> i16 {
    v.clamp(-COEF_MAX, COEF_MAX - 1) as i16
}

fn dc_first(
    bits: &mut Bits<'_>,
    dc: &Huff,
    pred: &mut i32,
    al: u32,
    blk: &mut [i16],
) -> Result<(), JpegError> {
    let t = bits.symbol(dc)? as u32;
    if t > 15 {
        return Err(JpegError::BadHuffman);
    }
    *pred = pred.wrapping_add(extend(bits.bits(t), t));
    blk[0] = clamp16(pred.wrapping_shl(al));
    Ok(())
}

fn dc_refine(bits: &mut Bits<'_>, al: u32, blk: &mut [i16]) {
    if bits.bits(1) == 1 {
        blk[0] |= 1 << al;
    }
}

fn ac_first(
    bits: &mut Bits<'_>,
    ac: &Huff,
    p: Params,
    eobrun: &mut u32,
    blk: &mut [i16],
) -> Result<(), JpegError> {
    if *eobrun > 0 {
        *eobrun -= 1;
        return Ok(());
    }
    let mut k = p.ss;
    while k <= p.se {
        let rs = bits.symbol(ac)?;
        let (r, s) = ((rs >> 4) as usize, (rs & 15) as u32);
        if s == 0 {
            if r < 15 {
                // An end-of-band run: this block and `eobrun` more have no coefficients here.
                *eobrun = (1u32 << r) - 1;
                if r > 0 {
                    *eobrun += bits.bits(r as u32);
                }
                break;
            }
            k += 16;
            continue;
        }
        k += r;
        if k > p.se {
            return Err(JpegError::BadHuffman);
        }
        let v = extend(bits.bits(s), s);
        blk[ZIGZAG[k] as usize] = clamp16(v.wrapping_shl(p.al));
        k += 1;
    }
    Ok(())
}

/// Correction bit for an already-nonzero coefficient.
fn refine_nonzero(bits: &mut Bits<'_>, c: &mut i16, p1: i16) {
    if bits.bits(1) == 1 && *c & p1 == 0 {
        *c = if *c >= 0 {
            c.saturating_add(p1)
        } else {
            c.saturating_sub(p1)
        };
    }
}

fn ac_refine(
    bits: &mut Bits<'_>,
    ac: &Huff,
    p: Params,
    eobrun: &mut u32,
    blk: &mut [i16],
) -> Result<(), JpegError> {
    let p1: i16 = 1 << p.al;
    let m1: i16 = -p1;
    let mut k = p.ss;
    if *eobrun == 0 {
        while k <= p.se {
            let rs = bits.symbol(ac)?;
            let (mut r, s) = ((rs >> 4) as i32, (rs & 15) as u32);
            let mut new: i16 = 0;
            if s != 0 {
                if s != 1 {
                    return Err(JpegError::BadHuffman);
                }
                new = if bits.bits(1) == 1 { p1 } else { m1 };
            } else if r != 15 {
                // End of band: this block and `eobrun - 1` more only get correction bits.
                *eobrun = 1u32 << r;
                if r > 0 {
                    *eobrun += bits.bits(r as u32);
                }
                break;
            }
            // Step over the nonzero coefficients (each takes a correction bit) and `r` zeros.
            while k <= p.se {
                let c = &mut blk[ZIGZAG[k] as usize];
                if *c != 0 {
                    refine_nonzero(bits, c, p1);
                } else {
                    r -= 1;
                    if r < 0 {
                        break;
                    }
                }
                k += 1;
            }
            if new != 0 {
                if k > p.se {
                    return Err(JpegError::BadHuffman);
                }
                blk[ZIGZAG[k] as usize] = new;
            }
            k += 1;
        }
    }
    if *eobrun > 0 {
        while k <= p.se {
            let c = &mut blk[ZIGZAG[k] as usize];
            if *c != 0 {
                refine_nonzero(bits, c, p1);
            }
            k += 1;
        }
        *eobrun -= 1;
    }
    Ok(())
}

/// Decode one progressive scan into the coefficient buffers; returns the position just after its
/// entropy-coded data (the caller finds the next marker from there).
#[allow(clippy::too_many_arguments)]
pub(super) fn scan(
    bytes: &[u8],
    data_start: usize,
    sc: &[ScanComp],
    p: Params,
    frame: &mut Frame,
    dc: &[Option<Huff>; 4],
    ac: &[Option<Huff>; 4],
    restart: usize,
) -> Result<usize, JpegError> {
    let ns = sc.len();
    let mut bits = Bits::new(bytes, data_start);
    let mut pred = [0i32; 4];
    let mut eobrun = 0u32;
    let mut unit = 0usize;
    let (cols, rows, per_mcu) = super::scan_grid(frame, sc);
    let mut skipping = false;
    'scan: for my in 0..rows {
        for mx in 0..cols {
            if restart > 0 && unit > 0 && unit.is_multiple_of(restart) {
                if bits.restart().is_err() {
                    break 'scan;
                }
                pred = [0; 4];
                eobrun = 0;
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
                        let c = &mut frame.comps[x.ci];
                        if bx >= c.bw || by >= c.bh {
                            continue;
                        }
                        let at = (by * c.bw + bx) * 64;
                        let blk = &mut c.coef[at..at + 64];
                        let r = if p.ss == 0 {
                            if p.ah == 0 {
                                let t = dc[x.td].as_ref().ok_or(JpegError::BadScan)?;
                                dc_first(&mut bits, t, &mut pred[x.ci], p.al, blk)
                            } else {
                                dc_refine(&mut bits, p.al, blk);
                                Ok(())
                            }
                        } else {
                            let t = ac[x.ta].as_ref().ok_or(JpegError::BadScan)?;
                            if p.ah == 0 {
                                ac_first(&mut bits, t, p, &mut eobrun, blk)
                            } else {
                                ac_refine(&mut bits, t, p, &mut eobrun, blk)
                            }
                        };
                        if r.is_err() || bits.overrun {
                            if restart > 0 {
                                skipping = true;
                                break 'mcu;
                            }
                            break 'scan;
                        }
                    }
                }
            }
        }
    }
    Ok(super::next_marker(bytes, bits.pos))
}

/// Dequantise every block and run the IDCT into the component planes.
pub(super) fn finish(frame: &mut Frame) -> Result<(), JpegError> {
    for c in &mut frame.comps {
        let qt = c.qlatch.ok_or(JpegError::BadScan)?;
        let mut plane: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        plane
            .try_reserve_exact(c.pw * c.ph)
            .map_err(|_| JpegError::Image(super::ImageError::OutOfMemory))?;
        plane.resize(c.pw * c.ph, 128);
        for by in 0..c.bh {
            for bx in 0..c.bw {
                let blk = &c.coef[(by * c.bw + bx) * 64..][..64];
                let mut coef = [0i32; 64];
                for (i, v) in coef.iter_mut().enumerate() {
                    *v = (i32::from(blk[i]) * i32::from(qt[i])).clamp(-COEF_MAX, COEF_MAX);
                }
                let s = idct(&coef);
                for r in 0..8 {
                    let o = (by * 8 + r) * c.pw + bx * 8;
                    plane[o..o + 8].copy_from_slice(&s[r * 8..r * 8 + 8]);
                }
            }
        }
        c.plane = plane;
        c.coef = alloc::vec::Vec::new();
    }
    Ok(())
}
