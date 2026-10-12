//! What the host functions need from the guest: its memory (bounds-checked), fuel and state, and the wasmi-backed implementation.

use super::*;

/// Where a host call runs: the guest's memory, its fuel and the host state, wherever the guest
/// executes. In the kernel it is a wasmi `Caller`; for an app in a user process it is that process's
/// page tables (`wasm::remote`). The functions below are written once against this trait.
pub(crate) trait Guest {
    fn host(&self) -> &HostState;
    fn host_mut(&mut self) -> &mut HostState;
    /// Bytes of linear memory the guest has right now.
    fn mem_size(&self) -> usize;
    /// Copy out a range already checked against [`Guest::mem_size`].
    fn read_mem(&self, r: Range<usize>) -> Vec<u8>;
    /// Copy into a range already checked against [`Guest::mem_size`].
    fn write_mem(&mut self, at: usize, bytes: &[u8]);
    /// Charge `units` of fuel for host work; the `OutOfFuel` trap when the budget is spent.
    fn charge(&mut self, units: u64) -> R<()>;
}

pub(super) fn fault(what: &'static str) -> wasmi::Error {
    wasmi::Error::host(AppFault(what))
}

pub(super) fn mono_ms() -> u64 {
    interrupts::ticks() * 4
}

pub(super) fn v2(g: &mut dyn Guest) -> R<&mut V2> {
    g.host_mut()
        .v2
        .as_deref_mut()
        .ok_or_else(|| fault(kitsune_core::tk!("apps.why.no_abi2")))
}

/// Validated range. A bad range is a fault (the app dies).
pub(super) fn range(g: &dyn Guest, ptr: i32, len: i32) -> R<Range<usize>> {
    check_range(g.mem_size(), ptr, len)
        .map_err(|_| fault(kitsune_core::tk!("apps.why.bad_pointer")))
}

/// Copy `[ptr, ptr+len)` out of guest memory. `Ok(Err(code))` when `len > cap`.
pub(super) fn read_capped(
    g: &dyn Guest,
    ptr: i32,
    len: i32,
    cap: usize,
) -> R<Result<Vec<u8>, i32>> {
    let r = check_capped(g.mem_size(), ptr, len, cap)
        .map_err(|_| fault(kitsune_core::tk!("apps.why.bad_pointer")))?;
    Ok(r.map(|r| g.read_mem(r)))
}

pub(super) fn write_guest(g: &mut dyn Guest, ptr: i32, bytes: &[u8]) -> R<()> {
    let r = range(g, ptr, bytes.len() as i32)?;
    g.write_mem(r.start, bytes);
    Ok(())
}

/// The wasmi side: a `Caller` is a [`Guest`].
pub(super) struct Local<'a, 'b>(
    pub(super) Caller<'a, HostState>,
    pub(super) core::marker::PhantomData<&'b ()>,
);

impl Guest for Local<'_, '_> {
    fn host(&self) -> &HostState {
        self.0.data()
    }
    fn host_mut(&mut self) -> &mut HostState {
        self.0.data_mut()
    }
    fn mem_size(&self) -> usize {
        guest_mem(&self.0).map_or(0, |m| m.data_size(&self.0))
    }
    fn read_mem(&self, r: Range<usize>) -> Vec<u8> {
        guest_mem(&self.0).map_or_else(Vec::new, |m| m.data(&self.0)[r].to_vec())
    }
    fn write_mem(&mut self, at: usize, bytes: &[u8]) {
        if let Some(m) = guest_mem(&self.0) {
            m.data_mut(&mut self.0)[at..at + bytes.len()].copy_from_slice(bytes);
        }
    }
    fn charge(&mut self, units: u64) -> R<()> {
        super::super::charge(&mut self.0, units)
    }
}
