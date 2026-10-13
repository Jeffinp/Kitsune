//! Clicks on a video: on the browser-drawn controls they play, pause, seek, mute or set the
//! volume; anywhere else on a `<video controls>` they play or pause. The page hears what changed
//! through the player's events (`play`, `pause`, `seeking`, `volumechange`...).

use super::*;
use kitsune_core::media::controls::{Part, layout};
use kitsune_core::media::player::Event;

impl MediaSet {
    /// A click at `(x, y)` inside the box `b` (page coordinates) of a media element with controls.
    /// Returns whether the click was the controls' (the page does not see it).
    pub(crate) fn click(
        &mut self,
        js: u32,
        b: &kitsune_core::web::VideoBox,
        x: i32,
        y: i32,
        zoom: i32,
    ) -> bool {
        if !b.controls {
            return false;
        }
        let l = layout(b.w, b.h, zoom);
        let part = l.hit(x - b.x, y - b.y);
        let Some(p) = self.find(b.uid) else {
            // Not playing yet (the page has not loaded it): the click is the page's.
            return part.is_some();
        };
        let toggle = match part {
            Some(Part::Play) => true,
            // A click on the picture itself.
            None => !b.audio,
            _ => false,
        };
        if toggle {
            if p.player.state().paused {
                p.player.play();
                p.started = true;
            } else {
                p.player.pause();
            }
            return true;
        }
        match part {
            Some(Part::Seek) => {
                if let Some(d) = p.player.state().duration {
                    let t = l.seek_time(x - b.x, d);
                    p.player.seek(t);
                }
            }
            Some(Part::Mute) => {
                let s = p.player.state();
                let (v, m) = (s.volume, !s.muted);
                p.player.set_volume(v, m);
                volume_changed(p, js);
            }
            Some(Part::Volume) => {
                let v = l.volume_at(x - b.x);
                p.player.set_volume(v, false);
                volume_changed(p, js);
            }
            Some(Part::Bar | Part::Play) | None => {}
        }
        true
    }
}

/// The volume changed from the controls: the page's `volume`/`muted` follow (`volumechange`).
fn volume_changed(p: &MediaPlayer, js: u32) {
    let m = wire::state_message(
        p.uid,
        p.player.state(),
        p.player.position(),
        &[Event::VolumeChange],
    );
    crate::userproc::page_post(js, m);
}
