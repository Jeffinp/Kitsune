//! Data formats and small utilities: base64, the deflate family (`inflate`, `deflate`, `gzip`),
//! the image codecs (`png`, `bmp`, `ppm`, `gif`, `jpeg`, and `image` over them) and the SVG renderer (`svg`), ZIP archives (`zip`), ELF executables for user processes (`elf`), text encodings (`text`), Unix time and
//! calendar maths (`unixtime`), the substring matcher (`search`), the WebM demuxer (`webm`) and
//! MD5 (`md5`, for checking decoded media against references).
//!
//! May depend on: nothing else in the crate (this is the leaf layer). See
//! `docs/design/code-structure.md`.

pub mod base64;
pub mod bmp;
pub mod deflate;
pub mod elf;
pub mod gif;
pub mod gzip;
pub mod image;
pub mod inflate;
pub mod jpeg;
pub mod md5;
pub mod png;
pub mod ppm;
pub mod search;
pub mod svg;
pub mod text;
pub mod unixtime;
pub mod webm;
pub mod zip;
