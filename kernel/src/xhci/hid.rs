//! HID boot keyboards and mice on interrupt IN endpoints: Configure Endpoint, SET_CONFIGURATION, boot
//! protocol, then one outstanding transfer per endpoint whose completion carries a report.

use super::dma::{DEV_INT_RING, DEV_REPORT_BUF, INPUT_CONTEXT, REPORT_BUF_BYTES, Ring};
use super::enumerate::area;
use super::{Error, Xhci};
use crate::klog;
use kitsune_core::hw::hid::{self, BootKeyboard};
use kitsune_core::hw::ps2::Event;
use kitsune_core::hw::usb::{BootHid, BootProtocol};
use kitsune_core::hw::xhci::context as ctx;
use kitsune_core::hw::xhci::trb::{CompletionCode, SetupPacket, Trb};

/// Consecutive failed transfers after which an endpoint is given up (the device is gone or broken).
const MAX_ERRORS: u8 = 8;
/// Input events logged to the serial log, as proof the path works; later ones are silent.
const LOG_BUDGET: u32 = 40;

/// One boot interface being served.
pub(crate) struct HidEp {
    /// Device context index of the interrupt endpoint.
    pub dci: u8,
    pub protocol: BootProtocol,
    pub ring: Ring,
    /// Offset of the report buffer in the DMA block, and how many bytes one transfer asks for.
    pub report_off: usize,
    pub report_len: u16,
    pub keyboard: BootKeyboard,
    pub errors: u8,
    pub active: bool,
}

impl Xhci {
    /// Configures the endpoints of `found` (at most two) and starts the report transfers.
    pub(super) fn setup_hid(&mut self, idx: usize, found: &[BootHid]) -> Result<(), Error> {
        let ctx_size = self.caps.context_size();
        let (slot, port, speed) = match self.devices[idx].as_ref() {
            Some(d) => (d.slot, d.port, d.speed),
            None => return Err(Error::Unsupported("no device")),
        };
        let input = self
            .dma
            .phys(INPUT_CONTEXT)
            .ok_or(Error::Unsupported("DMA address"))?;
        let mut last_dci = 1u8;
        for (i, h) in found.iter().enumerate().take(2) {
            let dci = ctx::endpoint_dci(h.endpoint);
            let ring = Ring::new(&self.dma, area(idx) + DEV_INT_RING + i * 0x400)
                .ok_or(Error::Unsupported("interrupt ring"))?;
            last_dci = last_dci.max(dci);
            self.dma
                .zero(INPUT_CONTEXT, ctx::input_context_bytes(ctx_size));
            self.dma.write_context(
                INPUT_CONTEXT,
                &ctx::input_control(0, ctx::FLAG_SLOT | ctx::flag_endpoint(dci)),
            );
            self.dma.write_context(
                INPUT_CONTEXT + ctx::input_slot_offset(ctx_size),
                &ctx::slot_context(speed, port, 0, last_dci),
            );
            self.dma.write_context(
                INPUT_CONTEXT + ctx::input_endpoint_offset(ctx_size, dci),
                &ctx::interrupt_in_endpoint_context(
                    h.max_packet.max(1),
                    ctx::interrupt_interval(speed, h.interval),
                    ring.ring.pointer(),
                ),
            );
            self.command(Trb::configure_endpoint(input, slot))?;
            let report_len = h.max_packet.clamp(1, REPORT_BUF_BYTES as u16);
            if let Some(dev) = self.devices[idx].as_mut() {
                dev.hid[i] = Some(HidEp {
                    dci,
                    protocol: h.protocol,
                    ring,
                    report_off: area(idx) + DEV_REPORT_BUF + i * REPORT_BUF_BYTES,
                    report_len,
                    keyboard: BootKeyboard::new(),
                    errors: 0,
                    active: false,
                });
            }
        }

        self.control_no_data(idx, SetupPacket::set_configuration(found[0].configuration))?;
        for (i, h) in found.iter().enumerate().take(2) {
            // Boot protocol and "report only on change". Some devices stall one of these; the report
            // format is the boot one either way, so a failure is not fatal.
            for setup in [
                SetupPacket::hid_set_protocol(h.interface, 0),
                SetupPacket::hid_set_idle(h.interface, 0, 0),
            ] {
                if let Err(e) = self.control_no_data(idx, setup) {
                    klog!(
                        Info,
                        "usb: slot {}: HID request {:#x} failed: {:?}",
                        slot,
                        setup.request,
                        e
                    );
                }
            }
            if let Some(ep) = self.devices[idx].as_mut().and_then(|d| d.hid[i].as_mut()) {
                ep.active = true;
            }
            self.queue_report(idx, i);
            klog!(
                Info,
                "usb: slot {}: boot {} on endpoint {:#04x}, interface {}",
                slot,
                match h.protocol {
                    BootProtocol::Keyboard => "keyboard",
                    BootProtocol::Mouse => "mouse",
                },
                h.endpoint,
                h.interface
            );
        }
        Ok(())
    }

    /// Queues one transfer for the report of interface `i` of device `idx` and rings its doorbell.
    fn queue_report(&mut self, idx: usize, i: usize) {
        let Some(dev) = self.devices[idx].as_mut() else {
            return;
        };
        let slot = dev.slot;
        let Some(ep) = dev.hid[i].as_mut() else {
            return;
        };
        let Some(buf) = self.dma.phys(ep.report_off) else {
            return;
        };
        ep.ring.push(&self.dma, Trb::normal(buf, ep.report_len));
        let dci = ep.dci;
        self.ring_doorbell(slot, dci);
    }

    /// An interrupt-endpoint transfer finished: turn the report into input events and queue the next.
    pub(super) fn hid_transfer(
        &mut self,
        slot: u8,
        endpoint: u8,
        code: CompletionCode,
        residual: u32,
    ) {
        let idx = usize::from(slot).wrapping_sub(1);
        let Some(i) = self
            .devices
            .get(idx)
            .and_then(|d| d.as_ref())
            .and_then(|d| {
                d.hid
                    .iter()
                    .position(|e| e.as_ref().is_some_and(|e| e.active && e.dci == endpoint))
            })
        else {
            return;
        };
        let now_ms = crate::interrupts::ticks() * 4;
        let Self {
            devices,
            dma,
            typematic,
            queue,
            logged,
            ..
        } = self;
        let Some(ep) = devices[idx].as_mut().and_then(|d| d.hid[i].as_mut()) else {
            return;
        };
        if code.is_ok() {
            ep.errors = 0;
            let len = usize::from(ep.report_len).saturating_sub(residual as usize);
            let mut report = [0u8; 64];
            let len = len.min(report.len());
            dma.read_bytes(ep.report_off, &mut report[..len]);
            match ep.protocol {
                BootProtocol::Keyboard => {
                    ep.keyboard.report(&report[..len], |k| {
                        typematic.on_key(k, now_ms);
                        if *logged < LOG_BUDGET {
                            *logged += 1;
                            klog!(
                                Info,
                                "usb: key {:#04x}{} {}",
                                k.scan_code,
                                if k.extended { " (ext)" } else { "" },
                                if k.pressed { "down" } else { "up" }
                            );
                        }
                        queue.push(Event::Key(k));
                    });
                }
                BootProtocol::Mouse => {
                    if let Some(p) = hid::mouse_packet(&report[..len]) {
                        if *logged < LOG_BUDGET && (p.dx != 0 || p.dy != 0 || p.left || p.right) {
                            *logged += 1;
                            klog!(
                                Info,
                                "usb: mouse dx={} dy={} left={} right={}",
                                p.dx,
                                p.dy,
                                p.left,
                                p.right
                            );
                        }
                        queue.push(Event::Mouse(p));
                    }
                }
            }
        } else {
            ep.errors += 1;
            if ep.errors >= MAX_ERRORS {
                ep.active = false;
                ep.keyboard.release_all(|k| {
                    typematic.on_key(k, now_ms);
                    queue.push(Event::Key(k));
                });
                klog!(
                    Warn,
                    "usb: slot {} endpoint {:#x}: giving up after {:?}",
                    slot,
                    endpoint,
                    code
                );
                return;
            }
        }
        self.queue_report(idx, i);
    }
}
