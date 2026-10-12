//! Marker segments before the scans: quantisation tables, Huffman tables and the frame header.

use super::*;

pub(super) fn read_dqt(s: &mut Seg<'_>, st: &mut State) -> Result<(), JpegError> {
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

pub(super) fn read_dht(s: &mut Seg<'_>, st: &mut State) -> Result<(), JpegError> {
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

pub(super) fn read_sof(s: &mut Seg<'_>, progressive: bool) -> Result<Frame, JpegError> {
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
