//! The media player's logic: `<video>`/`<audio>` and Media Source Extensions, minus the pixels and
//! the hardware.
//!
//! Three processes play a video. The page's script process (`jsrun --page`, `media.js`) has the
//! JavaScript API (`HTMLMediaElement`, `MediaSource`, `SourceBuffer`) and fetches the bytes; the
//! browser (the compositor, kernel) runs the [`player::Player`] of each element: its clock (the sound
//! is the master), the picture on screen, the sound fed to the mixer and the state the page sees; the
//! decoder (`mediadec --serve`, one process per element) keeps the coded frames
//! ([`track::TrackBuffer`]) and decodes ahead within the budget the player gives.
//!
//! * [`ranges`] `TimeRanges` (`buffered`, `played`, `seekable`)
//! * [`track`] the coded frames of a track: buffered ranges, `remove`, eviction, where to decode from
//! * [`proto`] the messages between the player and the decoder
//! * [`clock`] the player's clock and the sound device as the player sees it ([`clock::AudioSink`])
//! * [`player`] the player and the page-visible state machine with its events
//! * [`wire`] the messages between the page's script and the browser
//! * [`pacing`] when the pictures reach the screen (a smooth clock) and how evenly (statistics)
//! * [`controls`] where the browser-drawn controls sit and what a click on them means
//!
//! See `docs/design/midia.md` and `docs/design/tls-browser.md` §14.

pub mod clock;
pub mod controls;
pub mod pacing;
pub mod player;
pub mod proto;
pub mod ranges;
pub mod track;
pub mod wire;

#[cfg(test)]
mod tests;
