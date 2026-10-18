//! From a connected port to an addressed device whose descriptors are read: port power and reset,
//! Enable Slot, Address Device, control transfers on the default endpoint, GET_DESCRIPTOR.

use super::dma::{
    self, CONTROL_BUF_BYTES, DCBAA, DEV_CONTROL_BUF, DEV_EP0_RING, DEVICE_CONTEXT_STRIDE,
    DEVICE_CONTEXTS, DEVICE_STRIDE, DEVICES, INPUT_CONTEXT, Ring,
};
use super::hid::HidEp;
use super::{Error, Xhci, ticks_of_ms, wait_for};
use crate::klog;
use kitsune_core::hw::usb::{self, BootHid, MAX_CONFIG_LEN};
use kitsune_core::hw::xhci::context as ctx;
use kitsune_core::hw::xhci::trb::{
    DESCRIPTOR_CONFIGURATION, DESCRIPTOR_DEVICE, SetupPacket, Trb, control_transfer,
};
use kitsune_core::hw::xhci::{self as hw, Speed};

const POWER_SETTLE: u64 = ticks_of_ms(100);
const RESET_WAIT: u64 = ticks_of_ms(500);
const TRANSFER_TIMEOUT: u64 = ticks_of_ms(1000);

/// An addressed device. Four at most, indexed by slot id - 1.
pub(crate) struct Device {
    pub slot: u8,
    pub port: u8,
    pub speed: Speed,
    pub ep0: Ring,
    /// Boot HID interfaces being served (at most two per device).
    pub hid: [Option<HidEp>; 2],
}

/// Offset of device area `idx` in the DMA block.
pub(super) const fn area(idx: usize) -> usize {
    DEVICES + idx * DEVICE_STRIDE
}

impl Xhci {
    /// Brings up every port that has something connected. Logs one line per port that matters.
    pub fn enumerate_ports(&mut self) {
        let mut configured = 0;
        for port in 1..=self.caps.max_ports {
            match self.bring_up_port(port) {
                Ok(true) => configured += 1,
                Ok(false) => {}
                Err(e) => klog!(Warn, "usb: port {}: {:?}", port, e),
            }
        }
        klog!(Info, "usb: {} keyboard/mouse device(s) running", configured);
    }

    fn portsc(&self, port: u8) -> u32 {
        self.mmio.r32(self.caps.portsc(port))
    }

    /// `Ok(true)` when a keyboard or mouse on `port` is now delivering reports.
    fn bring_up_port(&mut self, port: u8) -> Result<bool, Error> {
        let reg = self.caps.portsc(port);
        let mut sc = self.portsc(port);
        if sc == u32::MAX {
            return Ok(false);
        }
        if sc & hw::PORTSC_PP == 0 {
            self.mmio.w32(reg, hw::portsc_neutral(sc) | hw::PORTSC_PP);
            wait_for(POWER_SETTLE, || false);
            sc = self.portsc(port);
        }
        if sc & hw::PORTSC_CCS == 0 {
            return Ok(false);
        }
        self.mmio.w32(reg, hw::portsc_ack(sc, hw::PORTSC_CHANGES));
        sc = self.portsc(port);
        if sc & hw::PORTSC_PED == 0 {
            // USB 2 devices are enabled by a port reset; SuperSpeed ones come up enabled by themselves.
            self.mmio.w32(reg, hw::portsc_reset(sc));
            if !wait_for(RESET_WAIT, || self.portsc(port) & hw::PORTSC_PRC != 0) {
                return Err(Error::Timeout("port reset"));
            }
            sc = self.portsc(port);
            self.mmio.w32(reg, hw::portsc_ack(sc, hw::PORTSC_PRC));
            sc = self.portsc(port);
            if sc & hw::PORTSC_PED == 0 {
                return Err(Error::Unsupported("port did not enable after reset"));
            }
        }
        let speed = Speed::from_portsc(sc).ok_or(Error::Unsupported("unknown port speed"))?;
        self.setup_device(port, speed)
    }

    /// Enable Slot, Address Device, read the descriptors, and hand boot HID interfaces to `hid`.
    fn setup_device(&mut self, port: u8, speed: Speed) -> Result<bool, Error> {
        let slot = self.command(Trb::enable_slot())?.slot;
        let idx = usize::from(slot).wrapping_sub(1);
        if idx >= dma::MAX_DEVICES {
            return Err(Error::Unsupported("slot id beyond the slots enabled"));
        }
        let ctx_size = self.caps.context_size();
        let phys = |off: usize| self.dma.phys(off).ok_or(Error::Unsupported("DMA address"));
        let ep0 = Ring::new(&self.dma, area(idx) + DEV_EP0_RING)
            .ok_or(Error::Unsupported("control ring"))?;
        let device_ctx = DEVICE_CONTEXTS + idx * DEVICE_CONTEXT_STRIDE;
        self.dma.zero(device_ctx, DEVICE_CONTEXT_STRIDE);
        self.dma
            .w64(DCBAA + usize::from(slot) * 8, phys(device_ctx)?);

        self.dma
            .zero(INPUT_CONTEXT, ctx::input_context_bytes(ctx_size));
        self.dma.write_context(
            INPUT_CONTEXT,
            &ctx::input_control(0, ctx::FLAG_SLOT | ctx::flag_endpoint(1)),
        );
        self.dma.write_context(
            INPUT_CONTEXT + ctx::input_slot_offset(ctx_size),
            &ctx::slot_context(speed, port, 0, 1),
        );
        self.dma.write_context(
            INPUT_CONTEXT + ctx::input_endpoint_offset(ctx_size, 1),
            &ctx::control_endpoint_context(speed.initial_ep0_max_packet(), ep0.ring.pointer()),
        );
        let input = phys(INPUT_CONTEXT)?;
        self.command(Trb::address_device(input, slot, false))?;
        self.devices[idx] = Some(Device {
            slot,
            port,
            speed,
            ep0,
            hid: [None, None],
        });

        // The first 8 bytes tell the real size of the control endpoint's packets.
        let mut head = [0u8; 8];
        self.control_read(
            idx,
            SetupPacket::get_descriptor(DESCRIPTOR_DEVICE, 0, 8),
            &mut head,
        )?;
        if let Some(real) = usb::max_packet_size0_of(&head).and_then(|b| speed.ep0_max_packet(b))
            && real != speed.initial_ep0_max_packet()
        {
            self.set_ep0_max_packet(idx, real)?;
        }
        let mut raw = [0u8; usb::DEVICE_DESCRIPTOR_LEN];
        self.control_read(
            idx,
            SetupPacket::get_descriptor(DESCRIPTOR_DEVICE, 0, 18),
            &mut raw,
        )?;
        let Some(desc) = usb::parse_device(&raw) else {
            return Err(Error::Unsupported("device descriptor"));
        };
        klog!(
            Info,
            "usb: slot {} port {} {:?} speed, device {:04x}:{:04x}, USB {:x}.{:02x}",
            slot,
            port,
            speed,
            desc.vendor,
            desc.product,
            desc.usb_version >> 8,
            desc.usb_version & 0xFF
        );

        let mut header = [0u8; usb::CONFIG_HEADER_LEN];
        self.control_read(
            idx,
            SetupPacket::get_descriptor(DESCRIPTOR_CONFIGURATION, 0, 9),
            &mut header,
        )?;
        let Some(h) = usb::parse_config_header(&header) else {
            return Err(Error::Unsupported("configuration descriptor"));
        };
        let total = usb::config_request_len(h.total_length);
        let mut blob = [0u8; MAX_CONFIG_LEN as usize];
        let blob = &mut blob[..usize::from(total)];
        self.control_read(
            idx,
            SetupPacket::get_descriptor(DESCRIPTOR_CONFIGURATION, 0, total),
            blob,
        )?;
        let mut found = [BootHid::EMPTY; 2];
        let n = usb::boot_hid_interfaces(blob, &mut found);
        if n == 0 {
            klog!(
                Info,
                "usb: slot {}: no boot keyboard or mouse, ignored",
                slot
            );
            return Ok(false);
        }
        self.setup_hid(idx, &found[..n])?;
        Ok(true)
    }

    /// Tells the controller the control endpoint's real packet size (Evaluate Context).
    fn set_ep0_max_packet(&mut self, idx: usize, max_packet: u16) -> Result<(), Error> {
        let ctx_size = self.caps.context_size();
        let Some(dev) = self.devices[idx].as_ref() else {
            return Err(Error::Unsupported("no device"));
        };
        let (slot, ring) = (dev.slot, dev.ep0.ring.pointer());
        self.dma
            .zero(INPUT_CONTEXT, ctx::input_context_bytes(ctx_size));
        self.dma
            .write_context(INPUT_CONTEXT, &ctx::input_control(0, ctx::flag_endpoint(1)));
        self.dma.write_context(
            INPUT_CONTEXT + ctx::input_endpoint_offset(ctx_size, 1),
            &ctx::control_endpoint_context(max_packet, ring),
        );
        let input = self
            .dma
            .phys(INPUT_CONTEXT)
            .ok_or(Error::Unsupported("DMA address"))?;
        self.command(Trb::evaluate_context(input, slot))?;
        Ok(())
    }

    /// A control transfer that reads `out.len()` bytes (a GET request), or has no data stage when
    /// `out` is empty and the setup says so. `setup.length` must match.
    pub(super) fn control_read(
        &mut self,
        idx: usize,
        setup: SetupPacket,
        out: &mut [u8],
    ) -> Result<(), Error> {
        let len = usize::from(setup.length);
        if len > CONTROL_BUF_BYTES || out.len() < len {
            return Err(Error::Unsupported("control transfer length"));
        }
        let area = area(idx);
        let buf = self
            .dma
            .phys(area + DEV_CONTROL_BUF)
            .ok_or(Error::Unsupported("DMA address"))?;
        self.dma.zero(area + DEV_CONTROL_BUF, CONTROL_BUF_BYTES);
        let (trbs, n) = control_transfer(setup, buf);
        let Some(dev) = self.devices[idx].as_mut() else {
            return Err(Error::Unsupported("no device"));
        };
        let slot = dev.slot;
        self.xfer_done = None;
        let mut last = 0;
        for t in &trbs[..n] {
            last = dev.ep0.push(&self.dma, *t);
        }
        self.ring_doorbell(slot, 1);
        let end = crate::interrupts::ticks() + TRANSFER_TIMEOUT;
        loop {
            self.pump();
            if let Some(done) = self.xfer_done.take_if(|d| d.trb == last) {
                if !done.code.is_ok() {
                    return Err(Error::Transfer(done.code));
                }
                break;
            }
            if crate::interrupts::ticks() > end {
                return Err(Error::Timeout("control transfer"));
            }
            core::hint::spin_loop();
        }
        self.dma.read_bytes(area + DEV_CONTROL_BUF, &mut out[..len]);
        Ok(())
    }

    /// A control request with no data stage (SET_CONFIGURATION, SET_PROTOCOL, SET_IDLE).
    pub(super) fn control_no_data(&mut self, idx: usize, setup: SetupPacket) -> Result<(), Error> {
        self.control_read(idx, SetupPacket { length: 0, ..setup }, &mut [])
    }
}
