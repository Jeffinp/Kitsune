//! Starting, reading and stopping child programs (`run` in the shell): the calls 52–54.

use alloc::vec::Vec;
use kitsune_core::shell::{ProgramCtx, ProgramEnd, ProgramPoll, SpawnFail};
use kitsune_user::abi::{self, ChildState, SpawnReq, errno};
use kitsune_user::syscall3;

/// Start `name` (an embedded program) or the ELF `image` as a child.
pub fn spawn(name: &str, image: Option<&[u8]>, ctx: &ProgramCtx) -> Result<u32, SpawnFail> {
    let args = abi::pack_args(&ctx.cwd, &ctx.args).ok_or(SpawnFail::BadArgs)?;
    let wire = abi::pack_spawn(&SpawnReq {
        name,
        args: &args,
        image: image.unwrap_or(&[]),
        stdin: &ctx.stdin,
    })
    .ok_or(SpawnFail::BadArgs)?;
    // SAFETY: the pointer and length describe `wire`.
    let r = unsafe { syscall3(abi::SYS_SPAWN, wire.as_ptr() as u64, wire.len() as u64, 0) };
    match r {
        pid if pid >= 0 => Ok(pid as u32),
        errno::ENOEXEC => Err(SpawnFail::BadImage),
        errno::ENOMEM => Err(SpawnFail::NoMemory),
        errno::EAGAIN => Err(SpawnFail::TooMany),
        errno::EINVAL => Err(SpawnFail::BadArgs),
        _ => Err(SpawnFail::Unknown),
    }
}

/// What child `pid` wrote since the last call, and whether it is over.
pub fn poll(pid: u32) -> ProgramPoll {
    let mut buf: Vec<u8> = alloc::vec![0u8; abi::MAX_IO];
    // SAFETY: the pointer and length describe `buf`.
    let r = unsafe {
        syscall3(
            abi::SYS_PROC_POLL,
            u64::from(pid),
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    };
    if r < 0 {
        // Not ours (any more): treat it as gone.
        return ProgramPoll {
            output: Vec::new(),
            end: Some(ProgramEnd::Killed),
        };
    }
    let (n, state) = abi::unpack_poll(r);
    buf.truncate(n);
    ProgramPoll {
        output: buf,
        end: match state {
            ChildState::Running => None,
            ChildState::Exited(c) => Some(ProgramEnd::Exited(i32::from(c))),
            ChildState::Killed => Some(ProgramEnd::Killed),
        },
    }
}

/// Stop child `pid`.
pub fn kill(pid: u32) {
    // SAFETY: one plain argument.
    unsafe { syscall3(abi::SYS_PROC_KILL, u64::from(pid), 0, 0) };
}
