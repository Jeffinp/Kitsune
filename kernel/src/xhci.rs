//! USB xHCI host controller: keyboard and mouse over the HID boot protocol, polled (no interrupt
//! line: there is no APIC/MSI yet, and the compositor loop already polls the PS/2 ring).
//!
//! The register, ring and context arithmetic is `kitsune_core::hw::xhci`; descriptors and HID reports
//! are `kitsune_core::hw::{usb, hid}`. This module is the glue: it owns the DMA memory, talks to the
//! registers and runs the sequences of the specification.
//!
//! - this file: bring-up (firmware handoff, reset, command and event rings), commands and events;
//! - `enumerate`: ports, Enable Slot, Address Device, control transfers, descriptors;
//! - `hid`: Configure Endpoint, SET_CONFIGURATION, boot protocol, and the reports that come back.
//!
//! Plan and limits (one controller, four devices, no hubs, no hot-plug): `docs/design/usb-xhci.md`.

mod dma;
mod enumerate;
mod hid;
mod mmio;

use crate::sync::RacyCell;
use crate::{klog, pci};
use dma::{Dma, MAX_DEVICES, Ring};
use enumerate::Device;
use kitsune_core::hw::hid::Typematic;
use kitsune_core::hw::ps2::Event;
use kitsune_core::hw::xhci::ring::EventRing;
use kitsune_core::hw::xhci::trb::{CompletionCode, Event as Xevent, Trb};
use kitsune_core::hw::xhci::{self as hw, Capabilities};
use mmio::Mmio;

/// Timer ticks are 4 ms (250 Hz).
const fn ticks_of_ms(ms: u64) -> u64 {
    ms.div_ceil(4)
}

const COMMAND_TIMEOUT: u64 = ticks_of_ms(1000);
const RESET_TIMEOUT: u64 = ticks_of_ms(1000);
/// Typematic: 500 ms before the first repeat, then one every 33 ms (~30 keys/s).
const REPEAT_DELAY_MS: u64 = 500;
const REPEAT_INTERVAL_MS: u64 = 33;
/// Largest BAR the driver maps: xHCI windows are a few tens of KiB.
const MAX_WINDOW: u64 = 1 << 20;

/// Why a step failed. Only the `Debug` text is used, in the log lines.
#[derive(Debug)]
#[allow(
    dead_code,
    reason = "the fields are read by the Debug impl the log uses"
)]
pub(crate) enum Error {
    Timeout(&'static str),
    Command(CompletionCode),
    Transfer(CompletionCode),
    Unsupported(&'static str),
}

pub(crate) struct CmdDone {
    pub trb: u64,
    pub code: CompletionCode,
    pub slot: u8,
}

pub(crate) struct XferDone {
    pub trb: u64,
    pub code: CompletionCode,
}

const QUEUE_LEN: usize = 64;

/// Decoded input waiting for the desktop. When full the newest event is dropped.
struct Queue {
    buf: [Option<Event>; QUEUE_LEN],
    head: usize,
    len: usize,
}

impl Queue {
    const fn new() -> Self {
        Self {
            buf: [None; QUEUE_LEN],
            head: 0,
            len: 0,
        }
    }

    fn push(&mut self, e: Event) {
        if self.len < QUEUE_LEN {
            self.buf[(self.head + self.len) % QUEUE_LEN] = Some(e);
            self.len += 1;
        }
    }

    fn pop(&mut self) -> Option<Event> {
        if self.len == 0 {
            return None;
        }
        let e = self.buf[self.head].take();
        self.head = (self.head + 1) % QUEUE_LEN;
        self.len -= 1;
        e
    }
}

pub(crate) struct Xhci {
    pub mmio: Mmio,
    pub caps: Capabilities,
    pub dma: Dma,
    pub cmd: Ring,
    pub event: EventRing,
    pub devices: [Option<Device>; MAX_DEVICES],
    pub cmd_done: Option<CmdDone>,
    pub xfer_done: Option<XferDone>,
    queue: Queue,
    pub typematic: Typematic,
    /// Input events logged so far (the first few go to the serial log as proof the path works).
    pub logged: u32,
}

/// The one controller. Created by [`init`] on the boot thread and used only from the compositor loop
/// (the same thread), so there is never a second reference.
static STATE: RacyCell<Option<Xhci>> = RacyCell::new(None);

fn wait_for(ticks: u64, mut ok: impl FnMut() -> bool) -> bool {
    let end = crate::interrupts::ticks() + ticks;
    while crate::interrupts::ticks() <= end {
        if ok() {
            return true;
        }
        core::hint::spin_loop();
    }
    ok()
}

/// Whether `[off, off + size)` lies inside a window of `len` bytes.
fn inside(len: u32, off: u32, size: u32) -> bool {
    off.checked_add(size).is_some_and(|end| end <= len)
}

/// Finds the controller, starts it and enumerates what is plugged in. Logs why when it cannot.
pub fn init(phys_offset: Option<u64>) {
    let Some(phys_offset) = phys_offset else {
        klog!(Info, "usb: no physical-memory map, xHCI skipped");
        return;
    };
    let Some(dev) = pci::find_xhci() else {
        klog!(Info, "usb: no xHCI controller");
        return;
    };
    match start(&dev, phys_offset) {
        Ok(mut x) => {
            x.enumerate_ports();
            // SAFETY: `init` runs once on the boot thread before the compositor loop starts, so nothing
            // else holds a reference to STATE.
            unsafe { *STATE.get() = Some(x) };
        }
        Err(e) => klog!(Warn, "usb: xHCI not started: {:?}", e),
    }
}

/// Next input event from a USB keyboard or mouse (the same events the PS/2 decoder produces), or
/// `None`. Call it from the compositor loop; it also advances the key repeat.
pub fn poll() -> Option<Event> {
    // SAFETY: STATE is only touched by `init` (before this runs) and by this function on the
    // compositor thread, one call at a time.
    let state = unsafe { &mut *STATE.get() };
    state.as_mut()?.poll()
}

fn start(dev: &pci::PciDevice, phys_offset: u64) -> Result<Xhci, Error> {
    klog!(
        Info,
        "usb: xHCI {:04x}:{:04x} at pci {:02x}:{:02x}.{}",
        dev.vendor,
        dev.device,
        dev.bus,
        dev.slot,
        dev.func
    );
    let len = dev
        .probe_bar_size(0)
        .ok_or(Error::Unsupported("BAR 0 is not a memory window"))?;
    if !(0x1000..=MAX_WINDOW).contains(&len) {
        return Err(Error::Unsupported("register window size"));
    }
    let phys = crate::virtio::bar_base(dev, 0).ok_or(Error::Unsupported("BAR 0 unassigned"))?;
    let virt = phys
        .checked_add(phys_offset)
        .ok_or(Error::Unsupported("BAR 0 address"))?;
    let mapped = |a: u64| crate::virtio::virt_to_phys(a, phys_offset).is_some();
    if !mapped(virt) || !mapped(virt + len - 1) {
        return Err(Error::Unsupported("register window not mapped"));
    }
    dev.enable_bus_master();
    let mmio = Mmio::new(virt, len as u32);

    let caps = Capabilities::decode(
        mmio.r32(hw::CAP_LENGTH_VERSION as u32),
        mmio.r32(hw::CAP_HCSPARAMS1 as u32),
        mmio.r32(hw::CAP_HCSPARAMS2 as u32),
        mmio.r32(hw::CAP_HCCPARAMS1 as u32),
        mmio.r32(hw::CAP_DBOFF as u32),
        mmio.r32(hw::CAP_RTSOFF as u32),
    )
    .ok_or(Error::Unsupported("capability registers"))?;
    klog!(
        Info,
        "usb: xHCI version {:x}.{:02x}, {} slots, {} ports, {}-byte contexts, {} scratchpad pages",
        caps.version >> 8,
        caps.version & 0xFF,
        caps.max_slots,
        caps.max_ports,
        caps.context_size(),
        caps.scratchpad_buffers
    );
    let slots = caps.max_slots.min(MAX_DEVICES as u8);
    let op = u32::from(caps.cap_length);
    if !inside(len as u32, caps.doorbell(slots), 4)
        || !inside(len as u32, caps.interrupter(0) + hw::INTR_ERDP, 8)
        || !inside(len as u32, caps.portsc(caps.max_ports), 4)
        || !inside(len as u32, op + hw::OP_CONFIG, 4)
    {
        return Err(Error::Unsupported("register offsets outside the window"));
    }
    if mmio.r32(op + 0x08) & 1 == 0 {
        return Err(Error::Unsupported(
            "controller does not support 4 KiB pages",
        ));
    }
    if usize::from(caps.scratchpad_buffers) > dma::SCRATCH_PAGES {
        return Err(Error::Unsupported("too many scratchpad pages"));
    }

    take_from_firmware(&mmio, &caps);
    reset(&mmio, op)?;

    let dma = Dma::take(phys_offset).ok_or(Error::Unsupported("DMA block already in use"))?;
    let phys_of = |off| {
        dma.phys(off)
            .ok_or(Error::Unsupported("DMA physical address"))
    };
    let dcbaa = phys_of(dma::DCBAA)?;
    let erst = phys_of(dma::ERST)?;
    if !caps.addresses_64 && [dcbaa, erst].iter().any(|&p| p > u64::from(u32::MAX)) {
        return Err(Error::Unsupported(
            "DMA memory above 4 GiB, controller is 32-bit",
        ));
    }

    // Scratchpad: an array of page addresses that entry 0 of the DCBAA points at.
    dma.zero(dma::DCBAA, 2048);
    if caps.scratchpad_buffers > 0 {
        for i in 0..usize::from(caps.scratchpad_buffers) {
            let page = dma
                .scratch_phys(i)
                .ok_or(Error::Unsupported("scratchpad page"))?;
            dma.w64(dma::SCRATCH_ARRAY + i * 8, page);
        }
        dma.w64(dma::DCBAA, phys_of(dma::SCRATCH_ARRAY)?);
    }

    let cmd = Ring::new(&dma, dma::CMD_RING).ok_or(Error::Unsupported("command ring"))?;
    dma.zero(dma::EVENT_RING, dma::RING_BYTES);
    let event = EventRing::new(phys_of(dma::EVENT_RING)?, dma::RING_TRBS)
        .ok_or(Error::Unsupported("event ring"))?;
    for (i, d) in hw::erst_entry(phys_of(dma::EVENT_RING)?, dma::RING_TRBS)
        .iter()
        .enumerate()
    {
        dma.w32(dma::ERST + i * 4, *d);
    }

    mmio.w32(op + hw::OP_CONFIG, u32::from(slots));
    mmio.w64(op + hw::OP_DCBAAP, dcbaa);
    mmio.w64(op + hw::OP_CRCR, cmd.ring.pointer());
    let intr = caps.interrupter(0);
    mmio.w32(intr + hw::INTR_ERSTSZ, 1);
    mmio.w64(intr + hw::INTR_ERDP, event.pointer());
    mmio.w64(intr + hw::INTR_ERSTBA, erst);
    // Run, with interrupts left off: events are collected by polling the event ring.
    mmio.w32(op + hw::OP_USBCMD, hw::USBCMD_RUN);
    if !wait_for(RESET_TIMEOUT, || {
        mmio.r32(op + hw::OP_USBSTS) & hw::USBSTS_HCH == 0
    }) {
        return Err(Error::Timeout("controller did not start"));
    }

    let mut x = Xhci {
        mmio,
        caps,
        dma,
        cmd,
        event,
        devices: [const { None }; MAX_DEVICES],
        cmd_done: None,
        xfer_done: None,
        queue: Queue::new(),
        typematic: Typematic::new(REPEAT_DELAY_MS, REPEAT_INTERVAL_MS),
        logged: 0,
    };
    x.command(Trb::no_op_command())?;
    klog!(Info, "usb: xHCI running, command ring answers (no-op ok)");
    Ok(x)
}

/// Asks the firmware for the controller (USB legacy support) and silences its SMIs. Not provable in
/// QEMU, which has no such capability; follows the specification's handoff and Linux's mask.
fn take_from_firmware(mmio: &Mmio, caps: &Capabilities) {
    let Some(off) = hw::find_ext_cap(caps.ext_caps_offset, hw::EXT_CAP_LEGACY_SUPPORT, |o| {
        mmio.r32(o)
    }) else {
        return;
    };
    let v = mmio.r32(off);
    if v & (1 << 16) != 0 {
        mmio.w32(off, hw::legacy_request_os_ownership(v));
        if !wait_for(ticks_of_ms(1000), || hw::legacy_handoff_done(mmio.r32(off))) {
            klog!(Warn, "usb: firmware did not release the controller in 1 s");
        }
    }
    let ctl = mmio.r32(off + 4);
    mmio.w32(off + 4, hw::legacy_disable_smi(ctl));
}

/// Halt (if running) and reset; waits for the controller to be ready again.
fn reset(mmio: &Mmio, op: u32) -> Result<(), Error> {
    let cmd = mmio.r32(op + hw::OP_USBCMD);
    if cmd & hw::USBCMD_RUN != 0 {
        mmio.w32(op + hw::OP_USBCMD, cmd & !hw::USBCMD_RUN);
    }
    if !wait_for(RESET_TIMEOUT, || {
        mmio.r32(op + hw::OP_USBSTS) & hw::USBSTS_HCH != 0
    }) {
        return Err(Error::Timeout("controller did not halt"));
    }
    mmio.w32(op + hw::OP_USBCMD, hw::USBCMD_HCRST);
    let done = wait_for(RESET_TIMEOUT, || {
        mmio.r32(op + hw::OP_USBCMD) & hw::USBCMD_HCRST == 0
            && mmio.r32(op + hw::OP_USBSTS) & hw::USBSTS_CNR == 0
    });
    if done {
        Ok(())
    } else {
        Err(Error::Timeout("controller reset"))
    }
}

impl Xhci {
    /// Rings doorbell `slot` with `target` (0 for the command ring; the endpoint index for a slot).
    pub fn ring_doorbell(&self, slot: u8, target: u8) {
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        self.mmio.w32(self.caps.doorbell(slot), u32::from(target));
    }

    /// Sends a command and waits for its completion event. Only `Success` is accepted.
    pub fn command(&mut self, trb: Trb) -> Result<CmdDone, Error> {
        self.cmd_done = None;
        let addr = self.cmd.push(&self.dma, trb);
        self.ring_doorbell(0, 0);
        let end = crate::interrupts::ticks() + COMMAND_TIMEOUT;
        loop {
            self.pump();
            if let Some(done) = self.cmd_done.take_if(|d| d.trb == addr) {
                return if done.code == CompletionCode::Success {
                    Ok(done)
                } else {
                    Err(Error::Command(done.code))
                };
            }
            if crate::interrupts::ticks() > end {
                return Err(Error::Timeout("command"));
            }
            core::hint::spin_loop();
        }
    }

    /// Drains the event ring: stores command and control-transfer results for whoever waits, hands
    /// interrupt-endpoint completions to the HID code.
    pub fn pump(&mut self) {
        let mut consumed = false;
        loop {
            let off = dma::EVENT_RING + usize::from(self.event.dequeue_index()) * 16;
            let trb = self.dma.read_trb(off);
            if !self.event.is_new(&trb) {
                break;
            }
            self.event.advance();
            consumed = true;
            match Xevent::decode(&trb) {
                Xevent::CommandCompletion {
                    command_trb,
                    code,
                    slot,
                } => {
                    self.cmd_done = Some(CmdDone {
                        trb: command_trb,
                        code,
                        slot,
                    })
                }
                Xevent::Transfer {
                    trb,
                    code,
                    slot,
                    endpoint,
                    residual,
                } => {
                    if endpoint == 1 {
                        self.xfer_done = Some(XferDone { trb, code });
                    } else {
                        self.hid_transfer(slot, endpoint, code, residual);
                    }
                }
                Xevent::PortStatusChange { .. } | Xevent::Other { .. } => {}
            }
        }
        if consumed {
            let intr = self.caps.interrupter(0);
            self.mmio
                .w64(intr + hw::INTR_ERDP, self.event.pointer() | hw::ERDP_EHB);
        }
    }

    fn poll(&mut self) -> Option<Event> {
        self.pump();
        let now_ms = crate::interrupts::ticks() * 4;
        if let Some(k) = self.typematic.poll(now_ms) {
            self.queue.push(Event::Key(k));
        }
        self.queue.pop()
    }
}
