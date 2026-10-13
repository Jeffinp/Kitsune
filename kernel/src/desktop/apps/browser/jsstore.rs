//! `localStorage` of the pages' scripts: kept per origin in `kitsune_core::web::script::WebStorage`
//! and in the file `~/.webstorage` (`/home/<name>/.webstorage`; plain `/home` is not writable by a
//! signed-in account), read when a browser window opens and written at most every few seconds after
//! a change. A page gets its origin's items right after `init` (`store` message) and sends each change
//! (`L`); see `docs/design/tls-browser.md` §14.

use crate::desktop::*;
use kitsune_core::web::script::{StorageOp, WebStorage, origin_of, store_message};

/// Shortest time between two writes of the file (ms).
const SAVE_EVERY_MS: u64 = 3000;

/// The store of a browser window and when it was last written.
pub(crate) struct Store {
    pub data: WebStorage,
    /// The home folder it belongs to (only that person's pages use it).
    owner: Vec<u8>,
    saved_at: u64,
    unsaved: bool,
}

fn path(home: &[u8]) -> Vec<u8> {
    let mut p = home.to_vec();
    p.extend_from_slice(b"/.webstorage");
    p
}

/// Read the signed-in person's store.
pub(super) fn load() -> Store {
    let home = crate::desktop::services::accounts::home();
    let text = crate::desktop::services::vfs::read_file(&path(&home)).unwrap_or_default();
    Store {
        data: WebStorage::from_text(&String::from_utf8_lossy(&text)),
        owner: home,
        saved_at: 0,
        unsaved: false,
    }
}

impl Store {
    /// The `store` message for a page at `url`.
    pub(super) fn message_for(&mut self, url: &str) -> Vec<u8> {
        self.follow_owner();
        let origin = origin_of(url).unwrap_or_default();
        store_message(self.data.items(&origin))
    }

    /// A change from a page at `url`.
    pub(super) fn apply(&mut self, url: &str, op: StorageOp) {
        self.follow_owner();
        let Some(origin) = origin_of(url) else {
            return;
        };
        if !self.data.apply(&origin, op) {
            crate::serial_println!("webstorage: {} is over its quota", origin);
        }
        self.unsaved |= self.data.take_dirty();
    }

    /// Another person signed in: their store, not the last one's.
    fn follow_owner(&mut self) {
        let home = crate::desktop::services::accounts::home();
        if home != self.owner {
            *self = load();
        }
    }

    /// Write the file if something changed (at most every [`SAVE_EVERY_MS`]); called every frame.
    pub(super) fn save_due(&mut self) {
        if !self.unsaved {
            return;
        }
        let now = crate::netd::now_ms();
        if now.saturating_sub(self.saved_at) < SAVE_EVERY_MS {
            return;
        }
        self.saved_at = now;
        self.unsaved = false;
        let text = self.data.to_text();
        match crate::desktop::services::vfs::write_file(&path(&self.owner), text.as_bytes()) {
            Ok(_) => crate::serial_println!("webstorage: saved ({} bytes)", text.len()),
            Err(_) => crate::serial_println!("webstorage: could not write the file"),
        }
    }
}
