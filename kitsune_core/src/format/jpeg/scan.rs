//! Decoding one scan: the entropy-coded segments, the MCU grid and the blocks of a baseline scan.

use super::*;

/// One component of a scan.
pub(super) struct ScanComp {
    pub(super) ci: usize,
    pub(super) td: usize,
    pub(super) ta: usize,
}

/// Decode one scan; returns the position just after its entropy-coded data.
pub(super) fn decode_scan(
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
pub(super) fn scan_grid(frame: &Frame, sc: &[ScanComp]) -> (usize, usize, Vec<(usize, usize)>) {
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
pub(super) fn next_marker(bytes: &[u8], from: usize) -> usize {
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
pub(super) fn block(
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
