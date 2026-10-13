//! WAV (RIFF/WAVE) files with PCM audio: the header parser `play` uses and a header writer.
//!
//! Accepted: format 1 (integer PCM) with 8 (unsigned), 16, 24 or 32 bits, format 3 (IEEE float)
//! with 32 bits, and `WAVE_FORMAT_EXTENSIBLE` (0xFFFE) whose sub-format is one of those two; one or
//! two channels; any rate from 1 Hz to 384 kHz (whether the mixer takes it is the mixer's
//! business). Chunks other than `fmt ` and `data` are skipped (with their pad byte); a `data`
//! chunk that claims more than the file holds is cut to what is there (a recording that was not
//! closed), and a partial last frame is dropped. Everything is checked arithmetic: the file is
//! untrusted input.
//!
//! [`Wav::decode_s16`] turns any accepted sample format into interleaved signed 16-bit samples
//! (the format the kernel mixer takes), with the file's channel count.

/// Why a file is not a WAV this module reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WavError {
    /// Not `RIFF....WAVE`.
    NotWav,
    /// No `fmt ` chunk before `data`, or one too short.
    NoFormat,
    /// No `data` chunk.
    NoData,
    /// A sample format other than integer PCM or 32-bit float.
    Unsupported,
    /// More than two channels, or none.
    Channels,
    /// A rate of 0 or above 384 kHz, or a block size that does not match.
    BadHeader,
}

/// How samples are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFormat {
    /// Integer PCM of 8 (unsigned, 128 is silence), 16, 24 or 32 bits (signed).
    Int(u16),
    /// 32-bit IEEE float, -1.0..=1.0.
    Float32,
}

/// A parsed WAV: the format and the sample bytes (whole frames only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wav<'a> {
    pub rate: u32,
    pub channels: u16,
    pub format: SampleFormat,
    /// The `data` chunk, cut to a whole number of frames.
    pub data: &'a [u8],
}

const FMT_PCM: u16 = 1;
const FMT_FLOAT: u16 = 3;
const FMT_EXTENSIBLE: u16 = 0xFFFE;
/// Highest rate accepted.
pub const MAX_RATE: u32 = 384_000;

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]))
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

/// Parse a WAV file.
pub fn parse(file: &[u8]) -> Result<Wav<'_>, WavError> {
    if file.get(0..4) != Some(b"RIFF") || file.get(8..12) != Some(b"WAVE") {
        return Err(WavError::NotWav);
    }
    let mut pos = 12usize;
    let mut fmt: Option<(u16, u32, u16, u16)> = None; // (tag, rate, block_align, bits)
    let mut channels = 0u16;
    while let Some(id) = file.get(pos..pos + 4) {
        let size = u32_at(file, pos + 4).ok_or(WavError::NoData)? as usize;
        let body = pos + 8;
        let end = body.saturating_add(size).min(file.len());
        if id == b"fmt " {
            let f = file.get(body..end).ok_or(WavError::NoFormat)?;
            if f.len() < 16 {
                return Err(WavError::NoFormat);
            }
            let mut tag = u16_at(f, 0).ok_or(WavError::NoFormat)?;
            channels = u16_at(f, 2).ok_or(WavError::NoFormat)?;
            let rate = u32_at(f, 4).ok_or(WavError::NoFormat)?;
            let align = u16_at(f, 12).ok_or(WavError::NoFormat)?;
            let bits = u16_at(f, 14).ok_or(WavError::NoFormat)?;
            if tag == FMT_EXTENSIBLE {
                // cbSize(2) valid_bits(2) channel_mask(4) then the sub-format GUID, whose first two
                // bytes are the plain format tag.
                tag = u16_at(f, 24).ok_or(WavError::NoFormat)?;
            }
            fmt = Some((tag, rate, align, bits));
        } else if id == b"data" {
            let (tag, rate, align, bits) = fmt.ok_or(WavError::NoFormat)?;
            let format = match (tag, bits) {
                (FMT_PCM, 8 | 16 | 24 | 32) => SampleFormat::Int(bits),
                (FMT_FLOAT, 32) => SampleFormat::Float32,
                _ => return Err(WavError::Unsupported),
            };
            if !(1..=2).contains(&channels) {
                return Err(WavError::Channels);
            }
            if rate == 0
                || rate > MAX_RATE
                || u32::from(align) != u32::from(channels) * u32::from(bits / 8)
            {
                return Err(WavError::BadHeader);
            }
            let data = file.get(body..end).ok_or(WavError::NoData)?;
            let whole = data.len() - data.len() % usize::from(align);
            return Ok(Wav {
                rate,
                channels,
                format,
                data: &data[..whole],
            });
        }
        // Chunks are padded to an even size.
        pos = body.saturating_add(size).saturating_add(size & 1);
    }
    Err(if fmt.is_none() {
        WavError::NoFormat
    } else {
        WavError::NoData
    })
}

impl Wav<'_> {
    /// Bytes per frame (all channels of one instant).
    pub fn frame_bytes(&self) -> usize {
        usize::from(self.channels) * self.sample_bytes()
    }

    fn sample_bytes(&self) -> usize {
        match self.format {
            SampleFormat::Int(bits) => usize::from(bits / 8),
            SampleFormat::Float32 => 4,
        }
    }

    /// Frames in the file.
    pub fn frames(&self) -> usize {
        self.data.len() / self.frame_bytes().max(1)
    }

    /// Length in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        self.frames() as u64 * 1000 / u64::from(self.rate.max(1))
    }

    /// Decode frames from `first` on into `out` as interleaved signed 16-bit samples with the file's
    /// channel count. Returns how many frames were written (fewer than fit at the end of the file).
    pub fn decode_s16(&self, first: usize, out: &mut [i16]) -> usize {
        let ch = usize::from(self.channels);
        let sb = self.sample_bytes();
        let fb = self.frame_bytes();
        let total = self.frames();
        let n = (out.len() / ch.max(1)).min(total.saturating_sub(first));
        for f in 0..n {
            let base = (first + f) * fb;
            for c in 0..ch {
                let at = base + c * sb;
                out[f * ch + c] = self.sample(at);
            }
        }
        n
    }

    /// The sample at byte `at` of `data`, as signed 16 bits.
    fn sample(&self, at: usize) -> i16 {
        let b = |i: usize| self.data.get(at + i).copied().unwrap_or(0);
        match self.format {
            SampleFormat::Int(8) => (i16::from(b(0)) - 128) << 8,
            SampleFormat::Int(16) => i16::from_le_bytes([b(0), b(1)]),
            // 24 and 32 bits: keep the top 16 bits (truncation, like most players without dither).
            SampleFormat::Int(24) => i16::from_le_bytes([b(1), b(2)]),
            SampleFormat::Int(_) => i16::from_le_bytes([b(2), b(3)]),
            SampleFormat::Float32 => float_to_s16(u32::from_le_bytes([b(0), b(1), b(2), b(3)])),
        }
    }
}

/// An IEEE single (as bits) in -1.0..=1.0 to signed 16 bits, clamped, by integer arithmetic only
/// (the kernel target has no floating point). NaN reads as silence.
pub fn float_to_s16(bits: u32) -> i16 {
    let neg = bits >> 31 != 0;
    let exp = ((bits >> 23) & 0xFF) as i32;
    let mant = (bits & 0x7F_FFFF) | 0x80_0000; // implicit 1
    if exp == 0xFF {
        return if bits & 0x7F_FFFF != 0 {
            0
        } else if neg {
            i16::MIN
        } else {
            i16::MAX
        };
    }
    if exp < 127 - 24 {
        return 0; // tiny (denormals too)
    }
    // value = mant * 2^(exp - 127 - 23); scaled by 32768 = 2^15 -> mant * 2^(exp - 135).
    let shift = exp - 135;
    let mag: u64 = if shift >= 0 {
        if shift > 16 {
            u64::MAX
        } else {
            u64::from(mant) << shift
        }
    } else {
        u64::from(mant) >> (-shift).min(63)
    };
    if neg {
        -(mag.min(32768) as i32) as i16
    } else {
        mag.min(32767) as i16
    }
}

/// The 44-byte header of a plain PCM WAV with `data_len` bytes of samples.
pub fn header(rate: u32, channels: u16, bits: u16, data_len: u32) -> [u8; 44] {
    let align = channels * (bits / 8);
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&data_len.saturating_add(36).to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&FMT_PCM.to_le_bytes());
    h[22..24].copy_from_slice(&channels.to_le_bytes());
    h[24..28].copy_from_slice(&rate.to_le_bytes());
    h[28..32].copy_from_slice(&(rate * u32::from(align)).to_le_bytes());
    h[32..34].copy_from_slice(&align.to_le_bytes());
    h[34..36].copy_from_slice(&bits.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_len.to_le_bytes());
    h
}

#[cfg(test)]
mod tests;
