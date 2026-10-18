//! The kernel threads the supervisor keeps alive (`sched::supervisor`, `docs/design/supervisao.md`): the
//! entry point to run again after a death and the cleanup that makes the shared state of the dead thread
//! safe to start over from.

use crate::sched::Service;

/// Nothing shared to repair (yet).
fn nothing() {}

/// Default stack of a service thread (the scheduler's `STACK_SIZE`).
const STACK: usize = 128 * 1024;

pub static FETCHER: Service = Service {
    name: "fetcher",
    entry: crate::fetch::worker,
    stack: STACK,
    cleanup: nothing,
};

pub static APPD: Service = Service {
    name: "appd",
    entry: crate::wasm::worker,
    stack: 512 * 1024,
    cleanup: nothing,
};

pub static SHELLD: Service = Service {
    name: "shelld",
    entry: crate::desktop::shell_worker,
    stack: STACK,
    cleanup: nothing,
};

pub static SHELLD2: Service = Service {
    name: "shelld2",
    entry: crate::desktop::shell_worker2,
    stack: STACK,
    cleanup: nothing,
};

pub static PROCFSD: Service = Service {
    name: "procfsd",
    entry: crate::userfs::worker,
    stack: STACK,
    cleanup: nothing,
};

pub static AUDIOD: Service = Service {
    name: "audiod",
    entry: crate::audio::worker,
    stack: STACK,
    cleanup: nothing,
};

pub static LOGD: Service = Service {
    name: "logd",
    entry: crate::logd::worker,
    stack: STACK,
    cleanup: nothing,
};
