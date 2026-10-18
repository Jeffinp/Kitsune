//! Device contexts: what the controller reads (input context) and writes (device context) for each
//! attached device. A context is 8 dwords, padded to 32 or 64 bytes depending on `CSZ`
//! ([`super::Capabilities::context_64`]).
//!
//! Input context layout, `ctx` = context size: `ctx * 0` input control, `ctx * 1` slot, `ctx * (1 + dci)`
//! endpoint. Device context: `ctx * 0` slot, `ctx * dci` endpoint. `dci` (device context index) is 1 for
//! the default control endpoint and `2 * number + direction` for the others.

use super::Speed;

/// Contexts in an input context: control, slot, and 31 endpoints.
pub const INPUT_CONTEXTS: usize = 33;
/// Contexts in a device context: slot and 31 endpoints.
pub const DEVICE_CONTEXTS: usize = 32;

pub const fn input_context_bytes(context_size: usize) -> usize {
    context_size * INPUT_CONTEXTS
}

pub const fn device_context_bytes(context_size: usize) -> usize {
    context_size * DEVICE_CONTEXTS
}

pub const fn input_slot_offset(context_size: usize) -> usize {
    context_size
}

/// Offset of endpoint `dci` (1..=31) inside an input context.
pub const fn input_endpoint_offset(context_size: usize, dci: u8) -> usize {
    context_size * (1 + dci as usize)
}

/// Device context index of the endpoint with USB address `address` (bit 7 = IN). The default control
/// endpoint (address 0) is index 1.
pub const fn endpoint_dci(address: u8) -> u8 {
    let number = address & 0x0F;
    if number == 0 {
        1
    } else {
        number * 2 + (address >> 7)
    }
}

/// Add/drop flag for the slot context in an input control context.
pub const FLAG_SLOT: u32 = 1;

/// Add/drop flag for endpoint `dci` in an input control context.
pub const fn flag_endpoint(dci: u8) -> u32 {
    1 << (dci & 31)
}

const EP_TYPE_CONTROL: u32 = 4;
const EP_TYPE_INTERRUPT_IN: u32 = 7;
const ERROR_COUNT_3: u32 = 3 << 1;

/// Input control context: the endpoints to drop (dword 0) and add (dword 1).
pub const fn input_control(drop: u32, add: u32) -> [u32; 8] {
    [drop, add, 0, 0, 0, 0, 0, 0]
}

/// Slot context. `route_string` is 0 for a device on a root port; `last_dci` is the highest endpoint
/// index the slot will use (1 when only the control endpoint exists).
pub const fn slot_context(
    speed: Speed,
    root_port: u8,
    route_string: u32,
    last_dci: u8,
) -> [u32; 8] {
    [
        (route_string & 0xF_FFFF) | speed.slot_id() << 20 | (last_dci as u32 & 0x1F) << 27,
        (root_port as u32) << 16,
        0,
        0,
        0,
        0,
        0,
        0,
    ]
}

/// Default control endpoint (DCI 1). `ring` is a transfer ring's [`super::ring::ProducerRing::pointer`]
/// (address with the cycle state in bit 0).
pub const fn control_endpoint_context(max_packet: u16, ring: u64) -> [u32; 8] {
    [
        0,
        ERROR_COUNT_3 | EP_TYPE_CONTROL << 3 | (max_packet as u32) << 16,
        ring as u32,
        (ring >> 32) as u32,
        8,
        0,
        0,
        0,
    ]
}

/// Interrupt IN endpoint (a HID keyboard or mouse). `interval` is the xHCI field, see
/// [`interrupt_interval`].
pub const fn interrupt_in_endpoint_context(max_packet: u16, interval: u8, ring: u64) -> [u32; 8] {
    [
        (interval as u32) << 16,
        ERROR_COUNT_3 | EP_TYPE_INTERRUPT_IN << 3 | (max_packet as u32) << 16,
        ring as u32,
        (ring >> 32) as u32,
        max_packet as u32 | (max_packet as u32) << 16, // average TRB length, max ESIT payload
        0,
        0,
        0,
    ]
}

/// The xHCI interval field for an interrupt endpoint, from its `bInterval`. The field is an exponent
/// of 125 us: high and super speed use `bInterval - 1` (their descriptor is already an exponent); low and
/// full speed give milliseconds, so the exponent is `floor(log2(8 * ms))`, kept within 3..=10.
pub const fn interrupt_interval(speed: Speed, b_interval: u8) -> u8 {
    match speed {
        Speed::High | Speed::Super => {
            let v = if b_interval < 1 {
                1
            } else if b_interval > 16 {
                16
            } else {
                b_interval
            };
            v - 1
        }
        Speed::Low | Speed::Full => {
            let ms = if b_interval < 1 { 1 } else { b_interval } as u32;
            let exponent = (31 - (ms * 8).leading_zeros()) as u8;
            if exponent < 3 {
                3
            } else if exponent > 10 {
                10
            } else {
                exponent
            }
        }
    }
}

/// Copies `context` into `buffer` (dwords) at byte offset `offset`. `false`, and nothing written, when it
/// does not fit or the offset is not dword-aligned.
pub fn write_context(buffer: &mut [u32], offset: usize, context: &[u32; 8]) -> bool {
    if !offset.is_multiple_of(4) {
        return false;
    }
    let start = offset / 4;
    match buffer.get_mut(start..start + 8) {
        Some(dst) => {
            dst.copy_from_slice(context);
            true
        }
        None => false,
    }
}
