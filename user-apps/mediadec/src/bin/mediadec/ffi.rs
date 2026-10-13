//! Safe wrappers over the C side (`csrc/media.h`): the VP9 decoder (libvpx), the Opus decoder
//! (libopus, fixed point) and the I420 to RGBA conversion. Only integers and pointers cross (this
//! Rust may be soft-float, the C uses SSE2).
//!
//! Shared by `mediadec` (ring 3) and the host checker `tools/media/mediacheck`, which compiles the
//! same C with the host's compiler.

use core::marker::PhantomData;
use core::ptr::NonNull;

/// `kmd_frame` of `media.h`.
#[repr(C)]
#[derive(Clone, Copy)]
struct RawFrame {
    plane: [*const u8; 3],
    stride: [i32; 3],
    width: i32,
    height: i32,
    color_space: i32,
    color_range: i32,
    format: i32,
    bit_depth: i32,
}

#[repr(C)]
struct RawVp9 {
    _opaque: [u8; 0],
}

#[repr(C)]
struct RawOpus {
    _opaque: [u8; 0],
}

unsafe extern "C" {
    fn kmd_vp9_new() -> *mut RawVp9;
    fn kmd_vp9_decode(d: *mut RawVp9, data: *const u8, len: usize) -> i32;
    fn kmd_vp9_frame(d: *mut RawVp9, out: *mut RawFrame) -> i32;
    fn kmd_vp9_error(d: *mut RawVp9) -> *const u8;
    fn kmd_vp9_free(d: *mut RawVp9);
    fn kmd_opus_new() -> *mut RawOpus;
    fn kmd_opus_decode(
        d: *mut RawOpus,
        data: *const u8,
        len: usize,
        pcm: *mut i16,
        max_samples: i32,
    ) -> i32;
    fn kmd_opus_reset(d: *mut RawOpus);
    fn kmd_opus_set_gain(d: *mut RawOpus, gain_q8: i32);
    fn kmd_opus_free(d: *mut RawOpus);
    fn kmd_i420_to_rgba(f: *const RawFrame, dst: *mut u32, w: i32, h: i32, stride: i32);
}

/// `VPX_IMG_FMT_I420`: the only layout shown (VP9 profile 0).
const FMT_I420: i32 = 0x102;

/// A VP9 decoder.
pub struct Vp9 {
    raw: NonNull<RawVp9>,
    /// The picture the last decode showed.
    shown: Option<RawFrame>,
}

impl Vp9 {
    /// `None` when the decoder cannot be made (out of memory).
    pub fn new() -> Option<Vp9> {
        // SAFETY: no arguments; the result is null or a decoder we now own.
        NonNull::new(unsafe { kmd_vp9_new() }).map(|raw| Vp9 { raw, shown: None })
    }

    /// Decode one compressed frame; the picture it shows (VP9 gives at most one per call, a hidden
    /// alt-ref frame gives none) is then [`Vp9::picture`].
    pub fn decode(&mut self, data: &[u8]) -> Result<(), i32> {
        self.shown = None;
        // SAFETY: `raw` is our live decoder; `data` is valid for its length during the call (libvpx
        // does not keep the pointer: a frame refers to its own buffers).
        let r = unsafe { kmd_vp9_decode(self.raw.as_ptr(), data.as_ptr(), data.len()) };
        if r != 0 {
            return Err(r);
        }
        let mut f = RawFrame {
            plane: [core::ptr::null(); 3],
            stride: [0; 3],
            width: 0,
            height: 0,
            color_space: 0,
            color_range: 0,
            format: 0,
            bit_depth: 0,
        };
        // SAFETY: `raw` is live, `f` is a writable `kmd_frame`.
        let got = unsafe { kmd_vp9_frame(self.raw.as_ptr(), &mut f) };
        let usable = got == 1
            && f.format == FMT_I420
            && f.bit_depth == 8
            && f.width > 0
            && f.height > 0
            && f.stride.iter().all(|&s| s > 0)
            && f.plane.iter().all(|p| !p.is_null());
        self.shown = usable.then_some(f);
        Ok(())
    }

    /// The picture the last decode showed, valid until the next decode.
    pub fn picture(&self) -> Option<Picture<'_>> {
        self.shown.map(|raw| Picture {
            raw,
            _decoder: PhantomData,
        })
    }

    /// libvpx's text for the last error (ASCII).
    pub fn error(&self) -> &str {
        // SAFETY: the decoder returns a NUL-terminated static or decoder-owned string that lives at
        // least until the next call on it, which `&self` prevents.
        unsafe {
            let p = kmd_vp9_error(self.raw.as_ptr());
            if p.is_null() {
                return "";
            }
            let mut n = 0;
            while *p.add(n) != 0 && n < 200 {
                n += 1;
            }
            core::str::from_utf8(core::slice::from_raw_parts(p, n)).unwrap_or("?")
        }
    }
}

impl Drop for Vp9 {
    fn drop(&mut self) {
        // SAFETY: we own the decoder and nothing refers to it after this.
        unsafe { kmd_vp9_free(self.raw.as_ptr()) }
    }
}

/// A decoded 8-bit 4:2:0 picture, borrowed from its decoder.
pub struct Picture<'a> {
    raw: RawFrame,
    _decoder: PhantomData<&'a Vp9>,
}

impl Picture<'_> {
    pub fn width(&self) -> u32 {
        self.raw.width as u32
    }

    pub fn height(&self) -> u32 {
        self.raw.height as u32
    }

    /// `(vpx_color_space_t, vpx_color_range_t)`: 2 is BT.709, range 1 is full.
    pub fn color(&self) -> (i32, i32) {
        (self.raw.color_space, self.raw.color_range)
    }

    /// Row `y` of plane `p` (0 Y, 1 U, 2 V), cropped to the visible width of that plane.
    pub fn row(&self, p: usize, y: u32) -> &[u8] {
        let (w, h) = self.plane_size(p);
        if y >= h {
            return &[];
        }
        let stride = self.raw.stride[p] as usize;
        // SAFETY: libvpx's plane `p` has `h` rows of `stride` bytes, each with at least `w` visible
        // bytes, alive until the next decoder call (the borrow of the decoder holds that off).
        unsafe {
            core::slice::from_raw_parts(self.raw.plane[p].add(y as usize * stride), w as usize)
        }
    }

    /// Width and height of plane `p`.
    pub fn plane_size(&self, p: usize) -> (u32, u32) {
        let (w, h) = (self.width(), self.height());
        if p == 0 {
            (w, h)
        } else {
            (w.div_ceil(2), h.div_ceil(2))
        }
    }

    /// Feed the planes, packed (Y, then U, then V, no padding), to `f`: the layout `ffmpeg -f
    /// framemd5` hashes.
    pub fn for_each_row(&self, mut f: impl FnMut(&[u8])) {
        for p in 0..3 {
            for y in 0..self.plane_size(p).1 {
                f(self.row(p, y));
            }
        }
    }

    /// Convert to RGBA bytes (R, G, B, 255) of `w` x `h`, scaled. `out` must hold `w * h` pixels.
    pub fn to_rgba(&self, out: &mut [u32], w: u32, h: u32) {
        if w == 0 || h == 0 || w > 4096 || h > 4096 || out.len() < (w as usize) * (h as usize) {
            return;
        }
        // SAFETY: the frame's planes are valid (see `row`); `out` holds `w * h` pixels and the C
        // writes `h` rows of `w` pixels, `w` apart.
        unsafe { kmd_i420_to_rgba(&self.raw, out.as_mut_ptr(), w as i32, h as i32, w as i32) }
    }
}

/// The most samples per channel one Opus packet gives (120 ms at 48 kHz).
pub const OPUS_MAX_SAMPLES: usize = 5760;

/// An Opus decoder giving stereo 48 kHz s16.
pub struct Opus {
    raw: NonNull<RawOpus>,
}

impl Opus {
    pub fn new() -> Option<Opus> {
        // SAFETY: no arguments; the result is null or a decoder we now own.
        NonNull::new(unsafe { kmd_opus_new() }).map(|raw| Opus { raw })
    }

    /// Decode one packet into `pcm` (interleaved stereo); returns samples per channel.
    pub fn decode(&mut self, data: &[u8], pcm: &mut [i16]) -> Result<usize, i32> {
        let max = (pcm.len() / 2).min(OPUS_MAX_SAMPLES) as i32;
        // SAFETY: `raw` is live; `data` is valid for its length; `pcm` has room for `max` stereo
        // samples.
        let n = unsafe {
            kmd_opus_decode(
                self.raw.as_ptr(),
                data.as_ptr(),
                data.len(),
                pcm.as_mut_ptr(),
                max,
            )
        };
        if n < 0 { Err(n) } else { Ok(n as usize) }
    }

    pub fn reset(&mut self) {
        // SAFETY: `raw` is our live decoder.
        unsafe { kmd_opus_reset(self.raw.as_ptr()) }
    }

    /// The `OpusHead` output gain, Q7.8 dB.
    pub fn set_gain(&mut self, gain_q8: i16) {
        // SAFETY: `raw` is our live decoder.
        unsafe { kmd_opus_set_gain(self.raw.as_ptr(), i32::from(gain_q8)) }
    }
}

impl Drop for Opus {
    fn drop(&mut self) {
        // SAFETY: we own the decoder and nothing refers to it after this.
        unsafe { kmd_opus_free(self.raw.as_ptr()) }
    }
}
