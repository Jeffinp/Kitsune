//! A `src` that is an address (not a MediaSource): the browser fetches it, a piece of 1 MiB at a time
//! with `Range`, never more than 30 s ahead of the playback position, through the fetcher's queue of
//! script requests but straight into the decoder (`ScriptFetch::media`): the page's script never
//! reads the bytes, so a video of another origin plays without CORS, as in every browser. A server
//! that does not answer ranges sends the whole file in one answer.

use super::*;
use kitsune_core::cors::{Credentials, Mode};
use kitsune_core::web::script::{FetchRequest, Redirect};

/// Bytes asked for at a time.
const PIECE: u64 = 1 << 20;
/// Not more than this far ahead of the playback position.
const AHEAD_NS: i64 = 30_000_000_000;

/// The download of a `src`.
pub(crate) struct Direct {
    url: String,
    /// Where the next request starts.
    offset: u64,
    /// The file's size, once known.
    total: Option<u64>,
    /// The request on its way (its id).
    inflight: Option<u32>,
    next_id: u32,
    /// Everything is in.
    pub done: bool,
    failed: bool,
    /// The server answers ranges (else the first answer is the whole file).
    ranged: bool,
    /// What source 0 holds (the decoder says after each piece).
    pub ranges: Vec<(i64, i64)>,
    started_ms: u64,
}

/// The page a player belongs to, for its requests.
pub(crate) struct NetCtx<'a> {
    pub page_url: String,
    pub shield: &'a mut kitsune_core::shields::PageShield,
    pub insecure_host: Vec<u8>,
}

impl Direct {
    pub(super) fn new(url: String) -> Direct {
        Direct {
            url,
            offset: 0,
            total: None,
            inflight: None,
            next_id: 1,
            done: false,
            failed: false,
            ranged: true,
            ranges: Vec::new(),
            started_ms: now_ms(),
        }
    }
}

impl MediaPlayer {
    /// Ask for the next piece when the player needs it.
    pub(super) fn pump_direct(&mut self, net: &mut NetCtx) {
        let pos = self.player.position();
        let pid = self.pid;
        let Some(d) = self.direct.as_mut() else {
            return;
        };
        if d.done || d.failed || d.inflight.is_some() {
            return;
        }
        let ahead = d
            .ranges
            .iter()
            .find(|r| r.0 <= pos + 500_000_000 && r.1 > pos)
            .map_or(0, |r| r.1 - pos);
        if ahead > AHEAD_NS {
            return;
        }
        let n0 = net.shield.blocked().len();
        if net.shield.check(d.url.as_bytes()).is_some() {
            super::super::shield::log_new(net.shield, n0, "media");
            d.failed = true;
            self.player.fail(MEDIA_ERR_NETWORK);
            return;
        }
        let id = d.next_id;
        d.next_id += 1;
        let end = d.offset + PIECE - 1;
        let req = FetchRequest {
            id,
            method: String::from("GET"),
            url: d.url.clone(),
            mode: Mode::NoCors,
            credentials: Credentials::Include,
            redirect: Redirect::Follow,
            headers: alloc::vec![(
                String::from("Range"),
                alloc::format!("bytes={}-{end}", d.offset)
            )],
            body: None,
        };
        let job = crate::fetch::ScriptFetch {
            pid,
            req,
            page_url: net.page_url.clone(),
            shield_on: net.shield.is_on(),
            insecure_host: net.insecure_host.clone(),
            media: true,
        };
        if crate::fetch::post_script_fetch(job) {
            d.inflight = Some(id);
        } else {
            d.failed = true;
            self.player.fail(MEDIA_ERR_NETWORK);
        }
    }

    /// The decoder says how a request went.
    pub(super) fn fetched(&mut self, id: u32, ev: u8, status: u32, total: Option<u64>, bytes: u64) {
        let uid = self.uid;
        let Some(d) = self.direct.as_mut() else {
            return;
        };
        if d.inflight != Some(id) {
            return;
        }
        match ev {
            proto::FETCH_HEAD => match status {
                206 => d.total = total.or(d.total),
                200 if d.offset == 0 => {
                    // No ranges: the whole file comes in this answer.
                    d.ranged = false;
                    d.total = total;
                }
                _ => {
                    crate::klog!(Info, "media: element {uid}: {} answered {status}", d.url);
                    d.failed = true;
                    d.inflight = None;
                    crate::fetch::abort_script_fetch(self.pid, id);
                    self.player.fail(MEDIA_ERR_NETWORK);
                }
            },
            proto::FETCH_END => {
                d.inflight = None;
                d.offset += bytes;
                let all = !d.ranged || bytes == 0 || d.total.is_some_and(|t| d.offset >= t);
                if all {
                    d.done = true;
                    let ms = now_ms().saturating_sub(d.started_ms).max(1);
                    crate::klog!(
                        Info,
                        "media: element {uid}: {} bytes of {} in {ms} ms ({} KB/s)",
                        d.offset,
                        d.url,
                        d.offset * 1000 / 1024 / ms
                    );
                    self.player.set_eos(true);
                    self.post(proto::msg(proto::IN_EOS, &[Field::U8(1)]));
                }
            }
            _ => {
                crate::klog!(
                    Info,
                    "media: element {uid}: the download of {} failed",
                    d.url
                );
                d.failed = true;
                d.inflight = None;
                self.player.fail(MEDIA_ERR_NETWORK);
            }
        }
    }
}
