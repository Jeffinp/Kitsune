//! Accounts, passwords and access control: who the users are, how a password is stored and checked,
//! what a user may do to a file, and how failed logins are slowed down.
//!
//! Everything here is pure (no clock, no randomness, no disk): the caller passes the time, the salt
//! and the bytes in. That keeps it testable on the host and lets the kernel decide where the
//! account database lives (`/etc/passwd` on the OJFS volume) and where the salt comes from
//! (`kitsune_core::entropy`).
//!
//! May depend on: nothing. See `docs/design/usuarios-seguranca.md` for the design and, in
//! `docs/SECURITY-MODEL.md`, for what this does **not** protect (while everything still runs in
//! ring 0, a bug in the kernel ignores these checks).

pub mod account;
pub mod password;
pub mod perm;
pub mod secret;
pub mod session;
