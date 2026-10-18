//! System services and state: the log ring (`klog`), the resource monitor (`sysmon`), the
//! traits the system apps use (`sysif`), notifications (`notify`), the process table (`process`),
//! the scheduler and memory models (`schedule`, `paging`, `heap`), randomness (`entropy`, `rng`),
//! user settings (`settings`) and input (`input`, `keymap`, `clipboard`).
//!
//! May depend on: `hw` (the clock), `i18n`, `ui`, `windowing`, `format`. See
//! `docs/design/code-structure.md`.

pub mod clipboard;
pub mod entropy;
pub mod heap;
pub mod input;
pub mod keymap;
pub mod klog;
pub mod notify;
pub mod paging;
pub mod process;
pub mod rng;
pub mod schedule;
pub mod settings;
pub mod supervise;
pub mod sysif;
pub mod sysmon;
