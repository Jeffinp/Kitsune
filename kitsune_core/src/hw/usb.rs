//! USB descriptors: the device descriptor and the configuration blob a device returns to GET_DESCRIPTOR,
//! and the search for the HID boot interfaces (keyboard, mouse) inside it.
//!
//! Everything a device sends is untrusted. A descriptor with a length that runs past the buffer, a
//! length below 2 (which would loop forever), or an endpoint before any interface is skipped or ends the
//! walk; nothing here panics or reads outside the slice.

pub const DESCRIPTOR_DEVICE: u8 = 1;
pub const DESCRIPTOR_CONFIGURATION: u8 = 2;
const DESCRIPTOR_INTERFACE: u8 = 4;
const DESCRIPTOR_ENDPOINT: u8 = 5;

/// Bytes of the device descriptor.
pub const DEVICE_DESCRIPTOR_LEN: usize = 18;
/// Bytes of the configuration descriptor header (the part that gives `wTotalLength`).
pub const CONFIG_HEADER_LEN: usize = 9;
/// Largest configuration blob the driver asks for; a device that claims more is read up to this.
pub const MAX_CONFIG_LEN: u16 = 1024;

const CLASS_HID: u8 = 3;
const SUBCLASS_BOOT: u8 = 1;

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]))
}

/// The fields of the device descriptor the driver uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceDescriptor {
    /// `bcdUSB` (`0x0200` = USB 2.0).
    pub usb_version: u16,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    /// `bMaxPacketSize0`: the control endpoint's packet size (an exponent for SuperSpeed).
    pub max_packet_size0: u8,
    pub vendor: u16,
    pub product: u16,
    pub configurations: u8,
}

/// Parses a device descriptor. `None` if it is too short or not a device descriptor. The first 8 bytes
/// are enough to learn `bMaxPacketSize0` (see [`max_packet_size0_of`]); the full parse needs 18.
pub fn parse_device(bytes: &[u8]) -> Option<DeviceDescriptor> {
    if bytes.len() < DEVICE_DESCRIPTOR_LEN || bytes[1] != DESCRIPTOR_DEVICE || bytes[0] < 18 {
        return None;
    }
    Some(DeviceDescriptor {
        usb_version: u16_at(bytes, 2)?,
        class: bytes[4],
        subclass: bytes[5],
        protocol: bytes[6],
        max_packet_size0: bytes[7],
        vendor: u16_at(bytes, 8)?,
        product: u16_at(bytes, 10)?,
        configurations: bytes[17],
    })
}

/// `bMaxPacketSize0` from the first 8 bytes of a device descriptor, which every device returns even
/// before the control endpoint's real packet size is known. `None` if it is not a device descriptor.
pub fn max_packet_size0_of(first8: &[u8]) -> Option<u8> {
    if first8.len() < 8 || first8[1] != DESCRIPTOR_DEVICE || first8[0] < 8 {
        return None;
    }
    Some(first8[7])
}

/// The configuration descriptor header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfigHeader {
    /// `wTotalLength`: the size of the whole blob (header, interfaces, endpoints, class descriptors).
    pub total_length: u16,
    pub interfaces: u8,
    /// `bConfigurationValue`, the argument of SET_CONFIGURATION.
    pub value: u8,
}

pub fn parse_config_header(bytes: &[u8]) -> Option<ConfigHeader> {
    if bytes.len() < CONFIG_HEADER_LEN || bytes[1] != DESCRIPTOR_CONFIGURATION || bytes[0] < 9 {
        return None;
    }
    Some(ConfigHeader {
        total_length: u16_at(bytes, 2)?,
        interfaces: bytes[4],
        value: bytes[5],
    })
}

/// How many bytes to request for the whole configuration: `wTotalLength`, never below the header and
/// never above [`MAX_CONFIG_LEN`].
pub const fn config_request_len(total_length: u16) -> u16 {
    if total_length < CONFIG_HEADER_LEN as u16 {
        CONFIG_HEADER_LEN as u16
    } else if total_length > MAX_CONFIG_LEN {
        MAX_CONFIG_LEN
    } else {
        total_length
    }
}

/// What a boot-protocol HID interface is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootProtocol {
    Keyboard,
    Mouse,
}

/// A HID boot interface and the interrupt IN endpoint its reports arrive on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootHid {
    /// `bConfigurationValue` to select with SET_CONFIGURATION.
    pub configuration: u8,
    pub interface: u8,
    pub protocol: BootProtocol,
    /// Endpoint address (bit 7 set: IN).
    pub endpoint: u8,
    /// `wMaxPacketSize` bits 10:0.
    pub max_packet: u16,
    /// `bInterval`, in the unit of the device's speed (see `xhci::context::interrupt_interval`).
    pub interval: u8,
}

/// Finds the boot keyboards and mice in a configuration blob and stores them in `out`, returning how
/// many were stored (the rest of the blob is ignored once `out` is full). Only alternate setting 0 is
/// considered, and only interfaces with class HID, subclass 1 (boot) and protocol 1 or 2 that have an
/// interrupt IN endpoint. Composite devices (a keyboard plus a media-key interface) yield just the boot
/// interface.
pub fn boot_hid_interfaces(config: &[u8], out: &mut [BootHid]) -> usize {
    let Some(header) = parse_config_header(config) else {
        return 0;
    };
    // Trust wTotalLength only as far as the bytes we actually have.
    let end = config.len().min(header.total_length as usize);
    let mut count = 0;
    // Index into `out` of the interface whose endpoints are being read, if it is a candidate.
    let mut current: Option<usize> = None;
    let mut offset = usize::from(config[0]).max(CONFIG_HEADER_LEN);
    while offset + 2 <= end {
        let len = usize::from(config[offset]);
        if len < 2 || offset + len > end {
            break;
        }
        let d = &config[offset..offset + len];
        match d[1] {
            DESCRIPTOR_INTERFACE if len >= 9 => {
                current = None;
                let protocol = match (d[5], d[6], d[7]) {
                    (CLASS_HID, SUBCLASS_BOOT, 1) => Some(BootProtocol::Keyboard),
                    (CLASS_HID, SUBCLASS_BOOT, 2) => Some(BootProtocol::Mouse),
                    _ => None,
                };
                if let Some(protocol) = protocol
                    && d[3] == 0
                    && count < out.len()
                {
                    out[count] = BootHid {
                        configuration: header.value,
                        interface: d[2],
                        protocol,
                        endpoint: 0, // filled in by its endpoint descriptor; 0 = none yet
                        max_packet: 0,
                        interval: 0,
                    };
                    current = Some(count);
                    count += 1;
                }
            }
            DESCRIPTOR_ENDPOINT if len >= 7 => {
                if let Some(i) = current {
                    let is_interrupt_in = d[2] & 0x80 != 0 && d[3] & 0b11 == 0b11;
                    if is_interrupt_in && out[i].endpoint == 0 {
                        out[i].endpoint = d[2];
                        out[i].max_packet = u16::from_le_bytes([d[4], d[5]]) & 0x7FF;
                        out[i].interval = d[6];
                    }
                }
            }
            _ => {}
        }
        offset += len;
    }
    // An interface with no interrupt IN endpoint cannot deliver reports: drop it, keeping the order.
    let mut kept = 0;
    for i in 0..count {
        if out[i].endpoint != 0 {
            out[kept] = out[i];
            kept += 1;
        }
    }
    kept
}

#[cfg(test)]
mod tests;
